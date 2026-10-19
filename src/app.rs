// SPDX-License-Identifier: GPL-3.0-or-later
//! Состояние приложения на главном потоке и диспетчер событий.
//!
//! Все внешние источники (tray-icon, скрытое окно, таймеры, панель) шлют [`AppEvent`]
//! через [`post`]: событие выполняется позже в event loop Slint, без реентерабельности
//! внутри оконных процедур.

use std::cell::RefCell;

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::config::log_dir;
use crate::devices::manager::{self, Command, MouseSnapshot};
use crate::hidden_window::HiddenWindow;
use crate::panel::PanelController;
use crate::state::{AppState, BatteryState, DeviceKind, LOW_BATTERY, Link};
use crate::theme::SystemTheme;
use crate::tray::{self, Tray, autostart};
use crate::win::wide;

#[derive(Debug, Clone, Copy)]
pub enum MenuCommand {
    ToggleAutostart,
    OpenLogs,
    Exit,
}

#[derive(Debug)]
pub enum AppEvent {
    /// Левый клик по иконке (прямоугольник иконки в физ. px).
    TrayClick(tray_icon::Rect),
    Menu(MenuCommand),
    /// Запрос от второго экземпляра.
    ShowPanel,
    /// Esc в панели.
    PanelDismiss,
    PanelFocusLost,
    /// Панель скрыта дольше порога — уничтожить окно.
    PanelIdle,
    ThemeChanged,
    DevicesChanged,
    /// `hubless --test-toast`: показать тестовое уведомление.
    TestNotification,
    /// Состояние мыши от device manager (`None` — не найдена).
    Mouse(Option<MouseSnapshot>),
}

struct App {
    tray: Tray,
    panel: PanelController,
    state: AppState,
    /// Уведомление о низком заряде уже показано (сбрасывается при зарядке или заряде ≥ порога).
    low_notified: bool,
    devices: manager::Handle,
    _hidden: HiddenWindow,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn init() -> Result<(), Box<dyn std::error::Error>> {
    let theme = SystemTheme::read();
    tracing::info!(?theme, "системная тема");
    let app = App {
        tray: Tray::new(theme.taskbar_light)?,
        panel: PanelController::new(theme),
        state: AppState::default(),
        low_notified: false,
        devices: manager::spawn(|s| post(AppEvent::Mouse(s))),
        _hidden: HiddenWindow::create()?,
    };
    APP.set(Some(app));
    Ok(())
}

pub fn shutdown() {
    tray::remove_handlers();
    // Drop убирает иконку из трея и закрывает окна.
    drop(APP.take());
}

/// Поставить событие в очередь event loop (можно из любого потока).
pub fn post(ev: AppEvent) {
    if let Err(e) = slint::invoke_from_event_loop(move || dispatch(ev)) {
        tracing::warn!("invoke_from_event_loop: {e}");
    }
}

fn dispatch(ev: AppEvent) {
    APP.with_borrow_mut(|app| {
        if let Some(app) = app {
            app.handle(ev);
        }
    });
}

impl App {
    fn handle(&mut self, ev: AppEvent) {
        tracing::debug!(?ev, "event");
        match ev {
            AppEvent::TrayClick(rect) => self.panel.toggle(Some(rect), &self.state),
            AppEvent::ShowPanel => self.panel.show(self.tray.rect(), &self.state),
            AppEvent::PanelDismiss | AppEvent::PanelFocusLost => self.panel.hide(),
            AppEvent::PanelIdle => self.panel.destroy_if_hidden(),
            AppEvent::ThemeChanged => {
                let theme = SystemTheme::read();
                tracing::info!(?theme, "тема изменилась");
                self.tray.set_taskbar_light(theme.taskbar_light);
                self.panel.set_theme(theme);
            }
            AppEvent::DevicesChanged => {
                tracing::debug!("HID hotplug");
                self.devices.send(Command::Rescan);
            }
            AppEvent::Mouse(s) => self.on_mouse(s),
            AppEvent::TestNotification => self.tray.notify(
                "Hubless: проверка уведомлений",
                "Так выглядит предупреждение о низком заряде.",
            ),
            AppEvent::Menu(cmd) => self.menu(cmd),
        }
    }

    fn on_mouse(&mut self, s: Option<MouseSnapshot>) {
        tracing::info!(?s, "мышь");
        if let Some(m) = self.state.device_mut(DeviceKind::Mouse) {
            m.connected = s.as_ref().is_some_and(|s| s.connected);
            m.link = s
                .as_ref()
                .map(|s| if s.wired { Link::Usb } else { Link::Wireless });
            m.battery = s.as_ref().and_then(|s| s.battery).map(|b| BatteryState {
                percent: b.percent,
                charging: b.charging.is_charging() || b.external_power,
            });
            if let Some(s) = &s {
                m.name.clone_from(&s.name);
            }
        }
        self.refresh_status();
    }

    /// Трей, уведомление о низком заряде и панель по текущему состоянию.
    fn refresh_status(&mut self) {
        let low = self.state.any_low_battery();
        self.tray.set_status(&self.state.tray_lines(), low);
        if low && !self.low_notified {
            let names: Vec<String> = self
                .state
                .devices
                .iter()
                .filter(|d| d.connected && d.battery.is_some_and(BatteryState::is_low))
                .map(|d| format!("{} — {}", d.name, d.battery_text().unwrap_or_default()))
                .collect();
            self.tray
                .notify(&format!("Заряд ниже {LOW_BATTERY}%"), &names.join("\n"));
        }
        self.low_notified = low;
        self.panel.refresh(&self.state);
    }

    fn menu(&mut self, cmd: MenuCommand) {
        match cmd {
            MenuCommand::ToggleAutostart => {
                let enable = !autostart::is_enabled();
                if let Err(e) = autostart::set_enabled(enable) {
                    tracing::error!("автозапуск: {e}");
                }
                let enabled = autostart::is_enabled();
                tracing::info!(enabled, "автозапуск");
                self.tray.set_autostart_checked(enabled);
            }
            MenuCommand::OpenLogs => open_folder(&log_dir()),
            MenuCommand::Exit => {
                tracing::info!("выход");
                self.panel.destroy();
                let _ = slint::quit_event_loop();
            }
        }
    }
}

fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let path = wide(path);
    // SAFETY: строки живут до конца вызова.
    let res = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(path.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW возвращает значение > 32 при успехе.
    if res.0 as isize <= 32 {
        tracing::warn!("ShellExecuteW: {}", res.0 as isize);
    }
}

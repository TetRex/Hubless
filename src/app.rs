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
use crate::hidden_window::HiddenWindow;
use crate::panel::PanelController;
use crate::state::AppState;
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
}

struct App {
    tray: Tray,
    panel: PanelController,
    state: AppState,
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
            // HID-ядро появится в фазе 1.
            AppEvent::DevicesChanged => tracing::debug!("HID hotplug"),
            AppEvent::Menu(cmd) => self.menu(cmd),
        }
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

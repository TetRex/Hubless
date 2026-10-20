// SPDX-License-Identifier: GPL-3.0-or-later
//! Иконка в трее и контекстное меню.

pub mod autostart;
mod icon;
pub mod toast;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::app::{self, AppEvent, MenuCommand};
use crate::i18n::tr;

const ID_STATUS: &str = "status";
const ID_SETTINGS: &str = "settings";
const ID_AUTOSTART: &str = "autostart";
const ID_LOGS: &str = "logs";
const ID_EXIT: &str = "exit";

const NO_DEVICES: &str = "No devices connected";

pub struct Tray {
    icon: TrayIcon,
    autostart: CheckMenuItem,
    /// Неактивный пункт меню с зарядом устройств.
    status: MenuItem,
    /// Пункты с подписями — переименовываются при смене языка.
    settings: MenuItem,
    logs: MenuItem,
    exit: MenuItem,
    taskbar_light: bool,
    low_battery: bool,
}

impl Tray {
    pub fn new(taskbar_light: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let autostart = CheckMenuItem::with_id(
            ID_AUTOSTART,
            tr("Start with Windows"),
            true,
            autostart::is_enabled(),
            None,
        );
        let status = MenuItem::with_id(ID_STATUS, tr(NO_DEVICES), false, None);
        let settings = MenuItem::with_id(ID_SETTINGS, tr("Settings…"), true, None);
        let logs = MenuItem::with_id(ID_LOGS, tr("Open log folder"), true, None);
        let exit = MenuItem::with_id(ID_EXIT, tr("Exit"), true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &settings,
            &autostart,
            &logs,
            &PredefinedMenuItem::separator(),
            &exit,
        ])?;

        let icon = TrayIconBuilder::new()
            .with_icon(icon::render(taskbar_light, false))
            .with_tooltip(format!("Hubless\n{}", tr(NO_DEVICES)))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        install_handlers();
        Ok(Self {
            icon,
            autostart,
            status,
            settings,
            logs,
            exit,
            taskbar_light,
            low_battery: false,
        })
    }

    /// Положение иконки в физических пикселях (`Shell_NotifyIconGetRect`).
    pub fn rect(&self) -> Option<tray_icon::Rect> {
        self.icon.rect()
    }

    pub fn set_taskbar_light(&mut self, light: bool) {
        if light != self.taskbar_light {
            self.taskbar_light = light;
            self.redraw_icon();
        }
    }

    /// Строки «устройство: заряд» — в tooltip (по строке на устройство) и в первый пункт меню;
    /// индикатор низкого заряда на иконке.
    pub fn set_status(&mut self, lines: &[String], low_battery: bool) {
        let (tooltip, menu) = if lines.is_empty() {
            (tr(NO_DEVICES).to_owned(), tr(NO_DEVICES).to_owned())
        } else {
            (lines.join("\n"), lines.join(" · "))
        };
        if let Err(e) = self.icon.set_tooltip(Some(format!("Hubless\n{tooltip}"))) {
            tracing::warn!("set_tooltip: {e}");
        }
        self.status.set_text(menu);
        if low_battery != self.low_battery {
            self.low_battery = low_battery;
            self.redraw_icon();
        }
    }

    fn redraw_icon(&self) {
        let icon = icon::render(self.taskbar_light, self.low_battery);
        if let Err(e) = self.icon.set_icon(Some(icon)) {
            tracing::warn!("set_icon: {e}");
        }
    }

    /// Уведомление Windows от имени иконки в трее.
    pub fn notify(&self, title: &str, text: &str) {
        let hwnd = windows::Win32::Foundation::HWND(self.icon.window_handle());
        toast::show(hwnd, title, text);
    }

    /// Подписи меню на текущем языке (строка состояния обновляется в `set_status`).
    pub fn relabel(&self) {
        self.settings.set_text(tr("Settings…"));
        self.autostart.set_text(tr("Start with Windows"));
        self.logs.set_text(tr("Open log folder"));
        self.exit.set_text(tr("Exit"));
    }

    pub fn set_autostart_checked(&self, checked: bool) {
        self.autostart.set_checked(checked);
    }
}

/// Обработчики tray-icon вызываются из оконной процедуры; всё переносим в очередь event loop.
fn install_handlers() {
    TrayIconEvent::set_event_handler(Some(|ev| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } = ev
        {
            app::post(AppEvent::TrayClick(rect));
        }
    }));
    MenuEvent::set_event_handler(Some(|ev: MenuEvent| {
        let cmd = match ev.id.as_ref() {
            ID_SETTINGS => MenuCommand::Settings,
            ID_AUTOSTART => MenuCommand::ToggleAutostart,
            ID_LOGS => MenuCommand::OpenLogs,
            ID_EXIT => MenuCommand::Exit,
            _ => return,
        };
        app::post(AppEvent::Menu(cmd));
    }));
}

pub fn remove_handlers() {
    TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
    MenuEvent::set_event_handler(None::<fn(MenuEvent)>);
}

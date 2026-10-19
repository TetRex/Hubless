// SPDX-License-Identifier: GPL-3.0-or-later
//! Иконка в трее и контекстное меню.

pub mod autostart;
mod icon;
pub mod toast;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::app::{self, AppEvent, MenuCommand};

const ID_STATUS: &str = "status";
const ID_AUTOSTART: &str = "autostart";
const ID_LOGS: &str = "logs";
const ID_EXIT: &str = "exit";

const NO_DEVICES: &str = "Нет подключённых устройств";

pub struct Tray {
    icon: TrayIcon,
    autostart: CheckMenuItem,
    /// Неактивный пункт меню с зарядом устройств.
    status: MenuItem,
    taskbar_light: bool,
    low_battery: bool,
}

impl Tray {
    pub fn new(taskbar_light: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let autostart = CheckMenuItem::with_id(
            ID_AUTOSTART,
            "Автозапуск",
            true,
            autostart::is_enabled(),
            None,
        );
        let status = MenuItem::with_id(ID_STATUS, NO_DEVICES, false, None);
        let menu = Menu::new();
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &autostart,
            &MenuItem::with_id(ID_LOGS, "Открыть папку логов", true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(ID_EXIT, "Выход", true, None),
        ])?;

        let icon = TrayIconBuilder::new()
            .with_icon(icon::render(taskbar_light, false))
            .with_tooltip(format!("Hubless\n{NO_DEVICES}"))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        install_handlers();
        Ok(Self {
            icon,
            autostart,
            status,
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
            (NO_DEVICES.to_owned(), NO_DEVICES.to_owned())
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

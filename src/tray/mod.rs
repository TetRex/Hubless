// SPDX-License-Identifier: GPL-3.0-or-later
//! Иконка в трее и контекстное меню.

pub mod autostart;
mod icon;

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::app::{self, AppEvent, MenuCommand};

const ID_AUTOSTART: &str = "autostart";
const ID_LOGS: &str = "logs";
const ID_EXIT: &str = "exit";

pub struct Tray {
    icon: TrayIcon,
    autostart: CheckMenuItem,
    taskbar_light: bool,
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
        let menu = Menu::new();
        menu.append_items(&[
            &autostart,
            &MenuItem::with_id(ID_LOGS, "Открыть папку логов", true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(ID_EXIT, "Выход", true, None),
        ])?;

        let icon = TrayIconBuilder::new()
            .with_icon(icon::render(taskbar_light, false))
            .with_tooltip("ProTray\nНет подключённых устройств")
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        install_handlers();
        Ok(Self {
            icon,
            autostart,
            taskbar_light,
        })
    }

    /// Положение иконки в физических пикселях (`Shell_NotifyIconGetRect`).
    pub fn rect(&self) -> Option<tray_icon::Rect> {
        self.icon.rect()
    }

    pub fn set_taskbar_light(&mut self, light: bool) {
        if light != self.taskbar_light {
            self.taskbar_light = light;
            if let Err(e) = self.icon.set_icon(Some(icon::render(light, false))) {
                tracing::warn!("set_icon: {e}");
            }
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

// SPDX-License-Identifier: GPL-3.0-or-later
//! Tray icon. There is no context menu: any click on the icon opens the panel, and exit,
//! settings and everything else are in the panel and the settings window.

pub mod autostart;
pub mod icon;
pub mod toast;

use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::app::{self, AppEvent};
use crate::i18n::tr;

const NO_DEVICES: &str = "No devices connected";

pub struct Tray {
    icon: TrayIcon,
    taskbar_light: bool,
    low_battery: bool,
}

impl Tray {
    pub fn new(taskbar_light: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let icon = TrayIconBuilder::new()
            .with_icon(icon::render(taskbar_light, false))
            .with_tooltip(format!("Hubless\n{}", tr(NO_DEVICES)))
            .build()?;

        install_handlers();
        Ok(Self {
            icon,
            taskbar_light,
            low_battery: false,
        })
    }

    /// Icon position in physical pixels (`Shell_NotifyIconGetRect`).
    pub fn rect(&self) -> Option<tray_icon::Rect> {
        self.icon.rect()
    }

    pub fn set_taskbar_light(&mut self, light: bool) {
        if light != self.taskbar_light {
            self.taskbar_light = light;
            self.redraw_icon();
        }
    }

    /// "Device: battery" lines go to the tooltip (one line per device); the low battery indicator
    /// goes on the icon.
    pub fn set_status(&mut self, lines: &[String], low_battery: bool) {
        let tooltip = if lines.is_empty() {
            tr(NO_DEVICES).to_owned()
        } else {
            lines.join("\n")
        };
        if let Err(e) = self.icon.set_tooltip(Some(format!("Hubless\n{tooltip}"))) {
            tracing::warn!("set_tooltip: {e}");
        }
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

    /// A Windows notification on behalf of the tray icon.
    pub fn notify(&self, title: &str, text: &str) {
        let hwnd = windows::Win32::Foundation::HWND(self.icon.window_handle());
        toast::show(hwnd, title, text);
    }
}

/// The tray-icon handler is called from the window procedure; we move the event to the event loop queue.
/// Left and right clicks both open and close the panel (the icon has no menu).
fn install_handlers() {
    TrayIconEvent::set_event_handler(Some(|ev| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left | MouseButton::Right,
            button_state: MouseButtonState::Up,
            rect,
            ..
        } = ev
        {
            app::post(AppEvent::TrayClick(rect));
        }
    }));
}

pub fn remove_handlers() {
    TrayIconEvent::set_event_handler(None::<fn(TrayIconEvent)>);
}

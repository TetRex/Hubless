// SPDX-License-Identifier: GPL-3.0-or-later
//! A hidden window for system notifications: theme changes (`WM_SETTINGCHANGE`),
//! HID hotplug (`WM_DEVICECHANGE`) and signals from a second instance.
//!
//! The window is top-level (not message-only), otherwise the `WM_SETTINGCHANGE` broadcast wouldn't reach it.

use std::ffi::c_void;

use windows::Win32::Devices::HumanInterfaceDevice::GUID_DEVINTERFACE_HID;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, CreateWindowExW, DBT_DEVICEARRIVAL,
    DBT_DEVICEREMOVECOMPLETE, DBT_DEVTYP_DEVICEINTERFACE, DEV_BROADCAST_DEVICEINTERFACE_W,
    DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DestroyWindow, FindWindowW, HDEVNOTIFY,
    PostMessageW, RegisterClassW, RegisterDeviceNotificationW, UnregisterDeviceNotification,
    WINDOW_STYLE, WM_APP, WM_DEVICECHANGE, WM_SETTINGCHANGE, WNDCLASSW, WS_EX_TOOLWINDOW,
};
use windows::core::{PCWSTR, w};

use crate::app::{self, AppEvent};

const CLASS: PCWSTR = w!("Hubless.Hidden");
const WM_APP_SHOW_PANEL: u32 = WM_APP + 1;
const WM_APP_TEST_NOTIFICATION: u32 = WM_APP + 2;
const WM_APP_SETTINGS_CHANGED: u32 = WM_APP + 3;
const WM_APP_QUIT: u32 = WM_APP + 4;
const WM_APP_SETTINGS: u32 = WM_APP + 5;

pub struct HiddenWindow {
    hwnd: HWND,
    notify: Option<HDEVNOTIFY>,
}

impl HiddenWindow {
    pub fn create() -> windows::core::Result<Self> {
        // SAFETY: registering the class and creating the window on the current (main) thread;
        // wndproc is an extern "system" function with a static lifetime.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS,
                w!("Hubless"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance.into()),
                None,
            )?;

            let mut filter = DEV_BROADCAST_DEVICEINTERFACE_W {
                dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
                dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE.0,
                dbcc_classguid: GUID_DEVINTERFACE_HID,
                ..Default::default()
            };
            let notify = RegisterDeviceNotificationW(
                HANDLE(hwnd.0),
                (&raw mut filter).cast::<c_void>(),
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )
            .inspect_err(|e| tracing::warn!("RegisterDeviceNotificationW: {e}"))
            .ok();

            Ok(Self { hwnd, notify })
        }
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        // SAFETY: the handles were created in `create` and are released once.
        unsafe {
            if let Some(n) = self.notify.take() {
                let _ = UnregisterDeviceNotification(n);
            }
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Asks the running instance to open the panel.
pub fn notify_running_instance() {
    // SAFETY: finds the window by class name and posts a message asynchronously.
    unsafe {
        if let Ok(hwnd) = FindWindowW(CLASS, None) {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            let _ = PostMessageW(Some(hwnd), WM_APP_SHOW_PANEL, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn is_running() -> bool {
    // SAFETY: finds the window by class name.
    unsafe { FindWindowW(CLASS, None).is_ok() }
}

/// Tells the running instance that `devices.toml` changed; `false` if it isn't running.
pub fn notify_settings_changed() -> bool {
    // SAFETY: finds the window by class name and posts a message asynchronously.
    unsafe {
        FindWindowW(CLASS, None).is_ok_and(|hwnd| {
            PostMessageW(Some(hwnd), WM_APP_SETTINGS_CHANGED, WPARAM(0), LPARAM(0)).is_ok()
        })
    }
}

/// Asks the running instance to open the settings window on section `page`
/// (0 "General", 1 "Devices", 2 "Profiles", 3 "Advanced"); `false` if it isn't running.
pub fn request_settings(page: usize) -> bool {
    // SAFETY: finds the window by class name and posts a message asynchronously.
    unsafe {
        FindWindowW(CLASS, None).is_ok_and(|hwnd| {
            let _ = AllowSetForegroundWindow(ASFW_ANY);
            PostMessageW(Some(hwnd), WM_APP_SETTINGS, WPARAM(page), LPARAM(0)).is_ok()
        })
    }
}

/// Asks the running instance to exit normally (like the panel's Exit button); `false` if it isn't running.
pub fn request_quit() -> bool {
    // SAFETY: finds the window by class name and posts a message asynchronously.
    unsafe {
        FindWindowW(CLASS, None)
            .is_ok_and(|hwnd| PostMessageW(Some(hwnd), WM_APP_QUIT, WPARAM(0), LPARAM(0)).is_ok())
    }
}

/// Asks the running instance to show a test notification; `false` if it isn't running.
pub fn request_test_notification() -> bool {
    // SAFETY: finds the window by class name and posts a message asynchronously.
    unsafe {
        FindWindowW(CLASS, None).is_ok_and(|hwnd| {
            PostMessageW(Some(hwnd), WM_APP_TEST_NOTIFICATION, WPARAM(0), LPARAM(0)).is_ok()
        })
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SETTINGCHANGE if is_color_set_change(lparam) => app::post(AppEvent::ThemeChanged),
        WM_DEVICECHANGE
            if matches!(
                wparam.0 as u32,
                DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE
            ) =>
        {
            app::post(AppEvent::DevicesChanged)
        }
        WM_APP_SHOW_PANEL => app::post(AppEvent::ShowPanel),
        WM_APP_TEST_NOTIFICATION => app::post(AppEvent::TestNotification),
        // The CLI changed devices.toml: rescan and apply the settings.
        WM_APP_SETTINGS_CHANGED => app::post(AppEvent::DevicesChanged),
        WM_APP_QUIT => app::post(AppEvent::Exit),
        WM_APP_SETTINGS => app::post(AppEvent::OpenSettings(Some(wparam.0))),
        _ => {}
    }
    // SAFETY: default handling of the other messages.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A theme/accent change arrives as `WM_SETTINGCHANGE` with lParam = "ImmersiveColorSet".
fn is_color_set_change(lparam: LPARAM) -> bool {
    if lparam.0 == 0 {
        return false;
    }
    // SAFETY: for WM_SETTINGCHANGE a non-zero lParam is a null-terminated UTF-16 string.
    let s = unsafe { PCWSTR(lparam.0 as *const u16).to_string() };
    s.is_ok_and(|s| s == "ImmersiveColorSet")
}

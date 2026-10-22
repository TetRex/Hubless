// SPDX-License-Identifier: GPL-3.0-or-later
//! Tracking the foreground window for automatic profile switching (Plan.md, phase 6).
//!
//! `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` out of context: Windows calls the callback on the main
//! thread from its message loop, and only when the foreground window changes, with no polling. Windows of
//! Hubless itself are skipped so that opening the panel doesn't reset a game profile.

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, EnumWindows, GW_OWNER, GetWindow, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsWindowVisible, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};
use windows::core::{BOOL, PWSTR};

use crate::app::{self, AppEvent};

/// The foreground window hook; removed on drop.
pub struct ForegroundHook(HWINEVENTHOOK);

impl ForegroundHook {
    pub fn install() -> Option<Self> {
        // SAFETY: the callback is an extern "system" function with a static lifetime; the hook is installed
        // on the main thread, which has a message loop (the Slint event loop).
        let hook = unsafe {
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_foreground),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if hook.is_invalid() {
            tracing::warn!("SetWinEventHook не удался — автопереключение профилей выключено");
            return None;
        }
        Some(Self(hook))
    }
}

impl Drop for ForegroundHook {
    fn drop(&mut self) {
        // SAFETY: the hook came from `install` and is removed once.
        let _ = unsafe { UnhookWinEvent(self.0) };
    }
}

unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if let Some(exe) = exe_of(hwnd) {
        app::post(AppEvent::Foreground(exe));
    }
}

/// System wrapper processes with windows (Store apps, search, the Start menu): binding a profile
/// to them makes no sense, since their foreground window always belongs to another app.
const SYSTEM_HOSTS: &[&str] = &[
    "applicationframehost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "textinputhost.exe",
];

/// Programs with visible top-level windows (exe names without repeats, sorted), except
/// Hubless itself, for binding profiles in the settings window.
pub fn running_apps() -> Vec<String> {
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: the callback gets a pointer to a local Vec through lParam and runs synchronously,
    // inside the EnumWindows call.
    unsafe {
        let _ = EnumWindows(Some(collect_window), LPARAM((&raw mut windows) as isize));
    }
    let own = std::process::id();
    let mut apps: Vec<String> = windows
        .into_iter()
        .filter(|&h| {
            let mut pid = 0u32;
            // SAFETY: writes the pid into a local variable.
            unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
            pid != own
        })
        .filter_map(exe_of)
        .filter(|exe| !SYSTEM_HOSTS.contains(&exe.as_str()))
        .collect();
    apps.sort();
    apps.dedup();
    apps
}

unsafe extern "system" fn collect_window(hwnd: HWND, list: LPARAM) -> BOOL {
    // SAFETY: only reads window properties; windows without a title, hidden ones and ones owned by other
    // windows (dialogs, tooltips) are skipped.
    let wanted = unsafe {
        IsWindowVisible(hwnd).as_bool()
            && GetWindowTextLengthW(hwnd) > 0
            && GetWindow(hwnd, GW_OWNER).is_err()
    };
    if wanted {
        // SAFETY: lParam is a pointer to the Vec from `running_apps`, alive for the duration of EnumWindows.
        unsafe { (*(list.0 as *mut Vec<HWND>)).push(hwnd) };
    }
    BOOL(1)
}

/// The exe name of the window's process (without the path, lowercase).
fn exe_of(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    // SAFETY: writes the pid into a local variable; the window may no longer exist, and then pid = 0.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    // SAFETY: the process is opened only to read its name; the handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: the buffer and its length are local.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: the handle was obtained above.
    let _ = unsafe { CloseHandle(process) };
    ok.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit(['\\', '/'])
        .next()
        .map(str::to_lowercase)
        .filter(|s| !s.is_empty())
}

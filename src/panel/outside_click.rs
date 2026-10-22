// SPDX-License-Identifier: GPL-3.0-or-later
//! Closing the panel with a click outside Hubless's windows.
//!
//! Normally the panel closes on focus loss. But Windows doesn't always give focus to a background
//! process (foreground lock): the panel is visible, the previous window stays active, and there is no
//! focus loss, so the panel didn't close on a click on another window (verified 2026-10-20: after
//! opening, another app's window stayed active). So while the panel is shown, a low-level mouse hook
//! is installed: a button press over another process's window closes it. The hook exists only while
//! the panel is shown; there is none while idle.

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetWindowThreadProcessId, HC_ACTION, HHOOK, MSLLHOOKSTRUCT, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN,
    WM_XBUTTONDOWN, WindowFromPoint,
};

use crate::app::{self, AppEvent};

/// A mouse hook for as long as the panel is shown; removed on drop.
pub struct OutsideClick(HHOOK);

impl OutsideClick {
    pub fn install() -> Option<Self> {
        // SAFETY: the callback is an extern "system" function with a static lifetime; the hook is installed
        // on the main thread, which has a message loop (the Slint event loop), and Windows calls the
        // callback from it.
        let hook = unsafe {
            let module = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
            SetWindowsHookExW(WH_MOUSE_LL, Some(on_mouse), module, 0)
        };
        match hook {
            Ok(h) => Some(Self(h)),
            Err(e) => {
                tracing::warn!("хук мыши для панели: {e}");
                None
            }
        }
    }
}

impl Drop for OutsideClick {
    fn drop(&mut self) {
        // SAFETY: the hook came from `install` and is removed once.
        let _ = unsafe { UnhookWindowsHookEx(self.0) };
    }
}

unsafe extern "system" fn on_mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let press = matches!(
        wparam.0 as u32,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    );
    if code == HC_ACTION as i32 && press {
        // SAFETY: for WH_MOUSE_LL with HC_ACTION, lparam points to an MSLLHOOKSTRUCT.
        let pt = unsafe { (*(lparam.0 as *const MSLLHOOKSTRUCT)).pt };
        if !own_window_at(pt) {
            // Through the queue, like a focus loss: the callback must return quickly.
            app::post(AppEvent::PanelFocusLost);
        }
    }
    // SAFETY: passes the event to the next hook with the original parameters.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// The window under the point belongs to Hubless (the panel, its popup lists, the settings window).
fn own_window_at(pt: POINT) -> bool {
    // SAFETY: the functions only read window state.
    unsafe {
        let hwnd = WindowFromPoint(pt);
        if hwnd.is_invalid() {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

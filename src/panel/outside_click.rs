// SPDX-License-Identifier: GPL-3.0-or-later
//! Закрытие панели щелчком вне окон Hubless.
//!
//! Обычно панель закрывается по потере фокуса. Но Windows не всегда отдаёт фокус фоновому
//! процессу (foreground lock): панель видна, а активным остаётся прежнее окно, и потери фокуса
//! не бывает — панель не закрывалась щелчком по другому окну (проверено 2026-10-20: после
//! открытия активным осталось чужое окно). Поэтому, пока панель показана, стоит низкоуровневый
//! хук мыши: нажатие кнопки над окном другого процесса закрывает её. Хук ставится только на время
//! показа — в простое его нет.

use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetWindowThreadProcessId, HC_ACTION, HHOOK, MSLLHOOKSTRUCT, SetWindowsHookExW,
    UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN,
    WM_XBUTTONDOWN, WindowFromPoint,
};

use crate::app::{self, AppEvent};

/// Хук мыши на время показа панели; снимается при drop.
pub struct OutsideClick(HHOOK);

impl OutsideClick {
    pub fn install() -> Option<Self> {
        // SAFETY: колбэк — extern "system" функция со статическим временем жизни; хук ставится
        // на главном потоке, у которого есть цикл сообщений (event loop Slint) — Windows вызывает
        // колбэк из него.
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
        // SAFETY: хук получен в `install` и снимается один раз.
        let _ = unsafe { UnhookWindowsHookEx(self.0) };
    }
}

unsafe extern "system" fn on_mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let press = matches!(
        wparam.0 as u32,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    );
    if code == HC_ACTION as i32 && press {
        // SAFETY: для WH_MOUSE_LL с HC_ACTION lparam указывает на MSLLHOOKSTRUCT.
        let pt = unsafe { (*(lparam.0 as *const MSLLHOOKSTRUCT)).pt };
        if !own_window_at(pt) {
            // Через очередь, как потеря фокуса: колбэк должен вернуться быстро.
            app::post(AppEvent::PanelFocusLost);
        }
    }
    // SAFETY: передача события следующему хуку с исходными параметрами.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Под точкой окно Hubless (панель, её всплывающие списки, окно настроек).
fn own_window_at(pt: POINT) -> bool {
    // SAFETY: функции только читают состояние окон.
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

// SPDX-License-Identifier: GPL-3.0-or-later
//! Слежение за активным окном для автопереключения профилей (Plan.md, фаза 6).
//!
//! `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` вне контекста: Windows вызывает колбэк на главном
//! потоке из его цикла сообщений и только при смене активного окна — без опроса. Окна самого
//! Hubless пропускаются, чтобы открытие панели не сбрасывало игровой профиль.

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, GetWindowThreadProcessId, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS,
};
use windows::core::PWSTR;

use crate::app::{self, AppEvent};

/// Хук активного окна; снимается при drop.
pub struct ForegroundHook(HWINEVENTHOOK);

impl ForegroundHook {
    pub fn install() -> Option<Self> {
        // SAFETY: колбэк — extern "system" функция со статическим временем жизни; хук ставится
        // на главном потоке, у которого есть цикл сообщений (event loop Slint).
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
        // SAFETY: хук получен в `install` и снимается один раз.
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

/// Имя exe процесса окна (без пути, в нижнем регистре).
fn exe_of(hwnd: HWND) -> Option<String> {
    let mut pid = 0u32;
    // SAFETY: запись pid в локальную переменную; окно может уже не существовать — тогда pid = 0.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return None;
    }
    // SAFETY: открываем процесс только на чтение имени; handle закрывается ниже.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: буфер и его длина — локальные.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: handle получен выше.
    let _ = unsafe { CloseHandle(process) };
    ok.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    path.rsplit(['\\', '/'])
        .next()
        .map(str::to_lowercase)
        .filter(|s| !s.is_empty())
}

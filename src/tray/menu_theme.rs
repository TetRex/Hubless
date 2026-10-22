// SPDX-License-Identifier: GPL-3.0-or-later
//! Тема контекстного меню трея. Win32-меню по умолчанию всегда светлые; системные меню трея
//! (Проводник) следуют теме панели задач. Тёмное или светлое меню для процесса задаёт
//! `SetPreferredAppMode` из uxtheme (ординал 135, Windows 10 1903+), новое оформление
//! применяет `FlushMenuThemes` (136). Функции не документированы, но их параметры известны
//! и стабильны: так делают Блокнот, Windows Terminal, Notepad++.

use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{PCSTR, w};

/// `PreferredAppMode`: 2 — всегда тёмное, 3 — всегда светлое.
const FORCE_DARK: i32 = 2;
const FORCE_LIGHT: i32 = 3;
const ORD_SET_PREFERRED_APP_MODE: usize = 135;
const ORD_FLUSH_MENU_THEMES: usize = 136;

type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemes = unsafe extern "system" fn();

/// Окрасить меню под панель задач: светлая панель — светлое меню, тёмная — тёмное.
pub fn apply(taskbar_light: bool) {
    // SAFETY: uxtheme загружается системой и не выгружается; функции берутся по ординалу
    // (MAKEINTRESOURCE) и вызываются с сигнатурами, совпадающими с экспортом uxtheme.
    unsafe {
        let Ok(lib) = LoadLibraryW(w!("uxtheme.dll")) else {
            return;
        };
        let set = GetProcAddress(lib, PCSTR(ORD_SET_PREFERRED_APP_MODE as *const u8));
        let flush = GetProcAddress(lib, PCSTR(ORD_FLUSH_MENU_THEMES as *const u8));
        let (Some(set), Some(flush)) = (set, flush) else {
            tracing::debug!("uxtheme: нет SetPreferredAppMode — меню трея в теме по умолчанию");
            return;
        };
        let set: SetPreferredAppMode = std::mem::transmute(set);
        let flush: FlushMenuThemes = std::mem::transmute(flush);
        set(if taskbar_light {
            FORCE_LIGHT
        } else {
            FORCE_DARK
        });
        flush();
    }
}

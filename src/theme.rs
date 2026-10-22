// SPDX-License-Identifier: GPL-3.0-or-later
//! Системная тема, акцентный цвет и шрифты.

use std::path::Path;

use crate::win::hkcu_read_dword;

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const DWM: &str = r"Software\Microsoft\Windows\DWM";

/// Акцент Windows по умолчанию (#0078D4).
const DEFAULT_ACCENT: (u8, u8, u8) = (0x00, 0x78, 0xd4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemTheme {
    /// Тёмная тема приложений (`AppsUseLightTheme = 0`).
    pub apps_dark: bool,
    /// Светлая панель задач (`SystemUsesLightTheme = 1`) — от неё зависит цвет иконки в трее.
    pub taskbar_light: bool,
    pub accent: (u8, u8, u8),
}

impl SystemTheme {
    pub fn read() -> Self {
        let apps_light = hkcu_read_dword(PERSONALIZE, "AppsUseLightTheme").unwrap_or(1);
        let taskbar_light = hkcu_read_dword(PERSONALIZE, "SystemUsesLightTheme").unwrap_or(0);
        // AccentColor хранится как 0xAABBGGRR.
        let accent = hkcu_read_dword(DWM, "AccentColor")
            .map(|v| (v as u8, (v >> 8) as u8, (v >> 16) as u8))
            .unwrap_or(DEFAULT_ACCENT);
        // Для проверки интерфейса без смены темы Windows: HUBLESS_THEME=light | dark.
        let apps_dark = match std::env::var("HUBLESS_THEME").as_deref() {
            Ok("light") => false,
            Ok("dark") => true,
            _ => apps_light == 0,
        };
        Self {
            apps_dark,
            taskbar_light: taskbar_light != 0,
            accent,
        }
    }
}

/// Системные шрифты: не вшиваем, а выбираем доступное семейство.
#[derive(Debug, Clone)]
pub struct Fonts {
    pub text: &'static str,
    pub icons: &'static str,
}

impl Fonts {
    pub fn detect() -> Self {
        let dir = std::env::var_os("WINDIR")
            .map(|w| Path::new(&w).join("Fonts"))
            .unwrap_or_else(|| r"C:\Windows\Fonts".into());
        let text = if dir.join("SegUIVar.ttf").exists() {
            "Segoe UI Variable Text"
        } else {
            "Segoe UI"
        };
        let icons = if dir.join("SegoeIcons.ttf").exists() {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        };
        Self { text, icons }
    }
}

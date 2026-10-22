// SPDX-License-Identifier: GPL-3.0-or-later
//! Interface language: English (the source) or Russian.
//!
//! `.slint` strings are translated through `@tr(...)` and the embedded `translations/ru/LC_MESSAGES/hubless.po`
//! (`slint::select_bundled_translation`). Strings from Rust (the tray tooltip, notifications) go through [`tr`]
//! by the same English string. The CLI and logs aren't translated.

use std::sync::atomic::{AtomicU8, Ordering};

use windows::Win32::Globalization::GetUserDefaultUILanguage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

/// The language choice in the settings (`language` in `config.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// By the Windows interface language.
    System,
    Fixed(Lang),
}

impl Choice {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "ru" => Self::Fixed(Lang::Ru),
            "en" => Self::Fixed(Lang::En),
            _ => Self::System,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "auto",
            Self::Fixed(Lang::Ru) => "ru",
            Self::Fixed(Lang::En) => "en",
        }
    }

    pub fn resolve(self) -> Lang {
        match self {
            Self::Fixed(l) => l,
            Self::System => system_lang(),
        }
    }
}

/// The Windows interface language: Russian by the primary language ID, otherwise English.
fn system_lang() -> Lang {
    const LANG_RUSSIAN: u16 = 0x19;
    // SAFETY: a function without arguments.
    let id = unsafe { GetUserDefaultUILanguage() };
    if id & 0x3FF == LANG_RUSSIAN {
        Lang::Ru
    } else {
        Lang::En
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current() -> Lang {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Lang::Ru,
        _ => Lang::En,
    }
}

/// Switches the language of Rust strings and Slint windows (call on the main thread after a window is created).
pub fn set(lang: Lang) {
    CURRENT.store(u8::from(lang == Lang::Ru), Ordering::Relaxed);
    apply_to_slint();
}

/// Switches the Slint translations to the current language. Slint requires at least one component
/// to exist by then, so it is also called after each window is created.
pub fn apply_to_slint() {
    let code = match current() {
        Lang::Ru => "ru",
        Lang::En => "",
    };
    if let Err(e) = slint::select_bundled_translation(code) {
        tracing::debug!("перевод Slint ({code}): {e}");
    }
}

/// Translates a string from Rust: the English original → the string in the current language.
pub fn tr(en: &'static str) -> &'static str {
    match current() {
        Lang::En => en,
        Lang::Ru => RU.iter().find(|(k, _)| *k == en).map_or(en, |(_, v)| v),
    }
}

/// Russian strings for [`tr`] (`.slint` strings are in `translations/ru/LC_MESSAGES/hubless.po`).
const RU: &[(&str, &str)] = &[
    ("No devices connected", "Нет подключённых устройств"),
    (
        "Hubless: notification test",
        "Hubless: проверка уведомлений",
    ),
    (
        "This is what the low battery warning looks like.",
        "Так выглядит предупреждение о низком заряде.",
    ),
    ("Battery below {}%", "Заряд ниже {}%"),
    ("Default", "Обычный"),
    (
        "Couldn't apply the setting",
        "Не удалось применить настройку",
    ),
    (
        "A profile with this name already exists",
        "Профиль с таким именем уже есть",
    ),
    ("Profile {}", "Профиль {}"),
    (
        "Used when no other profile matches",
        "Действует, когда не подходит ни один другой",
    ),
    (
        "No programs — switch to it by hand",
        "Без программ — включается вручную",
    ),
    ("Add a running program…", "Добавить запущенную программу…"),
    ("Saved: {}", "Сохранено: {}"),
    ("Couldn't create the report", "Не удалось создать отчёт"),
    ("Copied to the clipboard", "Скопировано в буфер обмена"),
    (
        "Couldn't copy to the clipboard",
        "Не удалось скопировать в буфер обмена",
    ),
    // Type names: the card's name if the device didn't report its own.
    ("Mouse", "Мышь"),
    ("Keyboard", "Клавиатура"),
    ("Headset", "Гарнитура"),
    ("{} Hz", "{} Гц"),
    ("Version {} is available", "Доступна версия {}"),
    (
        "You have the latest version",
        "Установлена последняя версия",
    ),
    (
        "Couldn't check for updates",
        "Не удалось проверить обновления",
    ),
    ("Log files deleted: {}", "Удалено файлов логов: {}"),
    ("Couldn't delete the logs", "Не удалось удалить логи"),
    ("{} kHz", "{} кГц"),
    // Equalizer presets: the built-in ones and the "the curve matches none" item.
    ("Flat", "Ровно"),
    ("Bass boost", "Больше басов"),
    ("Voice", "Голос"),
    ("FPS", "Шутеры"),
    ("Custom", "Своя"),
    ("No more than {} presets", "Не больше {} пресетов"),
    ("Couldn't save the preset", "Не удалось сохранить пресет"),
    // Notifications: charging finished, a new version (the check at startup).
    ("Fully charged", "Заряжено полностью"),
    ("Hubless {} is available", "Доступна Hubless {}"),
    (
        "Download it in Settings → General.",
        "Скачать — в «Настройки → Общие».",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_roundtrip() {
        for c in [
            Choice::System,
            Choice::Fixed(Lang::Ru),
            Choice::Fixed(Lang::En),
        ] {
            assert_eq!(Choice::parse(c.as_str()), c);
        }
        assert_eq!(Choice::parse("garbage"), Choice::System);
    }

    #[test]
    fn russian_table_has_unique_keys() {
        for (i, (k, _)) in RU.iter().enumerate() {
            assert!(RU[i + 1..].iter().all(|(k2, _)| k2 != k), "повтор: {k}");
        }
    }
}

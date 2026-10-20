// SPDX-License-Identifier: GPL-3.0-or-later
//! Язык интерфейса: английский (исходный) или русский.
//!
//! Строки `.slint` переводятся через `@tr(...)` и вшитый `translations/ru/LC_MESSAGES/hubless.po`
//! (`slint::select_bundled_translation`). Строки из Rust (меню трея, уведомления) — через [`tr`]
//! по той же английской строке. CLI и логи не переводятся.

use std::sync::atomic::{AtomicU8, Ordering};

use windows::Win32::Globalization::GetUserDefaultUILanguage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ru,
}

/// Выбор языка в настройках (`language` в `config.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// По языку интерфейса Windows.
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

/// Язык интерфейса Windows: русский — по первичному идентификатору языка, иначе английский.
fn system_lang() -> Lang {
    const LANG_RUSSIAN: u16 = 0x19;
    // SAFETY: функция без аргументов.
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

/// Сменить язык строк из Rust и окон Slint (вызывать на главном потоке после создания окна).
pub fn set(lang: Lang) {
    CURRENT.store(u8::from(lang == Lang::Ru), Ordering::Relaxed);
    apply_to_slint();
}

/// Переключить переводы Slint на текущий язык. Slint требует, чтобы к этому моменту уже был
/// создан хотя бы один компонент, — поэтому вызывается и после создания каждого окна.
pub fn apply_to_slint() {
    let code = match current() {
        Lang::Ru => "ru",
        Lang::En => "",
    };
    if let Err(e) = slint::select_bundled_translation(code) {
        tracing::debug!("перевод Slint ({code}): {e}");
    }
}

/// Перевод строки из Rust: английский оригинал → строка на текущем языке.
pub fn tr(en: &'static str) -> &'static str {
    match current() {
        Lang::En => en,
        Lang::Ru => RU.iter().find(|(k, _)| *k == en).map_or(en, |(_, v)| v),
    }
}

/// Русские строки для [`tr`] (строки `.slint` — в `translations/ru/LC_MESSAGES/hubless.po`).
const RU: &[(&str, &str)] = &[
    ("No devices connected", "Нет подключённых устройств"),
    ("Settings…", "Настройки…"),
    ("Start with Windows", "Автозапуск"),
    ("Open log folder", "Открыть папку логов"),
    ("Exit", "Выход"),
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

// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x8081` Per-key lighting v2 — цвет отдельных клавиш.
//!
//! По захвату G Hub (2026-10-20, PRO X TKL, `docs/devices/README.md`): fn5
//! `[первая зона, последняя зона, R, G, B]` красит диапазон зон, fn7 `[00]` применяет кадр;
//! номер зоны = код HID клавиши − 3 (A = 1 … Z = 26). Работает и через ресивер.
//! Запись из Hubless на железе ещё не проверена.

use crate::hidpp::Write;

pub const ID: u16 = 0x8081;
const SET_RANGE: u8 = 5;
const COMMIT: u8 = 7;

/// Зона клавиши по коду HID (usage page 0x07).
pub fn zone_of_usage(usage: u8) -> Option<u8> {
    usage.checked_sub(3).filter(|z| *z > 0)
}

/// Покрасить зоны `first..=last` одним цветом.
pub fn set_range(first: u8, last: u8, rgb: [u8; 3]) -> Write {
    Write {
        feature: ID,
        function: SET_RANGE,
        params: vec![first, last, rgb[0], rgb[1], rgb[2]],
        what: format!(
            "keys {first}–{last}: #{:02X}{:02X}{:02X}",
            rgb[0], rgb[1], rgb[2]
        ),
        short: false,
    }
}

/// Применить кадр (после всех `set_range`).
pub fn commit() -> Write {
    Write {
        feature: ID,
        function: COMMIT,
        params: vec![0],
        what: "per-key: apply".into(),
        short: false,
    }
}

/// Записи для набора зон одного цвета: соседние зоны склеиваются в диапазоны, в конце — commit.
pub fn paint(zones: &[u8], rgb: [u8; 3]) -> Vec<Write> {
    let mut zones = zones.to_vec();
    zones.sort_unstable();
    zones.dedup();
    let mut writes = Vec::new();
    let mut i = 0;
    while i < zones.len() {
        let first = zones[i];
        let mut last = first;
        while i + 1 < zones.len() && zones[i + 1] == last + 1 {
            i += 1;
            last = zones[i];
        }
        writes.push(set_range(first, last, rgb));
        i += 1;
    }
    if !writes.is_empty() {
        writes.push(commit());
    }
    writes
}

/// Код HID клавиши по имени (`a`, `f1`, `space`, `esc`, …; без учёта регистра).
pub fn usage_of(name: &str) -> Option<u8> {
    let n = name.to_ascii_lowercase();
    if let [c] = n.as_bytes() {
        match c {
            b'a'..=b'z' => return Some(0x04 + (c - b'a')),
            b'1'..=b'9' => return Some(0x1E + (c - b'1')),
            b'0' => return Some(0x27),
            _ => {}
        }
    }
    if let Some(f) = n.strip_prefix('f').and_then(|d| d.parse::<u8>().ok())
        && (1..=12).contains(&f)
    {
        return Some(0x3A + f - 1);
    }
    Some(match n.as_str() {
        "enter" => 0x28,
        "esc" | "escape" => 0x29,
        "backspace" => 0x2A,
        "tab" => 0x2B,
        "space" => 0x2C,
        "minus" | "-" => 0x2D,
        "equal" | "=" => 0x2E,
        "lbracket" | "[" => 0x2F,
        "rbracket" | "]" => 0x30,
        "backslash" | "\\" => 0x31,
        "semicolon" | ";" => 0x33,
        "quote" | "'" => 0x34,
        "grave" | "`" => 0x35,
        "comma" | "," => 0x36,
        "period" | "." => 0x37,
        "slash" | "/" => 0x38,
        "capslock" | "caps" => 0x39,
        "printscreen" => 0x46,
        "scrolllock" => 0x47,
        "pause" => 0x48,
        "insert" => 0x49,
        "home" => 0x4A,
        "pageup" => 0x4B,
        "delete" => 0x4C,
        "end" => 0x4D,
        "pagedown" => 0x4E,
        "right" => 0x4F,
        "left" => 0x50,
        "down" => 0x51,
        "up" => 0x52,
        "lctrl" => 0xE0,
        "lshift" => 0xE1,
        "lalt" => 0xE2,
        "lwin" => 0xE3,
        "rctrl" => 0xE4,
        "rshift" => 0xE5,
        "ralt" => 0xE6,
        "rwin" => 0xE7,
        _ => return None,
    })
}

/// Список клавиш из аргумента: `wasd` (каждая буква — клавиша), `w,a,s,d`, `f1,f2,space`.
pub fn parse_keys(arg: &str) -> Result<Vec<u8>, String> {
    let parts: Vec<String> = if arg.contains(',') {
        arg.split(',').map(|s| s.trim().to_owned()).collect()
    } else if arg.chars().all(|c| c.is_ascii_alphanumeric()) && usage_of(arg).is_none() {
        arg.chars().map(|c| c.to_string()).collect()
    } else {
        vec![arg.to_owned()]
    };
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .map(|p| {
            usage_of(p)
                .and_then(zone_of_usage)
                .ok_or_else(|| format!("unknown key: {p}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zones_follow_the_capture() {
        // Захват G Hub: T → зона 0x14; A = 1 … Z = 26.
        assert_eq!(usage_of("t").and_then(zone_of_usage), Some(0x14));
        assert_eq!(usage_of("A").and_then(zone_of_usage), Some(1));
        assert_eq!(usage_of("z").and_then(zone_of_usage), Some(26));
        assert_eq!(usage_of("f12"), Some(0x45));
        assert_eq!(parse_keys("wasd").unwrap(), vec![23, 1, 19, 4]);
        assert_eq!(parse_keys("f1,space").unwrap(), vec![0x37, 0x29]);
        assert!(parse_keys("w,nope").is_err());
    }

    #[test]
    fn neighbours_merge_into_ranges() {
        let w = paint(&[4, 1, 2, 3, 19, 20], [0xff, 0, 0]);
        let params: Vec<_> = w.iter().map(|w| (w.function, w.params.clone())).collect();
        assert_eq!(
            params,
            vec![
                (5, vec![1, 4, 0xff, 0, 0]),
                (5, vec![19, 20, 0xff, 0, 0]),
                (7, vec![0]),
            ]
        );
    }
}

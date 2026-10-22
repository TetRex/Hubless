// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyboard layouts for the key colour editor: TKL ANSI and TKL ISO (the user picks one, since there
//! is one `0x8081` zone map for both layouts, and the layout code from `0x4540` isn't decoded).
//! ISO differs by its tall Enter, the `0x32` key to its left (zone 47) and the `0x64` key between
//! the short left Shift and Z (zone 97); ISO has no `\` key above Enter (`0x31`).
//!
//! Coordinates are in "units" (the width of a normal key); `y` is the row number, and the F row is set
//! apart by a quarter of a unit. A key has a name for `devices.toml` (`[keyboard.keys]`, the same that
//! `hubless keyboard keys` accepts), a label for the key and an HID usage from which the `0x8081` zone is computed.
//! The labels are as printed on the keys and aren't translated.

use crate::hidpp::features::per_key;

/// Layout width and height, units.
pub const WIDTH: f32 = 18.25;
pub const HEIGHT: f32 = 6.25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    pub name: &'static str,
    pub label: &'static str,
    pub usage: u8,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    /// Height in rows (2 for the ISO Enter).
    pub h: f32,
}

impl Key {
    /// The `0x8081` zone.
    pub fn zone(&self) -> u8 {
        per_key::zone_of_usage(self.usage).expect("в раскладке только клавиши с зоной")
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        (self.x..self.x + self.w).contains(&x) && (self.y..self.y + self.h).contains(&y)
    }
}

const fn k(name: &'static str, label: &'static str, usage: u8, x: f32, y: f32, w: f32) -> Key {
    Key {
        name,
        label,
        usage,
        x,
        y,
        w,
        h: 1.0,
    }
}

/// The physical keyboard layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Ansi,
    Iso,
}

impl Layout {
    /// A value from `devices.toml` (`keyboard_layout`); anything unknown is ANSI.
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("iso") {
            Self::Iso
        } else {
            Self::Ansi
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ansi => "ansi",
            Self::Iso => "iso",
        }
    }

    /// All keys of the layout.
    pub fn keys(self) -> impl Iterator<Item = &'static Key> {
        let own = match self {
            Self::Ansi => ANSI_ONLY,
            Self::Iso => ISO_ONLY,
        };
        COMMON.iter().copied().flatten().chain(own)
    }
}

const R0: f32 = 0.0;
const R1: f32 = 1.25;
const R2: f32 = 2.25;
const R3: f32 = 3.25;
const R4: f32 = 4.25;
const R5: f32 = 5.25;
/// The navigation and arrow cluster.
const NAV: f32 = 15.25;

/// Keys shared by ANSI and ISO, by row.
const COMMON: &[&[Key]] = &[FN_ROW, NUMBER_ROW, TOP_ROW, HOME_ROW, SHIFT_ROW, BOTTOM_ROW];

const FN_ROW: &[Key] = &[
    k("esc", "Esc", 0x29, 0.0, R0, 1.0),
    k("f1", "F1", 0x3A, 2.0, R0, 1.0),
    k("f2", "F2", 0x3B, 3.0, R0, 1.0),
    k("f3", "F3", 0x3C, 4.0, R0, 1.0),
    k("f4", "F4", 0x3D, 5.0, R0, 1.0),
    k("f5", "F5", 0x3E, 6.5, R0, 1.0),
    k("f6", "F6", 0x3F, 7.5, R0, 1.0),
    k("f7", "F7", 0x40, 8.5, R0, 1.0),
    k("f8", "F8", 0x41, 9.5, R0, 1.0),
    k("f9", "F9", 0x42, 11.0, R0, 1.0),
    k("f10", "F10", 0x43, 12.0, R0, 1.0),
    k("f11", "F11", 0x44, 13.0, R0, 1.0),
    k("f12", "F12", 0x45, 14.0, R0, 1.0),
    k("printscreen", "PrtSc", 0x46, NAV, R0, 1.0),
    k("scrolllock", "ScrLk", 0x47, NAV + 1.0, R0, 1.0),
    k("pause", "Pause", 0x48, NAV + 2.0, R0, 1.0),
];

const NUMBER_ROW: &[Key] = &[
    k("grave", "`", 0x35, 0.0, R1, 1.0),
    k("1", "1", 0x1E, 1.0, R1, 1.0),
    k("2", "2", 0x1F, 2.0, R1, 1.0),
    k("3", "3", 0x20, 3.0, R1, 1.0),
    k("4", "4", 0x21, 4.0, R1, 1.0),
    k("5", "5", 0x22, 5.0, R1, 1.0),
    k("6", "6", 0x23, 6.0, R1, 1.0),
    k("7", "7", 0x24, 7.0, R1, 1.0),
    k("8", "8", 0x25, 8.0, R1, 1.0),
    k("9", "9", 0x26, 9.0, R1, 1.0),
    k("0", "0", 0x27, 10.0, R1, 1.0),
    k("minus", "-", 0x2D, 11.0, R1, 1.0),
    k("equal", "=", 0x2E, 12.0, R1, 1.0),
    k("backspace", "Backspace", 0x2A, 13.0, R1, 2.0),
    k("insert", "Ins", 0x49, NAV, R1, 1.0),
    k("home", "Home", 0x4A, NAV + 1.0, R1, 1.0),
    k("pageup", "PgUp", 0x4B, NAV + 2.0, R1, 1.0),
];

/// The top letter row without the key at its right edge (`\` on ANSI, the top of Enter on ISO).
const TOP_ROW: &[Key] = &[
    k("tab", "Tab", 0x2B, 0.0, R2, 1.5),
    k("q", "Q", 0x14, 1.5, R2, 1.0),
    k("w", "W", 0x1A, 2.5, R2, 1.0),
    k("e", "E", 0x08, 3.5, R2, 1.0),
    k("r", "R", 0x15, 4.5, R2, 1.0),
    k("t", "T", 0x17, 5.5, R2, 1.0),
    k("y", "Y", 0x1C, 6.5, R2, 1.0),
    k("u", "U", 0x18, 7.5, R2, 1.0),
    k("i", "I", 0x0C, 8.5, R2, 1.0),
    k("o", "O", 0x12, 9.5, R2, 1.0),
    k("p", "P", 0x13, 10.5, R2, 1.0),
    k("lbracket", "[", 0x2F, 11.5, R2, 1.0),
    k("rbracket", "]", 0x30, 12.5, R2, 1.0),
    k("delete", "Del", 0x4C, NAV, R2, 1.0),
    k("end", "End", 0x4D, NAV + 1.0, R2, 1.0),
    k("pagedown", "PgDn", 0x4E, NAV + 2.0, R2, 1.0),
];

/// The home row without Enter (and without the ISO key to its left).
const HOME_ROW: &[Key] = &[
    k("capslock", "Caps", 0x39, 0.0, R3, 1.75),
    k("a", "A", 0x04, 1.75, R3, 1.0),
    k("s", "S", 0x16, 2.75, R3, 1.0),
    k("d", "D", 0x07, 3.75, R3, 1.0),
    k("f", "F", 0x09, 4.75, R3, 1.0),
    k("g", "G", 0x0A, 5.75, R3, 1.0),
    k("h", "H", 0x0B, 6.75, R3, 1.0),
    k("j", "J", 0x0D, 7.75, R3, 1.0),
    k("k", "K", 0x0E, 8.75, R3, 1.0),
    k("l", "L", 0x0F, 9.75, R3, 1.0),
    k("semicolon", ";", 0x33, 10.75, R3, 1.0),
    k("quote", "'", 0x34, 11.75, R3, 1.0),
];

/// The bottom letter row without the left Shift (on ISO it's shorter, with one more key next to it).
const SHIFT_ROW: &[Key] = &[
    k("z", "Z", 0x1D, 2.25, R4, 1.0),
    k("x", "X", 0x1B, 3.25, R4, 1.0),
    k("c", "C", 0x06, 4.25, R4, 1.0),
    k("v", "V", 0x19, 5.25, R4, 1.0),
    k("b", "B", 0x05, 6.25, R4, 1.0),
    k("n", "N", 0x11, 7.25, R4, 1.0),
    k("m", "M", 0x10, 8.25, R4, 1.0),
    k("comma", ",", 0x36, 9.25, R4, 1.0),
    k("period", ".", 0x37, 10.25, R4, 1.0),
    k("slash", "/", 0x38, 11.25, R4, 1.0),
    k("rshift", "Shift", 0xE5, 12.25, R4, 2.75),
    k("up", "↑", 0x52, NAV + 1.0, R4, 1.0),
];

const BOTTOM_ROW: &[Key] = &[
    k("lctrl", "Ctrl", 0xE0, 0.0, R5, 1.25),
    k("lwin", "Win", 0xE3, 1.25, R5, 1.25),
    k("lalt", "Alt", 0xE2, 2.5, R5, 1.25),
    k("space", "", 0x2C, 3.75, R5, 6.25),
    k("ralt", "Alt", 0xE6, 10.0, R5, 1.25),
    k("rwin", "Fn", 0xE7, 11.25, R5, 1.25),
    k("menu", "Menu", 0x65, 12.5, R5, 1.25),
    k("rctrl", "Ctrl", 0xE4, 13.75, R5, 1.25),
    k("left", "←", 0x50, NAV, R5, 1.0),
    k("down", "↓", 0x51, NAV + 1.0, R5, 1.0),
    k("right", "→", 0x4F, NAV + 2.0, R5, 1.0),
];

const ANSI_ONLY: &[Key] = &[
    k("backslash", "\\", 0x31, 13.5, R2, 1.5),
    k("enter", "Enter", 0x28, 12.75, R3, 2.25),
    k("lshift", "Shift", 0xE1, 0.0, R4, 2.25),
];

const ISO_ONLY: &[Key] = &[
    // Tall Enter: the top part (13.5–15 units) is simplified to the width of the bottom part.
    Key {
        h: 2.0,
        ..k("enter", "Enter", 0x28, 13.75, R2, 1.25)
    },
    k("nonushash", "\\", 0x32, 12.75, R3, 1.0),
    k("lshift", "Shift", 0xE1, 0.0, R4, 1.25),
    k("nonusbackslash", "\\", 0x64, 1.25, R4, 1.0),
];

/// Keys of both layouts (for lookups by name and zone).
fn all() -> impl Iterator<Item = &'static Key> {
    COMMON
        .iter()
        .copied()
        .flatten()
        .chain(ANSI_ONLY)
        .chain(ISO_ONLY)
}

/// The keys of the layout that the keyboard has (by the `0x8081` zone map).
pub fn keys_for(layout: Layout, zones: &[u8]) -> Vec<&'static Key> {
    layout
        .keys()
        .filter(|k| zones.contains(&k.zone()))
        .collect()
}

/// The key under a point (`x`, `y` are units from the layout's top-left corner).
pub fn key_at(keys: &[&'static Key], x: f32, y: f32) -> Option<&'static Key> {
    keys.iter().copied().find(|k| k.contains(x, y))
}

/// A key by its name from `devices.toml` (or from `hubless keyboard keys`).
pub fn by_name(name: &str) -> Option<&'static Key> {
    let name = name.to_ascii_lowercase();
    all().find(|k| k.name == name)
}

/// A key by its `0x8081` zone.
pub fn by_zone(zone: u8) -> Option<&'static Key> {
    all().find(|k| k.zone() == zone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_the_cli_and_zones_are_unique() {
        for (layout, count) in [(Layout::Ansi, 87), (Layout::Iso, 88)] {
            let keys: Vec<&Key> = layout.keys().collect();
            for key in &keys {
                assert_eq!(
                    per_key::usage_of(key.name),
                    Some(key.usage),
                    "имя {} понимает CLI",
                    key.name
                );
                assert!(
                    key.x + key.w <= WIDTH && key.y + key.h <= HEIGHT,
                    "{}",
                    key.name
                );
            }
            let mut zones: Vec<u8> = keys.iter().map(|k| k.zone()).collect();
            zones.sort_unstable();
            zones.dedup();
            assert_eq!(
                zones.len(),
                keys.len(),
                "{layout:?}: у каждой клавиши своя зона"
            );
            assert_eq!(keys.len(), count, "{layout:?}");
        }
    }

    #[test]
    fn keys_do_not_overlap_and_hit_testing_finds_them() {
        for layout in [Layout::Ansi, Layout::Iso] {
            let keys: Vec<&Key> = layout.keys().collect();
            for key in &keys {
                let (cx, cy) = (key.x + key.w / 2.0, key.y + key.h / 2.0);
                assert_eq!(
                    key_at(&keys, cx, cy).map(|k| k.name),
                    Some(key.name),
                    "{layout:?}"
                );
            }
            assert_eq!(key_at(&keys, 1.5, 0.5), None, "промежуток после Esc");
        }
        let iso: Vec<&Key> = Layout::Iso.keys().collect();
        assert_eq!(key_at(&iso, 14.5, 3.5).map(|k| k.name), Some("enter"));
        assert_eq!(key_at(&iso, 13.0, 3.5).map(Key::zone), Some(47));
        assert_eq!(by_name("W").map(Key::zone), Some(23));
        assert_eq!(by_zone(104).map(|k| k.name), Some("lctrl"));
        assert_eq!(by_zone(97).map(|k| k.name), Some("nonusbackslash"));
        assert_eq!(Layout::parse("ISO"), Layout::Iso);
        assert_eq!(Layout::parse("x"), Layout::Ansi);
    }

    #[test]
    fn missing_zones_hide_keys() {
        let zones: Vec<u8> = Layout::Ansi
            .keys()
            .map(Key::zone)
            .filter(|&z| z != 98)
            .collect();
        let keys = keys_for(Layout::Ansi, &zones);
        assert_eq!(keys.len(), 86);
        assert!(keys.iter().all(|k| k.name != "menu"));
    }
}

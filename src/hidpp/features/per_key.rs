// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x8081` Per-key lighting v2: the colour of single keys.
//!
//! From a G Hub capture (2026-10-20, PRO X TKL, `docs/devices/README.md`): fn5
//! `[first zone, last zone, R, G, B]` paints a range of zones, fn7 `[00]` applies the frame;
//! zone number = the key's HID usage − 3 (A = 1 … Z = 26), and 104–111 for the modifiers.
//! Also works through the receiver; writing from Hubless verified on hardware 2026-10-22.

use std::collections::BTreeMap;

use crate::hidpp::{Device, Error, Write};

pub const ID: u16 = 0x8081;
const GET_INFO: u8 = 0;
const SET_RANGE: u8 = 5;
const COMMIT: u8 = 7;

/// The zones the keyboard has. As in Solaar: fn0 `[00, page]` for pages 0–2; after two bytes the reply
/// is a bitmap of zones (bit `i` is zone `i`).
pub fn supported_zones(dev: &Device) -> Result<Vec<u8>, Error> {
    let mut bitmap = Vec::new();
    for page in 0..3u8 {
        let r = dev.call(ID, GET_INFO, &[0x00, page])?;
        bitmap.extend_from_slice(r.params().get(2..).unwrap_or_default());
    }
    Ok(zones_in_bitmap(&bitmap))
}

fn zones_in_bitmap(bitmap: &[u8]) -> Vec<u8> {
    (1..=u8::MAX)
        .filter(|&z| {
            bitmap
                .get(usize::from(z / 8))
                .is_some_and(|b| b >> (z % 8) & 1 == 1)
        })
        .collect()
}

/// The key's zone from its HID usage (usage page 0x07): normal keys are usage − 3 (from the G Hub capture),
/// modifiers `0xE0`–`0xE7` are 104–111, as in Solaar. The PRO X TKL zone map (fn0, 2026-10-22)
/// confirms it: zones 104–111 exist, and "usage − 3" for the modifiers (221–228) doesn't.
pub fn zone_of_usage(usage: u8) -> Option<u8> {
    match usage {
        0x04..=0x65 => Some(usage - 3),
        0xE0..=0xE7 => Some(usage - 0x78),
        _ => None,
    }
}

/// Paints zones `first..=last` with one colour.
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

/// Applies the frame (after all `set_range` writes).
pub fn commit() -> Write {
    Write {
        feature: ID,
        function: COMMIT,
        params: vec![0],
        what: "per-key: apply".into(),
        short: false,
    }
}

/// Writes for a set of zones in one colour: adjacent zones merge into ranges, with a commit at the end.
pub fn paint(zones: &[u8], rgb: [u8; 3]) -> Vec<Write> {
    let keys: Vec<(u8, [u8; 3])> = zones.iter().map(|&z| (z, rgb)).collect();
    frame(&keys)
}

/// Writes for one frame: zones are grouped by colour (a repeated zone gets its last colour),
/// adjacent zones of one colour merge into ranges, with one commit at the end.
pub fn frame(keys: &[(u8, [u8; 3])]) -> Vec<Write> {
    let zones: BTreeMap<u8, [u8; 3]> = keys.iter().copied().collect();
    let mut by_color: BTreeMap<[u8; 3], Vec<u8>> = BTreeMap::new();
    for (zone, rgb) in zones {
        by_color.entry(rgb).or_default().push(zone);
    }
    let mut writes = Vec::new();
    for (rgb, zones) in by_color {
        // The zones are already ascending (from the BTreeMap).
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
    }
    if !writes.is_empty() {
        writes.push(commit());
    }
    writes
}

/// A key's HID usage by name (`a`, `f1`, `space`, `esc`, …; case-insensitive).
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
        // ISO: the key to the left of the tall Enter, and the one between the short left Shift and Z.
        "nonushash" | "hash" | "#" => 0x32,
        "nonusbackslash" => 0x64,
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
        "menu" | "application" => 0x65,
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

/// The list of keys from an argument: `wasd` (each letter is a key), `w,a,s,d`, `f1,f2,space`.
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
        // G Hub capture: T → zone 0x14; A = 1 … Z = 26.
        assert_eq!(usage_of("t").and_then(zone_of_usage), Some(0x14));
        assert_eq!(usage_of("A").and_then(zone_of_usage), Some(1));
        assert_eq!(usage_of("z").and_then(zone_of_usage), Some(26));
        assert_eq!(usage_of("f12"), Some(0x45));
        assert_eq!(parse_keys("wasd").unwrap(), vec![23, 1, 19, 4]);
        assert_eq!(parse_keys("f1,space").unwrap(), vec![0x37, 0x29]);
        assert!(parse_keys("w,nope").is_err());
        // Modifiers are 104–111, the Menu key is 98 (PRO X TKL zone map).
        assert_eq!(parse_keys("lctrl,rwin,menu").unwrap(), vec![104, 111, 98]);
    }

    #[test]
    fn zone_bitmap_is_read_like_solaar() {
        // Zones 1, 8 and 104: bit 1 of byte 0, bit 0 of byte 1, bit 0 of byte 13.
        let mut bitmap = vec![0u8; 42];
        bitmap[0] = 0b10;
        bitmap[1] = 0b1;
        bitmap[13] = 0b1;
        assert_eq!(zones_in_bitmap(&bitmap), vec![1, 8, 104]);
    }

    #[test]
    fn frame_groups_colours_and_commits_once() {
        let red = [0xff, 0, 0];
        let blue = [0, 0, 0xff];
        let w = frame(&[(3, red), (1, blue), (2, red), (1, red), (9, blue)]);
        let params: Vec<_> = w.iter().map(|w| (w.function, w.params.clone())).collect();
        assert_eq!(
            params,
            vec![
                (5, vec![9, 9, 0, 0, 0xff]),
                (5, vec![1, 3, 0xff, 0, 0]),
                (7, vec![0]),
            ]
        );
        assert!(frame(&[]).is_empty());
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

// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x2202` Extended Adjustable DPI (функции и раскладка — по Solaar `ExtendedAdjustableDpi`).
//!
//! - fn1 `getSensorCapabilities(sensor)` → байт 2: бит 0 — раздельный Y, бит 1 — lift-off;
//! - fn2 `getSensorDpiRanges(sensor, direction, page)` → `[sensor, direction, page, u16...]`,
//!   значение `0xE000 | step` означает диапазон «от предыдущего с шагом step до следующего»,
//!   `0x0000` — конец списка;
//! - fn5 `getSensorDpiParameters(sensor)` → `[sensor, x(2), x_default(2), y(2), y_default(2), lod]`;
//! - fn6 `setSensorDpiParameters(sensor, x(2), y(2), lod)` — запись.

use crate::hidpp::{Device, Error, Write};

pub const ID: u16 = 0x2202;
/// Старая `0x2201` Adjustable DPI (G903 и др.; спецификация Logitech и Solaar `AdjustableDpi`):
/// - fn1 `getSensorDpiList(sensor)` → `[sensor, u16 × 7]`, диапазоны `0xE000 | step` как у `0x2202`;
/// - fn2 `getSensorDpi(sensor)` → `[sensor, dpi(2), default(2)]`;
/// - fn3 `setSensorDpi(sensor, dpi(2))`. Не проверено на железе — только в экспериментальном режиме.
pub const OLD_ID: u16 = 0x2201;
const OLD_GET_LIST: u8 = 1;
const OLD_GET_DPI: u8 = 2;
const OLD_SET_DPI: u8 = 3;
const GET_CAPABILITIES: u8 = 1;
const GET_DPI_RANGES: u8 = 2;
const GET_PARAMETERS: u8 = 5;
const SET_PARAMETERS: u8 = 6;

const SENSOR: u8 = 0;
const MAX_PAGES: u8 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub separate_y: bool,
    pub lift_off: bool,
}

/// Lift-off distance (значения по Solaar: LOW/MEDIUM/HIGH).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiftOff {
    Low,
    Medium,
    High,
    Unknown(u8),
}

impl LiftOff {
    pub fn to_byte(self) -> u8 {
        match self {
            Self::Low => 0,
            Self::Medium => 1,
            Self::High => 2,
            Self::Unknown(b) => b,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Low,
            1 => Self::Medium,
            2 => Self::High,
            other => Self::Unknown(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parameters {
    pub x: u16,
    pub x_default: u16,
    pub y: u16,
    pub y_default: u16,
    pub lift_off: LiftOff,
}

pub fn capabilities(dev: &Device) -> Result<Capabilities, Error> {
    let r = dev.call(ID, GET_CAPABILITIES, &[SENSOR])?;
    let flags = r.params()[2];
    Ok(Capabilities {
        separate_y: flags & 0x01 != 0,
        lift_off: flags & 0x02 != 0,
    })
}

/// Все допустимые значения DPI по оси X (`y = false`) или Y.
pub fn supported(dev: &Device, y: bool) -> Result<Vec<u16>, Error> {
    // Страница несёт 13 байт данных (нечётно), поэтому слова разбираются по склеенному потоку.
    let mut bytes = Vec::new();
    for page in 0..MAX_PAGES {
        let r = dev.call(ID, GET_DPI_RANGES, &[SENSOR, u8::from(y), page])?;
        bytes.extend_from_slice(&r.params()[3..]);
        if words_until_end(&bytes).1 {
            break;
        }
    }
    expand_ranges(&words_until_end(&bytes).0).ok_or(Error::Hidpp20(0xFF))
}

/// Слова big-endian до первого нулевого; второй элемент — найден ли конец списка.
fn words_until_end(bytes: &[u8]) -> (Vec<u16>, bool) {
    let mut words = Vec::new();
    for pair in bytes.as_chunks::<2>().0 {
        let w = u16::from_be_bytes(*pair);
        if w == 0 {
            return (words, true);
        }
        words.push(w);
    }
    (words, false)
}

/// Разворачивает список с диапазонами `0xE000 | step` в явные значения.
pub fn expand_ranges(words: &[u16]) -> Option<Vec<u16>> {
    let mut list: Vec<u16> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w >> 13 == 0b111 {
            let step = w & 0x1FFF;
            let last = *words.get(i + 1)?;
            let from = *list.last()?;
            if step == 0 || last <= from {
                return None;
            }
            let mut v = from + step;
            while v <= last {
                list.push(v);
                v += step;
            }
            i += 2;
        } else {
            list.push(w);
            i += 1;
        }
    }
    Some(list)
}

pub fn parameters(dev: &Device) -> Result<Parameters, Error> {
    let r = dev.call(ID, GET_PARAMETERS, &[SENSOR])?;
    let p = r.params();
    let be = |i: usize| u16::from_be_bytes([p[i], p[i + 1]]);
    Ok(Parameters {
        x: be(1),
        x_default: be(3),
        y: be(5),
        y_default: be(7),
        lift_off: LiftOff::from_byte(p[9]),
    })
}

/// Текущий DPI по старой фиче `0x2201`.
pub fn old_current(dev: &Device) -> Result<u16, Error> {
    let r = dev.call(OLD_ID, OLD_GET_DPI, &[SENSOR])?;
    let p = r.params();
    Ok(u16::from_be_bytes([p[1], p[2]]))
}

/// Допустимые значения DPI по `0x2201`.
pub fn old_supported(dev: &Device) -> Result<Vec<u16>, Error> {
    let r = dev.call(OLD_ID, OLD_GET_LIST, &[SENSOR])?;
    let p = r.params();
    let (words, _) = words_until_end(&p[1..15.min(p.len())]);
    expand_ranges(&words)
        .filter(|l| !l.is_empty())
        .ok_or(Error::Hidpp20(0xFF))
}

/// Запись DPI по `0x2201`.
pub fn old_set(dpi: u16) -> Write {
    let [hi, lo] = dpi.to_be_bytes();
    Write {
        feature: OLD_ID,
        function: OLD_SET_DPI,
        params: vec![SENSOR, hi, lo],
        what: format!("DPI {dpi} (0x2201)"),
        short: false,
    }
}

/// Записать DPI по X/Y и lift-off.
pub fn set_parameters(x: u16, y: u16, lift_off: LiftOff) -> Write {
    let [x_hi, x_lo] = x.to_be_bytes();
    let [y_hi, y_lo] = y.to_be_bytes();
    Write {
        feature: ID,
        function: SET_PARAMETERS,
        params: vec![SENSOR, x_hi, x_lo, y_hi, y_lo, lift_off.to_byte()],
        what: format!("DPI X={x} Y={y}, lift-off {lift_off:?}"),
        short: false,
    }
}

#[cfg(test)]
mod tests {
    use super::{expand_ranges, words_until_end};

    #[test]
    fn words_span_odd_pages() {
        // 13 + 13 байт: слово 0x01F4 разрезано между страницами.
        let mut bytes = vec![
            0x00, 0x64, 0xE0, 0x01, 0x00, 0xC8, 0xE0, 0x02, 0x01, 0xF4, 0xE0, 0x05, 0x01,
        ];
        bytes.extend_from_slice(&[0xF4, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let (words, end) = words_until_end(&bytes);
        assert!(end);
        assert_eq!(words, vec![100, 0xE001, 200, 0xE002, 500, 0xE005, 500]);
    }

    #[test]
    fn expands_step_ranges() {
        assert_eq!(
            expand_ranges(&[100, 0xE000 | 50, 300]),
            Some(vec![100, 150, 200, 250, 300])
        );
        assert_eq!(expand_ranges(&[400, 800, 1600]), Some(vec![400, 800, 1600]));
        assert_eq!(expand_ranges(&[0xE000 | 50, 300]), None);
    }
}

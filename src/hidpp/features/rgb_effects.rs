// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x8071` RGB Effects (раскладка — по Solaar `RGBEffectsInfo`, `RGBControl`, `rgb_power`).
//!
//! fn0 `getInfo(zone, effect, 0)`:
//! - `(0xFF, 0xFF)` → `[_, _, zone_count, _(2), caps(2)]`;
//! - `(zone, 0xFF)` → `[zone, _, location(2), effect_count]`;
//! - `(zone, effect)` → `[zone, effect, id(2), caps(2), period(2)]`.
//!
//! Смена эффекта требует перехвата управления подсветкой (fn5), после чего гасить подсветку
//! при простое должно приложение — см. раздел «Программное управление» ниже.

use crate::hidpp::{Device, Error, Write};

pub const ID: u16 = 0x8071;
const GET_INFO: u8 = 0;
const ALL: u8 = 0xFF;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect {
    pub index: u8,
    pub id: u16,
    pub caps: u16,
    pub period: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    pub index: u8,
    pub location: u16,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub caps: u16,
    pub zones: Vec<Zone>,
}

/// Имя эффекта по ID (справочник — Solaar `LEDEffects`).
pub fn effect_name(id: u16) -> Option<&'static str> {
    Some(match id {
        0x00 => "Disabled",
        0x01 => "Static",
        0x02 => "Pulse",
        0x03 | 0x15 => "Cycle",
        0x04 | 0x16 => "Wave",
        0x08 => "Boot",
        0x09 => "Demo",
        0x0A => "Breathe",
        0x0B | 0x17 => "Ripple",
        0x0E => "Decomposition",
        0x0F => "Signature1",
        0x10 => "Signature2",
        0x80 => "Dim",
        _ => return None,
    })
}

/// Расположение зоны (справочник — Solaar `LEDZoneLocations`).
pub fn location_name(location: u16) -> &'static str {
    match location {
        0x01 => "Primary",
        0x02 => "Logo",
        0x03 => "Left Side",
        0x04 => "Right Side",
        0x05 => "Combined",
        0x06..=0x0B => "Primary N",
        _ => "Unknown",
    }
}

pub fn info(dev: &Device) -> Result<Info, Error> {
    let r = dev.call(ID, GET_INFO, &[ALL, ALL, 0])?;
    let p = r.params();
    let zone_count = p[2];
    let caps = u16::from_be_bytes([p[5], p[6]]);
    let mut zones = Vec::with_capacity(zone_count as usize);
    for z in 0..zone_count {
        let r = dev.call(ID, GET_INFO, &[z, ALL, 0])?;
        let p = r.params();
        let location = u16::from_be_bytes([p[2], p[3]]);
        let effect_count = p[4];
        let mut effects = Vec::with_capacity(effect_count as usize);
        for e in 0..effect_count {
            let r = dev.call(ID, GET_INFO, &[z, e, 0])?;
            let p = r.params();
            effects.push(Effect {
                index: p[1],
                id: u16::from_be_bytes([p[2], p[3]]),
                caps: u16::from_be_bytes([p[4], p[5]]),
                period: u16::from_be_bytes([p[6], p[7]]),
            });
        }
        zones.push(Zone {
            index: z,
            location,
            effects,
        });
    }
    Ok(Info { caps, zones })
}

// --- Программное управление подсветкой (по Solaar `RGBControl` и `rgb_power`) ---
//
// fn1 `setZoneEffect(zone, effect_index, params[10], persist=1)`;
// fn1 событие `onUserActivity`: params[0] = 0 — простой, иначе — активность;
// fn5 `SW control`: `[00]` — чтение, `[01, mode, flags]` — запись
//     (mode 3 — управляет приложение; flags 4 — клавиатура следит за простоем, 2 — за активностью);
// fn7 таймеры: `[00]` — чтение `[_, _, _, idle(2), sleep(2)]`, секунды;
// fn8 питание подсветки: `[01, mode, 00]`, mode 3 — выключить (сон), 1 — включить.

const SET_ZONE_EFFECT: u8 = 1;
const SW_CONTROL: u8 = 5;
const TIMERS: u8 = 7;
const POWER: u8 = 8;

/// ID эффектов, которые умеем задавать.
pub const EFFECT_DISABLED: u16 = 0x00;
pub const EFFECT_STATIC: u16 = 0x01;

const SW_ACTIVE: [u8; 3] = [0x01, 0x03, 0x04];
const SW_IDLE: [u8; 3] = [0x01, 0x03, 0x02];
const SW_RELEASE: [u8; 3] = [0x01, 0x00, 0x00];

/// Текущий режим программного управления: сырые байты ответа.
pub fn sw_control(dev: &Device) -> Result<Vec<u8>, Error> {
    Ok(dev.call(ID, SW_CONTROL, &[0x00])?.params()[..4].to_vec())
}

/// Таймеры простоя и сна подсветки из прошивки, секунды.
pub fn timers(dev: &Device) -> Result<(u16, u16), Error> {
    let r = dev.call(ID, TIMERS, &[0x00])?;
    let p = r.params();
    Ok((
        u16::from_be_bytes([p[3], p[4]]),
        u16::from_be_bytes([p[5], p[6]]),
    ))
}

fn write(function: u8, params: &[u8], what: impl Into<String>) -> Write {
    Write {
        feature: ID,
        function,
        params: params.to_vec(),
        what: what.into(),
        short: false,
    }
}

/// Забрать управление подсветкой (клавиатура продолжает сообщать о простое).
pub fn claim() -> Write {
    write(SW_CONTROL, &SW_ACTIVE, "lighting control: Hubless")
}

/// Режим простоя: клавиатура сообщает о возобновлении активности.
pub fn idle_mode() -> Write {
    write(SW_CONTROL, &SW_IDLE, "lighting: idle mode")
}

/// Вернуть управление подсветкой клавиатуре.
pub fn release() -> Write {
    write(SW_CONTROL, &SW_RELEASE, "lighting control: keyboard")
}

pub fn power_off() -> Write {
    write(POWER, &[0x01, 0x03, 0x00], "lighting: sleep")
}

pub fn power_on() -> Write {
    write(POWER, &[0x01, 0x01, 0x00], "lighting: wake")
}

/// Индекс эффекта с данным ID в зоне.
pub fn effect_index(zone: &Zone, effect_id: u16) -> Option<u8> {
    zone.effects
        .iter()
        .find(|e| e.id == effect_id)
        .map(|e| e.index)
}

/// Статичный цвет зоны (раскладка Static: цвет в байтах 0..3, ramp — 3).
pub fn static_color(zone: &Zone, rgb: [u8; 3]) -> Option<Write> {
    let index = effect_index(zone, EFFECT_STATIC)?;
    let mut params = vec![zone.index, index, rgb[0], rgb[1], rgb[2]];
    params.extend_from_slice(&[0; 7]);
    params.push(0x01);
    Some(write(
        SET_ZONE_EFFECT,
        &params,
        format!(
            "zone {}: static colour #{:02X}{:02X}{:02X}",
            zone.index, rgb[0], rgb[1], rgb[2]
        ),
    ))
}

/// Выключить подсветку зоны эффектом Disabled.
pub fn disabled(zone: &Zone) -> Option<Write> {
    let index = effect_index(zone, EFFECT_DISABLED)?;
    let mut params = vec![zone.index, index];
    params.extend_from_slice(&[0; 10]);
    params.push(0x01);
    Some(write(
        SET_ZONE_EFFECT,
        &params,
        format!("zone {}: off", zone.index),
    ))
}

// --- Встроенные анимированные эффекты ---
//
// Раскладка 10 байт параметров — по Solaar `LEDEffects` (hidpp20.py), числа big-endian.
// У эффектов две версии ID: старая и с насыщенностью (0x15/0x16/0x17); берём ту, что есть в зоне.
//
// | ID   | эффект  | раскладка                                                   |
// |------|---------|-------------------------------------------------------------|
// | 0x0A | Breathe | цвет 0..3, период 3..5, форма 5, интенсивность 6            |
// | 0x03 | Cycle   | период 5..7, интенсивность 7                                |
// | 0x15 | Cycle   | насыщенность 1, период 6..8, интенсивность 8                |
// | 0x04 | Wave    | период 6..8, направление 9                                  |
// | 0x16 | Wave    | насыщенность 1, период 6..8, интенсивность 8, направление 9 |
// | 0x0B | Ripple  | цвет 0..3, период 4..6                                      |
// | 0x17 | Ripple  | цвет 0..3, насыщенность 3, период 6..8                      |

pub const EFFECT_CYCLE: u16 = 0x03;
pub const EFFECT_WAVE: u16 = 0x04;
pub const EFFECT_BREATHE: u16 = 0x0A;
pub const EFFECT_RIPPLE: u16 = 0x0B;
pub const EFFECT_CYCLE_S: u16 = 0x15;
pub const EFFECT_WAVE_S: u16 = 0x16;
pub const EFFECT_RIPPLE_S: u16 = 0x17;

/// Период анимаций Breathe/Cycle/Wave, мс (диапазон Solaar).
pub const PERIOD_MS: std::ops::RangeInclusive<u16> = 1000..=20000;
/// Период Ripple — в своих единицах прошивки (диапазон и значение по умолчанию — Solaar).
pub const RIPPLE_PERIOD: std::ops::RangeInclusive<u16> = 2..=200;
pub const RIPPLE_PERIOD_DEFAULT: u16 = 20;
/// Насыщенность для версий с насыщенностью: полная, как по умолчанию в Solaar.
const SATURATION: u8 = 0xFF;

/// Направление волны (значения — Solaar `LedDirectionChoices`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Cycle = 0,
    Right = 1,
    Down = 2,
    CenterOut = 3,
    In = 4,
    Out = 5,
    Left = 6,
    Up = 7,
    CenterIn = 8,
}

impl Direction {
    pub const ALL: [Direction; 9] = [
        Self::Cycle,
        Self::Right,
        Self::Down,
        Self::CenterOut,
        Self::In,
        Self::Out,
        Self::Left,
        Self::Up,
        Self::CenterIn,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Cycle => "cycle",
            Self::Right => "right",
            Self::Down => "down",
            Self::CenterOut => "center-out",
            Self::In => "in",
            Self::Out => "out",
            Self::Left => "left",
            Self::Up => "up",
            Self::CenterIn => "center-in",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.name() == s)
    }
}

/// Первый из `ids`, который есть в зоне: (ID, индекс эффекта).
fn pick(zone: &Zone, ids: &[u16]) -> Option<(u16, u8)> {
    ids.iter()
        .find_map(|&id| effect_index(zone, id).map(|i| (id, i)))
}

fn zone_effect(zone: &Zone, index: u8, p: [u8; 10], what: String) -> Write {
    let mut params = vec![zone.index, index];
    params.extend_from_slice(&p);
    params.push(0x01);
    write(SET_ZONE_EFFECT, &params, what)
}

fn put_u16(p: &mut [u8; 10], at: usize, v: u16) {
    p[at..at + 2].copy_from_slice(&v.to_be_bytes());
}

fn color_text(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// Дыхание цветом. Форма волны 0 — по умолчанию (прошивка 0x8071 её фиксирует).
pub fn breathe(zone: &Zone, rgb: [u8; 3], period_ms: u16, intensity: u8) -> Option<Write> {
    let (_, index) = pick(zone, &[EFFECT_BREATHE])?;
    let mut p = [0; 10];
    p[..3].copy_from_slice(&rgb);
    put_u16(&mut p, 3, period_ms);
    p[6] = intensity;
    Some(zone_effect(
        zone,
        index,
        p,
        format!(
            "zone {}: breathe {}, period {period_ms} ms, intensity {intensity}",
            zone.index,
            color_text(rgb)
        ),
    ))
}

/// Смена цветов по кругу.
pub fn cycle(zone: &Zone, period_ms: u16, intensity: u8) -> Option<Write> {
    let (id, index) = pick(zone, &[EFFECT_CYCLE_S, EFFECT_CYCLE])?;
    let mut p = [0; 10];
    if id == EFFECT_CYCLE_S {
        p[1] = SATURATION;
        put_u16(&mut p, 6, period_ms);
        p[8] = intensity;
    } else {
        put_u16(&mut p, 5, period_ms);
        p[7] = intensity;
    }
    Some(zone_effect(
        zone,
        index,
        p,
        format!(
            "zone {}: colour cycle, period {period_ms} ms, intensity {intensity}",
            zone.index
        ),
    ))
}

/// Цветная волна. Интенсивность есть только у версии 0x16.
pub fn wave(zone: &Zone, period_ms: u16, direction: Direction, intensity: u8) -> Option<Write> {
    let (id, index) = pick(zone, &[EFFECT_WAVE_S, EFFECT_WAVE])?;
    let mut p = [0; 10];
    if id == EFFECT_WAVE_S {
        p[1] = SATURATION;
        p[8] = intensity;
    }
    put_u16(&mut p, 6, period_ms);
    p[9] = direction as u8;
    Some(zone_effect(
        zone,
        index,
        p,
        format!(
            "zone {}: wave {}, period {period_ms} ms, intensity {intensity}",
            zone.index,
            direction.name()
        ),
    ))
}

/// Рябь от нажатых клавиш.
pub fn ripple(zone: &Zone, rgb: [u8; 3], period: u16) -> Option<Write> {
    let (id, index) = pick(zone, &[EFFECT_RIPPLE_S, EFFECT_RIPPLE])?;
    let mut p = [0; 10];
    p[..3].copy_from_slice(&rgb);
    if id == EFFECT_RIPPLE_S {
        p[3] = SATURATION;
        put_u16(&mut p, 6, period);
    } else {
        put_u16(&mut p, 4, period);
    }
    Some(zone_effect(
        zone,
        index,
        p,
        format!(
            "zone {}: ripple {}, period {period}",
            zone.index,
            color_text(rgb)
        ),
    ))
}

/// Событие `onUserActivity` (fn1, sw_id 0): `Some(true)` — активность, `Some(false)` — простой.
pub fn activity_event(dev: &Device, r: &crate::hidpp::message::Report) -> Option<bool> {
    let f = dev.feature(ID)?;
    (r.device() == dev.index && r.sub_id() == f.index && r.address() == SET_ZONE_EFFECT << 4)
        .then(|| r.params()[0] != 0)
}

/// Запись эффекта для сохранённой настройки подсветки.
pub fn effect_for(zone: &Zone, lighting: crate::settings::Lighting) -> Option<Write> {
    use crate::settings::Lighting as L;
    match lighting {
        L::Off => disabled(zone),
        L::Static { color } => static_color(zone, color),
        L::Breathe {
            color,
            period_ms,
            intensity,
        } => breathe(zone, color, period_ms, intensity),
        L::Cycle {
            period_ms,
            intensity,
        } => cycle(zone, period_ms, intensity),
        L::Wave {
            period_ms,
            direction,
            intensity,
        } => wave(zone, period_ms, direction, intensity),
        L::Ripple { color, period } => ripple(zone, color, period),
    }
}

/// Разбор цвета `RRGGBB` или `#RRGGBB`.
pub fn parse_rgb(s: &str) -> Option<[u8; 3]> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone() -> Zone {
        Zone {
            index: 0,
            location: 1,
            effects: vec![
                Effect {
                    index: 0,
                    id: 0x00,
                    caps: 0,
                    period: 0,
                },
                Effect {
                    index: 1,
                    id: 0x01,
                    caps: 0,
                    period: 0,
                },
            ],
        }
    }

    #[test]
    fn static_color_layout() {
        let w = static_color(&zone(), [0xFF, 0x80, 0x00]).unwrap();
        assert_eq!(w.function, 1);
        assert_eq!(
            w.params,
            vec![0, 1, 0xFF, 0x80, 0x00, 0, 0, 0, 0, 0, 0, 0, 1]
        );
    }

    fn zone_with(ids: &[u16]) -> Zone {
        Zone {
            index: 0,
            location: 1,
            effects: ids
                .iter()
                .enumerate()
                .map(|(i, &id)| Effect {
                    index: i as u8,
                    id,
                    caps: 0,
                    period: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn animated_layouts() {
        let z = zone_with(&[0x00, 0x01, 0x0A, 0x15, 0x16, 0x17]);
        // 5000 мс = 0x1388
        assert_eq!(
            breathe(&z, [0xFF, 0x80, 0x00], 5000, 100).unwrap().params,
            vec![0, 2, 0xFF, 0x80, 0x00, 0x13, 0x88, 0, 100, 0, 0, 0, 1]
        );
        assert_eq!(
            cycle(&z, 5000, 100).unwrap().params,
            vec![0, 3, 0, 0xFF, 0, 0, 0, 0, 0x13, 0x88, 100, 0, 1]
        );
        assert_eq!(
            wave(&z, 5000, Direction::Left, 100).unwrap().params,
            vec![0, 4, 0, 0xFF, 0, 0, 0, 0, 0x13, 0x88, 100, 6, 1]
        );
        assert_eq!(
            ripple(&z, [0, 0, 0xFF], 20).unwrap().params,
            vec![0, 5, 0, 0, 0xFF, 0xFF, 0, 0, 0, 20, 0, 0, 1]
        );
    }

    #[test]
    fn animated_layouts_old_ids() {
        let z = zone_with(&[0x03, 0x04, 0x0B]);
        assert_eq!(
            cycle(&z, 5000, 100).unwrap().params,
            vec![0, 0, 0, 0, 0, 0, 0, 0x13, 0x88, 100, 0, 0, 1]
        );
        assert_eq!(
            wave(&z, 5000, Direction::Right, 100).unwrap().params,
            vec![0, 1, 0, 0, 0, 0, 0, 0, 0x13, 0x88, 0, 1, 1]
        );
        assert_eq!(
            ripple(&z, [0, 0, 0xFF], 20).unwrap().params,
            vec![0, 2, 0, 0, 0xFF, 0, 0, 20, 0, 0, 0, 0, 1]
        );
        assert!(breathe(&z, [0; 3], 5000, 100).is_none());
    }

    #[test]
    fn parses_colors() {
        assert_eq!(parse_rgb("#ff8000"), Some([0xFF, 0x80, 0x00]));
        assert_eq!(parse_rgb("00FF00"), Some([0, 0xFF, 0]));
        assert_eq!(parse_rgb("fff"), None);
    }
}

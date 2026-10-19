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
    }
}

/// Забрать управление подсветкой (клавиатура продолжает сообщать о простое).
pub fn claim() -> Write {
    write(SW_CONTROL, &SW_ACTIVE, "управление подсветкой — у Hubless")
}

/// Режим простоя: клавиатура сообщает о возобновлении активности.
pub fn idle_mode() -> Write {
    write(SW_CONTROL, &SW_IDLE, "подсветка: режим простоя")
}

/// Вернуть управление подсветкой клавиатуре.
pub fn release() -> Write {
    write(
        SW_CONTROL,
        &SW_RELEASE,
        "управление подсветкой — у клавиатуры",
    )
}

pub fn power_off() -> Write {
    write(POWER, &[0x01, 0x03, 0x00], "подсветка: сон")
}

pub fn power_on() -> Write {
    write(POWER, &[0x01, 0x01, 0x00], "подсветка: проснуться")
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
            "зона {}: статичный цвет #{:02X}{:02X}{:02X}",
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
        format!("зона {}: выключить", zone.index),
    ))
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

    #[test]
    fn parses_colors() {
        assert_eq!(parse_rgb("#ff8000"), Some([0xFF, 0x80, 0x00]));
        assert_eq!(parse_rgb("00FF00"), Some([0, 0xFF, 0]));
        assert_eq!(parse_rgb("fff"), None);
    }
}

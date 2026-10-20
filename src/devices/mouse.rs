// SPDX-License-Identifier: GPL-3.0-or-later
//! Мышь (PRO X Superlight 2): чтение состояния и бэкап onboard-памяти.

use std::path::PathBuf;

use serde_json::json;

use crate::config::config_dir;
use crate::devices::discovery::Found;
use crate::hidpp::features::{battery, device_info, dpi, onboard, report_rate};
use crate::hidpp::message::hex;
use crate::hidpp::{Device, Error, Write};
use crate::settings::{self, MouseSettings};

#[derive(Debug)]
pub struct Status {
    pub battery: Result<battery::Battery, Error>,
    pub dpi: Result<dpi::Parameters, Error>,
    pub dpi_caps: Result<dpi::Capabilities, Error>,
    pub dpi_supported: Result<Vec<u16>, Error>,
    pub rate: Result<u8, Error>,
    pub rate_supported: Result<Vec<u8>, Error>,
    pub onboard_mode: Result<onboard::Mode, Error>,
    pub onboard_profile: Result<u16, Error>,
    pub onboard_dpi_index: Result<u8, Error>,
}

pub fn status(dev: &Device) -> Status {
    Status {
        battery: battery::status(dev),
        dpi: dpi::parameters(dev),
        dpi_caps: dpi::capabilities(dev),
        dpi_supported: dpi::supported(dev, false),
        rate: report_rate::current(dev),
        rate_supported: report_rate::supported(dev),
        onboard_mode: onboard::mode(dev),
        onboard_profile: onboard::current_profile(dev),
        onboard_dpi_index: onboard::current_dpi_index(dev),
    }
}

/// Краткая запись списка DPI отрезками с постоянным шагом: `100–200 шаг 1, 200–500 шаг 2`.
pub fn describe_dpi_list(list: &[u16]) -> String {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < list.len() {
        let start = list[i];
        let Some(&next) = list.get(i + 1) else {
            parts.push(start.to_string());
            break;
        };
        let step = next - start;
        let mut j = i + 1;
        while j + 1 < list.len() && list[j + 1] - list[j] == step {
            j += 1;
        }
        if j - i >= 2 {
            parts.push(format!("{start}–{} step {step}", list[j]));
            i = j;
            if i + 1 == list.len() {
                break;
            }
        } else {
            parts.push(start.to_string());
            i += 1;
        }
    }
    if parts.is_empty() {
        "—".into()
    } else {
        parts.join(", ")
    }
}

pub fn lift_off_to_device(l: settings::LiftOff) -> dpi::LiftOff {
    match l {
        settings::LiftOff::Low => dpi::LiftOff::Low,
        settings::LiftOff::Medium => dpi::LiftOff::Medium,
        settings::LiftOff::High => dpi::LiftOff::High,
    }
}

pub fn lift_off_from_device(l: dpi::LiftOff) -> Option<settings::LiftOff> {
    match l {
        dpi::LiftOff::Low => Some(settings::LiftOff::Low),
        dpi::LiftOff::Medium => Some(settings::LiftOff::Medium),
        dpi::LiftOff::High => Some(settings::LiftOff::High),
        dpi::LiftOff::Unknown(_) => None,
    }
}

/// Мышь со старыми фичами DPI/частоты (`0x2201`, `0x8060`), а не новыми.
pub fn old_dpi(dev: &Device) -> bool {
    dev.feature(dpi::ID).is_none() && dev.feature(dpi::OLD_ID).is_some()
}

pub fn old_rate(dev: &Device) -> bool {
    dev.feature(report_rate::ID).is_none() && dev.feature(report_rate::OLD_ID).is_some()
}

/// У мыши со старыми фичами Hubless не включает режим host: в нём мышь отключает onboard-профиль,
/// а с ним и свою кнопку DPI — она оживала только после перезапуска мыши (G903, отчёт
/// пользователя 2026-10-20). DPI и частота пишутся в onboard-режиме.
pub fn host_mode_supported(dev: &Device) -> bool {
    !(old_dpi(dev) || old_rate(dev))
}

/// Записи настроек из `devices.toml`: в режиме host — сначала режим, затем DPI/lift-off и частота.
/// Значения, которые устройство в текущем подключении не поддерживает, пропускаются
/// (например, 8000 Гц по кабелю) — с записью в лог. Старые фичи (`0x2201`, `0x8060`, G903)
/// пишутся по спецификации Logitech и Solaar без режима host (см. [`host_mode_supported`]),
/// а оставшийся от 0.1.3 режим host снимается.
pub fn host_writes(dev: &Device, s: &MouseSettings) -> Vec<Write> {
    let mut writes = Vec::new();
    if host_mode_supported(dev) {
        if s.mode != Some(settings::Mode::Host) {
            return Vec::new();
        }
        writes.push(onboard::set_mode(onboard::Mode::Host));
    } else if matches!(onboard::mode(dev), Ok(onboard::Mode::Host)) {
        writes.push(onboard::set_mode(onboard::Mode::Onboard));
    }
    if old_dpi(dev) {
        if let Some(v) = s.dpi {
            match dpi::old_supported(dev) {
                Ok(list) if list.contains(&v) => {
                    if dpi::old_current(dev).ok() != Some(v) {
                        writes.push(dpi::old_set(v));
                    }
                }
                _ => tracing::warn!(
                    v,
                    "DPI из devices.toml не поддерживается (0x2201) — пропуск"
                ),
            }
        }
    } else if s.dpi.is_some() || s.lift_off.is_some() {
        match dpi::parameters(dev) {
            Ok(cur) => {
                let x = s.dpi.unwrap_or(cur.x);
                let y = s.dpi_y.or(s.dpi).unwrap_or(cur.y);
                let lod = s.lift_off.map_or(cur.lift_off, lift_off_to_device);
                let ok = dpi::supported(dev, false).is_ok_and(|l| l.contains(&x) && l.contains(&y));
                if ok {
                    writes.push(dpi::set_parameters(x, y, lod));
                } else {
                    tracing::warn!(x, y, "DPI из devices.toml не поддерживается — пропуск");
                }
            }
            Err(e) => tracing::warn!("чтение DPI: {e}"),
        }
    }
    if old_rate(dev) {
        if let Some(hz) = s.rate_hz {
            let supported = report_rate::old_supported_hz(dev).is_ok_and(|l| l.contains(&hz));
            match report_rate::old_set(hz).filter(|_| supported) {
                Some(w) => {
                    if report_rate::old_current_hz(dev).ok() != Some(hz) {
                        writes.push(w);
                    }
                }
                None => tracing::warn!(
                    hz,
                    "частота из devices.toml не поддерживается (0x8060) — пропуск"
                ),
            }
        }
    } else if let Some(hz) = s.rate_hz {
        match report_rate::index_of_hz(hz) {
            Some(i) if report_rate::supported(dev).is_ok_and(|l| l.contains(&i)) => {
                if report_rate::current(dev).ok() != Some(i) {
                    writes.push(report_rate::set(i));
                }
            }
            _ => tracing::warn!(hz, "частота из devices.toml не поддерживается — пропуск"),
        }
    }
    writes
}

/// Перечитать значение после записи: `Some(true)` — применилось, `Some(false)` — нет,
/// `None` — проверить не удалось.
pub fn verify(dev: &Device, w: &Write) -> Option<bool> {
    let p = &w.params;
    match w.feature {
        report_rate::ID => report_rate::current(dev).ok().map(|cur| cur == p[0]),
        report_rate::OLD_ID => report_rate::old_current_hz(dev)
            .ok()
            .map(|hz| hz == 1000 / u32::from(p[0].max(1))),
        dpi::OLD_ID => dpi::old_current(dev)
            .ok()
            .map(|v| v.to_be_bytes() == [p[1], p[2]]),
        dpi::ID => dpi::parameters(dev).ok().map(|cur| {
            cur.x.to_be_bytes() == [p[1], p[2]]
                && cur.y.to_be_bytes() == [p[3], p[4]]
                && cur.lift_off.to_byte() == p[5]
        }),
        onboard::ID => onboard::mode(dev).ok().map(|m| match m {
            onboard::Mode::Onboard => p[0] == 1,
            onboard::Mode::Host => p[0] == 2,
            onboard::Mode::Unknown(b) => p[0] == b,
        }),
        _ => None,
    }
}

/// Полный бэкап onboard-памяти: все пользовательские сектора и сектора ROM-профилей.
/// Пишет `<backups>/<pid>-<unit_id>-<время>.bin` и `.json` с описанием раскладки.
pub fn backup(found: &Found) -> Result<PathBuf, String> {
    let dev = found.device();
    let info = onboard::info(&dev).map_err(|e| format!("0x8100 info: {e}"))?;
    let unit_id = device_info::device_info(&dev)
        .map(|d| hex(&d.raw[device_info::UNIT_ID_BYTES]).replace(' ', ""))
        .unwrap_or_else(|_| "unknown".into());

    // Пользовательские сектора 0..N (0 — каталог профилей) и ROM: каталог 0x0100 + профили 0x0101...
    let mut sectors: Vec<u16> = (0..u16::from(info.sector_count)).collect();
    sectors.extend((0..=u16::from(info.rom_profile_count)).map(|i| onboard::ROM_BASE + i));

    let mut bin = Vec::new();
    let mut layout = Vec::new();
    for s in sectors {
        match onboard::read_sector(&dev, s, info.sector_size) {
            Ok(data) => {
                let crc_ok = onboard::sector_crc_ok(&data);
                let empty = data.iter().all(|&b| b == 0xFF) || data.iter().all(|&b| b == 0);
                layout.push(json!({
                    "sector": format!("{s:04X}"),
                    "offset": bin.len(),
                    "size": data.len(),
                    "crc_ok": crc_ok,
                    "empty": empty,
                }));
                bin.extend_from_slice(&data);
            }
            Err(e) => layout.push(json!({ "sector": format!("{s:04X}"), "error": e.to_string() })),
        }
    }

    let dir = config_dir().join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stem = format!(
        "{:04X}-{unit_id}-{}",
        found.pid,
        crate::timeutil::compact_utc()
    );
    let bin_path = dir.join(format!("{stem}.bin"));
    let meta = json!({
        "device": found.name,
        "pid": format!("{:04X}", found.pid),
        "unit_id": unit_id,
        "created_utc": crate::timeutil::iso_utc(),
        "onboard": {
            "memory_model": info.memory_model,
            "profile_format": info.profile_format,
            "macro_format": info.macro_format,
            "profile_count": info.profile_count,
            "rom_profile_count": info.rom_profile_count,
            "button_count": info.button_count,
            "sector_count": info.sector_count,
            "sector_size": info.sector_size,
        },
        "sectors": layout,
    });
    std::fs::write(&bin_path, &bin).map_err(|e| format!("{}: {e}", bin_path.display()))?;
    let json_path = dir.join(format!("{stem}.json"));
    let text = serde_json::to_string_pretty(&meta).unwrap_or_default();
    std::fs::write(&json_path, text).map_err(|e| format!("{}: {e}", json_path.display()))?;
    Ok(bin_path)
}

#[cfg(test)]
mod tests {
    use super::describe_dpi_list;

    #[test]
    fn describes_dpi_lists() {
        let mut list: Vec<u16> = (100..=200).collect();
        list.extend((202..=500).step_by(2));
        assert_eq!(describe_dpi_list(&list), "100–200 step 1, 200–500 step 2");
        assert_eq!(describe_dpi_list(&[400, 800, 1600]), "400, 800, 1600");
        assert_eq!(describe_dpi_list(&[]), "—");
    }
}

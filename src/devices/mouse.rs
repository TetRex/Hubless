// SPDX-License-Identifier: GPL-3.0-or-later
//! Мышь (PRO X Superlight 2): чтение состояния и бэкап onboard-памяти.

use std::path::PathBuf;

use serde_json::json;

use crate::config::config_dir;
use crate::devices::discovery::Found;
use crate::hidpp::features::{battery, device_info, dpi, onboard, report_rate};
use crate::hidpp::message::hex;
use crate::hidpp::{Device, Error};

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
            parts.push(format!("{start}–{} шаг {step}", list[j]));
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
        assert_eq!(describe_dpi_list(&list), "100–200 шаг 1, 200–500 шаг 2");
        assert_eq!(describe_dpi_list(&[400, 800, 1600]), "400, 800, 1600");
        assert_eq!(describe_dpi_list(&[]), "—");
    }
}

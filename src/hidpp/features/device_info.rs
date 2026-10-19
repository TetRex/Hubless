// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x0003` DeviceFwVersion: модель, транспорты, версии прошивок.
//!
//! Раскладка ответов — по Solaar; в дампе рядом сохраняются сырые байты,
//! чтобы расшифровку можно было перепроверить.

use crate::hidpp::{Device, Error};

pub const ID: u16 = 0x0003;
const GET_DEVICE_INFO: u8 = 0;
const GET_FW_INFO: u8 = 1;

/// Байты unit id в ответе GetDeviceInfo — идентификатор экземпляра; не разбираем, в дампах маскируем.
pub const UNIT_ID_BYTES: std::ops::Range<usize> = 1..5;

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub entity_count: u8,
    /// Битовая маска транспортов (USB, eQuad, BT, BLE...).
    pub transport: u16,
    pub model_ids: [u16; 3],
    pub extended_model_id: u8,
    pub capabilities: u8,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct FirmwareInfo {
    pub entity: u8,
    pub kind: &'static str,
    /// Например `U1 12.01.B0040`; для аппаратной ревизии — только номер.
    pub version: String,
    pub raw: Vec<u8>,
}

pub fn device_info(dev: &Device) -> Result<DeviceInfo, Error> {
    let r = dev.call(ID, GET_DEVICE_INFO, &[])?;
    let p = r.params();
    let be = |i: usize| u16::from_be_bytes([p[i], p[i + 1]]);
    Ok(DeviceInfo {
        entity_count: p[0],
        transport: be(5),
        model_ids: [be(7), be(9), be(11)],
        extended_model_id: p[13],
        capabilities: p[14],
        raw: p.to_vec(),
    })
}

pub fn firmware(dev: &Device, entity: u8) -> Result<FirmwareInfo, Error> {
    let r = dev.call(ID, GET_FW_INFO, &[entity])?;
    let p = r.params();
    let kind = match p[0] & 0x0F {
        0 => "firmware",
        1 => "bootloader",
        2 => "hardware",
        3 => "touchpad",
        4 => "optical_sensor",
        5 => "softdevice",
        6 => "rf_companion",
        _ => "other",
    };
    let version = if kind == "hardware" {
        format!("{}", p[1])
    } else {
        let prefix: String = p[1..4]
            .iter()
            .filter(|b| b.is_ascii_graphic())
            .map(|&b| b as char)
            .collect();
        let build = u16::from_be_bytes([p[6], p[7]]);
        format!("{prefix} {:02X}.{:02X}.B{build:04X}", p[4], p[5])
    };
    Ok(FirmwareInfo {
        entity,
        kind,
        version,
        raw: p.to_vec(),
    })
}

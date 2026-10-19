// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x8100` Onboard Profiles (функции — по libratbag `hidpp20.c`, раскладка descriptor — оттуда же).
//!
//! Чтение: описание памяти, режим, активный профиль, индекс DPI, сектора (для бэкапа).
//! Запись: только смена режима onboard/host; запись памяти профилей не реализована.

use crate::hidpp::{Device, Error, Write};

pub const ID: u16 = 0x8100;
const GET_PROFILES_DESCR: u8 = 0x0;
const SET_ONBOARD_MODE: u8 = 0x1;
const GET_ONBOARD_MODE: u8 = 0x2;
const GET_CURRENT_PROFILE: u8 = 0x4;
const MEMORY_READ: u8 = 0x5;
const GET_CURRENT_DPI_INDEX: u8 = 0xB;

/// Сектора заводских (read-only) профилей начинаются с 0x0100.
pub const ROM_BASE: u16 = 0x0100;
const CHUNK: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub memory_model: u8,
    pub profile_format: u8,
    pub macro_format: u8,
    pub profile_count: u8,
    pub rom_profile_count: u8,
    pub button_count: u8,
    pub sector_count: u8,
    pub sector_size: u16,
    pub mechanical_layout: u8,
    pub various_info: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Onboard,
    Host,
    Unknown(u8),
}

impl Mode {
    pub fn from_byte(b: u8) -> Self {
        match b {
            1 => Self::Onboard,
            2 => Self::Host,
            other => Self::Unknown(other),
        }
    }
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "onboard" => Some(Self::Onboard),
            "host" => Some(Self::Host),
            _ => None,
        }
    }

    fn to_byte(self) -> u8 {
        match self {
            Self::Onboard => 1,
            Self::Host => 2,
            Self::Unknown(b) => b,
        }
    }
}

/// Режим: onboard — настройки из памяти мыши; host — их задаёт приложение.
pub fn set_mode(mode: Mode) -> Write {
    Write {
        feature: ID,
        function: SET_ONBOARD_MODE,
        params: vec![mode.to_byte()],
        what: format!("режим {mode:?}"),
        short: false,
    }
}

pub fn info(dev: &Device) -> Result<Info, Error> {
    let r = dev.call(ID, GET_PROFILES_DESCR, &[])?;
    let p = r.params();
    Ok(Info {
        memory_model: p[0],
        profile_format: p[1],
        macro_format: p[2],
        profile_count: p[3],
        rom_profile_count: p[4],
        button_count: p[5],
        sector_count: p[6],
        sector_size: u16::from_be_bytes([p[7], p[8]]),
        mechanical_layout: p[9],
        various_info: p[10],
    })
}

pub fn mode(dev: &Device) -> Result<Mode, Error> {
    Ok(Mode::from_byte(
        dev.call(ID, GET_ONBOARD_MODE, &[])?.params()[0],
    ))
}

/// Активный профиль: (сектор/индекс как в ответе, сырые 2 байта).
pub fn current_profile(dev: &Device) -> Result<u16, Error> {
    let r = dev.call(ID, GET_CURRENT_PROFILE, &[])?;
    let p = r.params();
    Ok(u16::from_be_bytes([p[0], p[1]]))
}

pub fn current_dpi_index(dev: &Device) -> Result<u8, Error> {
    Ok(dev.call(ID, GET_CURRENT_DPI_INDEX, &[])?.params()[0])
}

/// Прочитать сектор целиком. Прошивка не даёт читать дальше `size - 16`,
/// поэтому последний кусок читается со смещения `size - 16` (как в libratbag).
pub fn read_sector(dev: &Device, sector: u16, size: u16) -> Result<Vec<u8>, Error> {
    if size < CHUNK {
        return Err(Error::Hidpp20(0x02));
    }
    let mut data = vec![0u8; size as usize];
    let mut offset = 0u16;
    while offset < size {
        let at = offset.min(size - CHUNK);
        let [s_hi, s_lo] = sector.to_be_bytes();
        let [o_hi, o_lo] = at.to_be_bytes();
        let r = dev.call(ID, MEMORY_READ, &[s_hi, s_lo, o_hi, o_lo])?;
        data[at as usize..(at + CHUNK) as usize].copy_from_slice(&r.params()[..CHUNK as usize]);
        offset = at + CHUNK;
    }
    Ok(data)
}

/// CRC-16/CCITT-FALSE: последние 2 байта сектора (как проверяет libratbag/Solaar).
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Сектор с корректной CRC в последних двух байтах (big-endian).
pub fn sector_crc_ok(data: &[u8]) -> bool {
    let n = data.len();
    n >= 2 && crc16(&data[..n - 2]) == u16::from_be_bytes([data[n - 2], data[n - 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc16_ccitt_false_check_value() {
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x8061` Extended Adjustable Report Rate (по Solaar `ExtendedReportRate`).
//! fn1 — битовая маска поддерживаемых частот (u16), fn2 — текущий индекс, fn3 — запись индекса.

use crate::hidpp::{Device, Error, Write};

pub const ID: u16 = 0x8061;
const GET_SUPPORTED: u8 = 1;
const GET_RATE: u8 = 2;
const SET_RATE: u8 = 3;

/// Индекс частоты → Гц: 8 мс, 4 мс, 2 мс, 1 мс, 500 мкс, 250 мкс, 125 мкс.
pub const RATES_HZ: [u32; 7] = [125, 250, 500, 1000, 2000, 4000, 8000];

pub fn hz(index: u8) -> Option<u32> {
    RATES_HZ.get(index as usize).copied()
}

/// Поддерживаемые индексы частот.
pub fn supported(dev: &Device) -> Result<Vec<u8>, Error> {
    let r = dev.call(ID, GET_SUPPORTED, &[])?;
    let p = r.params();
    let mask = u16::from_be_bytes([p[0], p[1]]);
    Ok((0..RATES_HZ.len() as u8)
        .filter(|i| mask & (1 << i) != 0)
        .collect())
}

pub fn current(dev: &Device) -> Result<u8, Error> {
    Ok(dev.call(ID, GET_RATE, &[])?.params()[0])
}

/// Индекс частоты по значению в Гц.
pub fn index_of_hz(hz: u32) -> Option<u8> {
    RATES_HZ.iter().position(|&r| r == hz).map(|i| i as u8)
}

pub fn set(index: u8) -> Write {
    Write {
        feature: ID,
        function: SET_RATE,
        params: vec![index],
        what: format!("частота опроса {} Гц", hz(index).unwrap_or(0)),
    }
}

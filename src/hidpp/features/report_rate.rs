// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x8061` Extended Adjustable Report Rate (по Solaar `ExtendedReportRate`).
//! fn1 — битовая маска поддерживаемых частот (u16), fn2 — текущий индекс для типа связи,
//! fn3 — запись индекса (для текущей связи; мышь подтверждает событием fn0 `[связь, индекс]`).

use crate::hidpp::message::DEVICE_SELF;
use crate::hidpp::{Device, Error, Write};

pub const ID: u16 = 0x8061;
const GET_SUPPORTED: u8 = 1;
const GET_RATE: u8 = 2;
const SET_RATE: u8 = 3;
const LINK_WIRED: u8 = 0;
const LINK_WIRELESS: u8 = 1;

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

/// Текущая частота для связи, через которую подключена мышь. У fn2 параметр — тип связи:
/// `01` — радио, `00` — кабель (у каждой своя частота; проверено через ресивер 2026-10-20:
/// после записи 500 Гц `[01]` → `02`, `[00]` → `03`). Без параметра читалась частота кабеля,
/// отсюда ложный вывод 2026-10-19, что запись не работает.
pub fn current(dev: &Device) -> Result<u8, Error> {
    let link = if dev.index == DEVICE_SELF {
        LINK_WIRED
    } else {
        LINK_WIRELESS
    };
    Ok(dev.call(ID, GET_RATE, &[link])?.params()[0])
}

/// Старая `0x8060` Report Rate (спецификация Logitech и Solaar `ReportRate`):
/// fn0 — битовая маска интервалов (бит n — n+1 мс), fn1 — текущий интервал, мс,
/// fn2 — запись интервала, мс. Запись не проверена на железе — только в экспериментальном режиме.
pub const OLD_ID: u16 = 0x8060;
const OLD_GET_LIST: u8 = 0;
const OLD_GET_RATE: u8 = 1;
const OLD_SET_RATE: u8 = 2;

/// Поддерживаемые частоты по `0x8060`, Гц, по возрастанию.
pub fn old_supported_hz(dev: &Device) -> Result<Vec<u32>, Error> {
    let mask = dev.call(OLD_ID, OLD_GET_LIST, &[])?.params()[0];
    let mut list: Vec<u32> = (0..8u32)
        .filter(|n| mask & (1 << n) != 0)
        .map(|n| 1000 / (n + 1))
        .collect();
    list.sort_unstable();
    list.dedup();
    Ok(list)
}

/// Запись частоты по `0x8060` (интервал в мс; `None` — частота не делит 1000 мс нацело).
pub fn old_set(hz: u32) -> Option<Write> {
    let ms = u8::try_from(1000 / hz.max(1)).ok().filter(|ms| *ms > 0)?;
    (1000 / u32::from(ms) == hz).then(|| Write {
        feature: OLD_ID,
        function: OLD_SET_RATE,
        params: vec![ms],
        what: format!("polling rate {hz} Hz (0x8060)"),
        short: false,
    })
}

/// Текущая частота по `0x8060`, Гц.
pub fn old_current_hz(dev: &Device) -> Result<u32, Error> {
    let ms = dev.call(OLD_ID, OLD_GET_RATE, &[])?.params()[0];
    if ms == 0 {
        return Err(Error::Hidpp20(0xFF));
    }
    Ok(1000 / u32::from(ms))
}

/// Индекс частоты по значению в Гц.
pub fn index_of_hz(hz: u32) -> Option<u8> {
    RATES_HZ.iter().position(|&r| r == hz).map(|i| i as u8)
}

/// Запись частоты. Short-отчётом, как G Hub (захват 2026-10-20).
pub fn set(index: u8) -> Write {
    Write {
        feature: ID,
        function: SET_RATE,
        params: vec![index],
        what: format!("polling rate {} Hz", hz(index).unwrap_or(0)),
        short: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_rate_write() {
        assert_eq!(old_set(1000).unwrap().params, vec![1]);
        assert_eq!(old_set(125).unwrap().params, vec![8]);
        assert!(old_set(300).is_none(), "1000/300 не целое число мс");
        assert!(old_set(2000).is_none(), "меньше 1 мс — нет у 0x8060");
    }
}

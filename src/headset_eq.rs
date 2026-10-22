// SPDX-License-Identifier: GPL-3.0-or-later
//! Встроенный эквалайзер гарнитуры PRO X 2 LIGHTSPEED: фича Centurion `0x0636` (ONBOARD_EQ).
//!
//! Разобран по захвату G Hub 2026-10-20 (`captures/headset_eq.pcapng`, `docs/devices/README.md`):
//! - fn1 `[00]` — чтение: `[флаг, число полос, (частота u16 BE, усиление дБ i8, тип 02) × N]`;
//!   флаг `80` — кривая сохранена в гарнитуре;
//! - fn2 — запись, 253 байта: та же шапка, постоянный заголовок и два блока коэффициентов —
//!   для 44,1 и 48 кГц. Блок: `[id, 00 35 00 05 00]`, 5 фильтров biquad по 5 коэффициентов и
//!   общее усиление. G Hub шлёт промежуточные записи с флагом `00`, последнюю — с `80`.
//!
//! Фильтры — peaking по Audio EQ Cookbook (Q = 2) на частотах полос, по убыванию усиления.
//! Числитель каждого нормирован по b0, произведение b0 уходит в общее усиление (× постоянная),
//! числитель первого фильтра домножен на постоянную (запас от перегрузки). b1 и a1 хранятся
//! пополам. Коэффициент — Q1.23 в байтах `[средний, старший, 00, младший]`.
//! Модель воспроизводит записи G Hub с точностью до 1 младшего разряда (тест ниже — байт в байт).

use std::ops::RangeInclusive;

use crate::centurion::Write;

pub const FEATURE: u16 = 0x0636;
const FN_READ: u8 = 0x10;
const FN_WRITE: u8 = 0x20;
pub const BANDS: usize = 5;
/// Пределы усиления в интерфейсе, дБ (G Hub в захвате менял в пределах −1…+4).
pub const GAIN_RANGE: RangeInclusive<i8> = -12..=12;
const FLAG_SAVED: u8 = 0x80;
const BAND_TYPE: u8 = 0x02;
const Q: f64 = 2.0;
/// Числитель первого фильтра блока × 0x6B9022 / 2²³ ≈ 0,840 (−1,5 дБ).
const FIRST_SCALE: i32 = 0x6B9022;
/// Общее усиление блока при ровной кривой.
const BASE_GAIN: i32 = 0x04C28F;
/// Заголовок перед блоками — одинаковый во всех записях G Hub.
const PREAMBLE: [u8; 11] = [
    0x05, 0x5A, 0xE3, 0x00, 0x03, 0x0E, 0x00, 0x02, 0x00, 0x00, 0x00,
];
const BLOCK_HEADER: [u8; 5] = [0x00, 0x35, 0x00, 0x05, 0x00];
/// Блоки коэффициентов: номер и частота дискретизации.
const RATES: [(u8, f64); 2] = [(1, 44_100.0), (2, 48_000.0)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub freq: u16,
    pub gain: i8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Eq {
    pub saved: bool,
    pub bands: Vec<Band>,
}

/// Запрос чтения: функция и параметры.
pub fn read_request() -> (u8, [u8; 1]) {
    (FN_READ, [0x00])
}

/// Разбор ответа на чтение.
pub fn parse(r: &[u8]) -> Option<Eq> {
    let (&flag, rest) = r.split_first()?;
    let (&count, rest) = rest.split_first()?;
    let bands = rest
        .as_chunks::<4>()
        .0
        .iter()
        .take(count as usize)
        .map(|b| Band {
            freq: u16::from_be_bytes([b[0], b[1]]),
            gain: b[2] as i8,
        })
        .collect::<Vec<_>>();
    (bands.len() == count as usize).then_some(Eq {
        saved: flag & FLAG_SAVED != 0,
        bands,
    })
}

/// Запись кривой: `save` — сохранить в гарнитуре (как последняя запись G Hub).
pub fn write(bands: &[Band; BANDS], save: bool) -> Write {
    let mut params = vec![if save { FLAG_SAVED } else { 0 }, BANDS as u8];
    for b in bands {
        params.extend_from_slice(&b.freq.to_be_bytes());
        params.extend_from_slice(&[b.gain as u8, BAND_TYPE]);
    }
    params.extend_from_slice(&PREAMBLE);
    for (id, fs) in RATES {
        params.push(id);
        params.extend_from_slice(&BLOCK_HEADER);
        params.extend_from_slice(&block(bands, fs));
    }
    let gains: Vec<String> = bands.iter().map(|b| format!("{:+}", b.gain)).collect();
    Write {
        feature: FEATURE,
        function: FN_WRITE,
        params,
        what: format!(
            "equalizer {} dB{}",
            gains.join(" "),
            if save { ", save" } else { "" }
        ),
    }
}

/// Пять фильтров блока и общее усиление.
fn block(bands: &[Band; BANDS], fs: f64) -> Vec<u8> {
    let mut order: Vec<usize> = (0..BANDS).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(bands[i].gain));
    let first_scale = f64::from(FIRST_SCALE) / f64::from(1 << 23);
    let mut gain = f64::from(BASE_GAIN) / f64::from(1 << 23);
    let mut out = Vec::with_capacity(BANDS * 20 + 4);
    for (k, &i) in order.iter().enumerate() {
        let [b0, b1, b2, a1, a2] = peaking(f64::from(bands[i].freq), f64::from(bands[i].gain), fs);
        gain *= b0;
        let scale = if k == 0 { first_scale } else { 1.0 };
        let coefs = [scale, b1 / b0 * scale / 2.0, b2 / b0 * scale, a1 / 2.0, a2];
        for c in coefs {
            out.extend_from_slice(&encode(c));
        }
    }
    out.extend_from_slice(&encode(gain));
    out
}

/// Peaking EQ (Audio EQ Cookbook), нормированный по a0: `[b0, b1, b2, a1, a2]`.
fn peaking(f0: f64, gain_db: f64, fs: f64) -> [f64; 5] {
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f64::consts::PI * f0 / fs;
    let alpha = w0.sin() / (2.0 * Q);
    let a0 = 1.0 + alpha / a;
    [
        (1.0 + alpha * a) / a0,
        -2.0 * w0.cos() / a0,
        (1.0 - alpha * a) / a0,
        -2.0 * w0.cos() / a0,
        (1.0 - alpha / a) / a0,
    ]
}

/// Q1.23 → `[средний, старший, 00, младший]`.
fn encode(x: f64) -> [u8; 4] {
    let v = (x * f64::from(1 << 23))
        .round()
        .clamp(-f64::from(1 << 23), f64::from((1 << 23) - 1)) as i32;
    let v = v as u32 & 0xFF_FFFF;
    [(v >> 8) as u8, (v >> 16) as u8, 0, v as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Запись G Hub из захвата 2026-10-20 (178,276 с): полосы −1, +2, +1, +3, +3 дБ.
    const CAPTURED: &str = "00050050ff0200f0020202ee01020898030219c80302055ae300030e000200000001\
        0035000500906b002243a200f08259001ca78d002d6870004fff7f00ff25c300b8734e005d80bf005bd15a00ee\
        ff7f00ff4a81003a917d007f0a8100bb107e008fff7f00ff398400d3f77800e3da8300e4b67900d7ff7f00ff5a\
        800009507f002d648000ba3a7f00ca32050027020035000500906b0022e4a00075d25a005a308c009991710073\
        ff7f00ff30bc00edd15000ed55b8006eb25c00f7ff7f00ff2e810038c37d009df38000cc387e0085ff7f00ffd6\
        8300d0857900f87f83001f367a0033ff7f00ff528000945e7f006d5c8000694a7f00c42b050072";

    fn bands(gains: [i8; 5]) -> [Band; 5] {
        let freqs = [80, 240, 750, 2200, 6600];
        std::array::from_fn(|i| Band {
            freq: freqs[i],
            gain: gains[i],
        })
    }

    #[test]
    fn matches_g_hub_byte_for_byte() {
        let w = write(&bands([-1, 2, 1, 3, 3]), false);
        assert_eq!(w.params.len(), 253);
        assert_eq!(w.params, hex(CAPTURED));
        assert_eq!(w.function, FN_WRITE);
    }

    #[test]
    fn flat_curve_is_identity_but_first_scale() {
        let w = write(&bands([0; 5]), true);
        assert_eq!(w.params[0], FLAG_SAVED);
        // Первый фильтр первого блока: b0 = постоянная, остальные фильтры начинаются с 1,0.
        assert_eq!(&w.params[39..43], &[0x90, 0x6B, 0x00, 0x22]);
        assert_eq!(&w.params[59..63], &[0xFF, 0x7F, 0x00, 0xFF]);
    }

    #[test]
    fn parses_read_reply() {
        // Ответ гарнитуры из захвата: 80 Гц +4, 240 Гц +2, остальные 0, сохранено.
        let r = hex("80050050040200f0020202ee000208980002" /* 2200 Гц */);
        let r = [r, hex("19c80002")].concat();
        let eq = parse(&r).unwrap();
        assert!(eq.saved);
        assert_eq!(eq.bands.len(), 5);
        assert_eq!((eq.bands[0].freq, eq.bands[0].gain), (80, 4));
        assert_eq!((eq.bands[4].freq, eq.bands[4].gain), (6600, 0));
    }
}

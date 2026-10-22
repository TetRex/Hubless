// SPDX-License-Identifier: GPL-3.0-or-later
//! The built-in equalizer of the PRO X 2 LIGHTSPEED headset: Centurion feature `0x0636` (ONBOARD_EQ).
//!
//! Decoded from a G Hub capture 2026-10-20 (`captures/headset_eq.pcapng`, `docs/devices/README.md`):
//! - fn1 `[00]` reads: `[flag, number of bands, (frequency u16 BE, gain dB i8, type 02) × N]`;
//!   flag `80` means the curve is saved in the headset;
//! - fn2 writes, 253 bytes: the same header, a constant preamble and two coefficient blocks,
//!   for 44.1 and 48 kHz. A block is `[id, 00 35 00 05 00]`, 5 biquad filters with 5 coefficients each, and
//!   an overall gain. G Hub sends intermediate writes with flag `00` and the last one with `80`.
//!
//! The filters are peaking filters from the Audio EQ Cookbook (Q = 2) at the band frequencies, sorted by
//! gain in descending order. Each numerator is normalized by b0, and the product of the b0 values goes into
//! the overall gain (× a constant); the first filter's numerator is multiplied by a constant (headroom against
//! clipping). b1 and a1 are stored halved. A coefficient is Q1.23 in the byte order `[middle, high, 00, low]`.
//! The model reproduces G Hub's writes to within 1 LSB (the test below matches byte for byte).

use std::ops::RangeInclusive;

use crate::centurion::Write;

pub const FEATURE: u16 = 0x0636;
const FN_READ: u8 = 0x10;
const FN_WRITE: u8 = 0x20;
pub const BANDS: usize = 5;
/// Gain limits in the interface, dB (G Hub changed them within −1…+4 in the capture).
pub const GAIN_RANGE: RangeInclusive<i8> = -12..=12;
const FLAG_SAVED: u8 = 0x80;
const BAND_TYPE: u8 = 0x02;
const Q: f64 = 2.0;
/// The numerator of a block's first filter × 0x6B9022 / 2²³ ≈ 0.840 (−1.5 dB).
const FIRST_SCALE: i32 = 0x6B9022;
/// The overall gain of a block for a flat curve.
const BASE_GAIN: i32 = 0x04C28F;
/// The preamble before the blocks: the same in all G Hub writes.
const PREAMBLE: [u8; 11] = [
    0x05, 0x5A, 0xE3, 0x00, 0x03, 0x0E, 0x00, 0x02, 0x00, 0x00, 0x00,
];
const BLOCK_HEADER: [u8; 5] = [0x00, 0x35, 0x00, 0x05, 0x00];
/// Coefficient blocks: number and sample rate.
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

/// The read request: function and parameters.
pub fn read_request() -> (u8, [u8; 1]) {
    (FN_READ, [0x00])
}

/// Parses the reply to a read.
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

/// Writes the curve: `save` saves it in the headset (like G Hub's last write).
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

/// A block's five filters and its overall gain.
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

/// Peaking EQ (Audio EQ Cookbook), normalized by a0: `[b0, b1, b2, a1, a2]`.
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

/// Q1.23 → `[middle, high, 00, low]`.
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

    /// A G Hub write from the capture 2026-10-20 (178.276 s): bands −1, +2, +1, +3, +3 dB.
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
        // The first filter of the first block: b0 = the constant, the other filters start at 1.0.
        assert_eq!(&w.params[39..43], &[0x90, 0x6B, 0x00, 0x22]);
        assert_eq!(&w.params[59..63], &[0xFF, 0x7F, 0x00, 0xFF]);
    }

    #[test]
    fn parses_read_reply() {
        // The headset's reply from the capture: 80 Hz +4, 240 Hz +2, the rest 0, saved.
        let r = hex("80050050040200f0020202ee000208980002" /* 2200 Гц */);
        let r = [r, hex("19c80002")].concat();
        let eq = parse(&r).unwrap();
        assert!(eq.saved);
        assert_eq!(eq.bands.len(), 5);
        assert_eq!((eq.bands[0].freq, eq.bands[0].gain), (80, 4));
        assert_eq!((eq.bands[4].freq, eq.bands[4].gain), (6600, 0));
    }
}

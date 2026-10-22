// SPDX-License-Identifier: GPL-3.0-or-later
//! Battery: `0x1004` Unified Battery or the old `0x1001` Battery Voltage (layouts per Solaar).
//!
//! - `0x1004` fn1: `[state_of_charge %, level flags, charging status, external power]`;
//! - `0x1001` fn0: `[voltage mV (2), flags]`; the percentage is an estimate from the discharge curve (G903 and
//!   other older mice, user report 2026-10-20).
//!
//! The status also arrives as an event (function 0, sw_id 0) when the charge changes.

use crate::hidpp::message::Report;
use crate::hidpp::{Device, Error};

pub const ID: u16 = 0x1004;
const GET_STATUS: u8 = 1;
pub const VOLTAGE_ID: u16 = 0x1001;
const GET_VOLTAGE: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChargeState {
    Discharging,
    Charging,
    AlmostFull,
    Full,
    SlowCharging,
    InvalidBattery,
    ThermalError,
    Unknown(u8),
}

impl ChargeState {
    fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Discharging,
            1 => Self::Charging,
            2 => Self::AlmostFull,
            3 => Self::Full,
            4 => Self::SlowCharging,
            5 => Self::InvalidBattery,
            6 => Self::ThermalError,
            other => Self::Unknown(other),
        }
    }

    pub fn is_charging(self) -> bool {
        matches!(self, Self::Charging | Self::AlmostFull | Self::SlowCharging)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    /// Level flags: 1 critical, 2 low, 4 good, 8 full.
    pub level: u8,
    pub charging: ChargeState,
    pub external_power: bool,
}

fn parse(p: &[u8]) -> Battery {
    Battery {
        percent: p[0],
        level: p[1],
        charging: ChargeState::from_byte(p[2]),
        external_power: p[3] != 0,
    }
}

// Ported from Solaar (`hidpp20.py`: decipher_battery_voltage, estimate_battery_level_percentage).
// Copyright (C) 2012-2013 Daniel Pavel
// Copyright (C) 2014-2024 Solaar Contributors <https://pwr-solaar.github.io/Solaar/>
// Solaar is distributed under GPL-2.0-or-later.

/// Voltage (mV) → percentage: points of a lithium battery's discharge curve.
const VOLTAGE_CURVE: [(u16, u8); 13] = [
    (4186, 100),
    (4067, 90),
    (3989, 80),
    (3922, 70),
    (3859, 60),
    (3811, 50),
    (3778, 40),
    (3751, 30),
    (3717, 20),
    (3671, 10),
    (3646, 5),
    (3579, 2),
    (3500, 0),
];

/// Charge estimate from voltage: linear between the curve points.
pub fn percent_from_voltage(mv: u16) -> u8 {
    if mv >= VOLTAGE_CURVE[0].0 {
        return 100;
    }
    for pair in VOLTAGE_CURVE.windows(2) {
        let ((v_hi, p_hi), (v_lo, p_lo)) = (pair[0], pair[1]);
        if mv >= v_lo {
            let k = f32::from(mv - v_lo) / f32::from(v_hi - v_lo);
            return p_lo + (f32::from(p_hi - p_lo) * k) as u8;
        }
    }
    0
}

/// `0x1001`: `[mV (2), flags]`; bit 7 is external power, and with it bits 0–1: 0 charging,
/// 1 charged, 2 not charging.
fn parse_voltage(p: &[u8]) -> Battery {
    let mv = u16::from_be_bytes([p[0], p[1]]);
    let flags = p[2];
    let external_power = flags & 0x80 != 0;
    let charging = match (external_power, flags & 0x03) {
        (false, _) => ChargeState::Discharging,
        (true, 0) => ChargeState::Charging,
        (true, 1) => ChargeState::Full,
        (true, other) => ChargeState::Unknown(other),
    };
    Battery {
        percent: percent_from_voltage(mv),
        level: 0,
        charging,
        external_power,
    }
}

// End of the ported fragment.

pub fn status(dev: &Device) -> Result<Battery, Error> {
    if dev.feature(ID).is_none() && dev.feature(VOLTAGE_ID).is_some() {
        return Ok(parse_voltage(
            dev.call(VOLTAGE_ID, GET_VOLTAGE, &[])?.params(),
        ));
    }
    Ok(parse(dev.call(ID, GET_STATUS, &[])?.params()))
}

/// A battery change event from the device (function 0, sw_id 0).
pub fn from_event(dev: &Device, r: &Report) -> Option<Battery> {
    let is_event = |id| {
        dev.feature(id).is_some_and(|f| {
            r.device() == dev.index && r.sub_id() == f.index && r.address() == 0x00
        })
    };
    if is_event(ID) {
        Some(parse(r.params()))
    } else if is_event(VOLTAGE_ID) {
        Some(parse_voltage(r.params()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voltage_curve() {
        assert_eq!(percent_from_voltage(4200), 100);
        assert_eq!(percent_from_voltage(4186), 100);
        assert_eq!(percent_from_voltage(3811), 50);
        assert_eq!(percent_from_voltage(3835), 55);
        assert_eq!(percent_from_voltage(3500), 0);
        assert_eq!(percent_from_voltage(3000), 0);
        let b = parse_voltage(&[0x0E, 0xE3, 0x80]); // 3811 mV, charging
        assert_eq!(
            (b.percent, b.charging, b.external_power),
            (50, ChargeState::Charging, true)
        );
        assert_eq!(
            parse_voltage(&[0x0E, 0xE3, 0x00]).charging,
            ChargeState::Discharging
        );
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x1004` Unified Battery (раскладка — по Solaar).
//! Статус: `[state_of_charge %, level flags, charging status, external power]`;
//! то же приходит событием (функция 0, sw_id 0) при изменении заряда.

use crate::hidpp::message::Report;
use crate::hidpp::{Device, Error};

pub const ID: u16 = 0x1004;
const GET_STATUS: u8 = 1;

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
    /// Флаги уровня: 1 critical, 2 low, 4 good, 8 full.
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

pub fn status(dev: &Device) -> Result<Battery, Error> {
    Ok(parse(dev.call(ID, GET_STATUS, &[])?.params()))
}

/// Событие изменения заряда от устройства (функция 0, sw_id 0).
pub fn from_event(dev: &Device, r: &Report) -> Option<Battery> {
    let f = dev.feature(ID)?;
    (r.device() == dev.index && r.sub_id() == f.index && r.address() == 0x00)
        .then(|| parse(r.params()))
}

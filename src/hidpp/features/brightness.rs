// SPDX-License-Identifier: GPL-3.0-or-later
//! `0x8040` Brightness Control (layout per Solaar `BrightnessControl`).
//!
//! - fn0 `getInfo` → `[max(2), steps|flags, caps, min(2)]`; caps bit 2 means a separate on/off;
//! - fn1 `getBrightness` → `[value(2)]`;
//! - fn3 `getIllumination` → bit 0 means the lighting is on.

use crate::hidpp::{Device, Error};

pub const ID: u16 = 0x8040;
const GET_INFO: u8 = 0;
const GET_BRIGHTNESS: u8 = 1;
const GET_ILLUMINATION: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub min: u16,
    pub max: u16,
    /// The number of steps (the low nibble of the steps|flags byte); 0 means smooth.
    pub steps: u8,
    pub on_off: bool,
}

pub fn info(dev: &Device) -> Result<Info, Error> {
    let r = dev.call(ID, GET_INFO, &[])?;
    let p = r.params();
    Ok(Info {
        max: u16::from_be_bytes([p[0], p[1]]),
        steps: p[2] & 0x0F,
        on_off: p[3] & 0x04 != 0,
        min: u16::from_be_bytes([p[4], p[5]]),
    })
}

pub fn current(dev: &Device) -> Result<u16, Error> {
    let r = dev.call(ID, GET_BRIGHTNESS, &[])?;
    let p = r.params();
    Ok(u16::from_be_bytes([p[0], p[1]]))
}

/// Whether the lighting is on (only if `Info::on_off`).
pub fn illumination(dev: &Device) -> Result<bool, Error> {
    Ok(dev.call(ID, GET_ILLUMINATION, &[])?.params()[0] & 0x01 != 0)
}

const SET_BRIGHTNESS: u8 = 2;

pub fn set(value: u16) -> crate::hidpp::Write {
    crate::hidpp::Write {
        feature: ID,
        function: SET_BRIGHTNESS,
        params: value.to_be_bytes().to_vec(),
        what: format!("brightness {value}"),
        short: false,
    }
}

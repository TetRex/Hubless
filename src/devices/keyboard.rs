// SPDX-License-Identifier: GPL-3.0-or-later
//! Клавиатура (PRO X TKL Lightspeed): чтение состояния.

use crate::hidpp::features::{battery, brightness, onboard, rgb_effects};
use crate::hidpp::{Device, Error};

/// `0x4523` KEYBOARD_DISABLE_CONTROLS (имя — по Solaar); функции не описаны ни в одном справочнике.
pub const DISABLE_CONTROLS: u16 = 0x4523;
/// `0x8060` Report Rate (по Solaar): fn1 — текущий интервал опроса в мс.
const REPORT_RATE: u16 = 0x8060;
/// `0x8081` Per-key lighting v2.
const PER_KEY: u16 = 0x8081;

#[derive(Debug)]
pub struct Status {
    pub battery: Result<battery::Battery, Error>,
    pub brightness_info: Result<brightness::Info, Error>,
    pub brightness: Result<u16, Error>,
    pub illumination: Option<Result<bool, Error>>,
    pub rgb: Result<rgb_effects::Info, Error>,
    /// Интервал опроса, мс.
    pub rate_ms: Result<u8, Error>,
    pub onboard_mode: Result<onboard::Mode, Error>,
    pub has_per_key: bool,
    pub has_disable_controls: bool,
}

pub fn status(dev: &Device) -> Status {
    let brightness_info = brightness::info(dev);
    let illumination = brightness_info
        .as_ref()
        .is_ok_and(|i| i.on_off)
        .then(|| brightness::illumination(dev));
    Status {
        battery: battery::status(dev),
        brightness: brightness::current(dev),
        brightness_info,
        illumination,
        rgb: rgb_effects::info(dev),
        rate_ms: dev.call(REPORT_RATE, 1, &[]).map(|r| r.params()[0]),
        onboard_mode: onboard::mode(dev),
        has_per_key: dev.feature(PER_KEY).is_some(),
        has_disable_controls: dev.feature(DISABLE_CONTROLS).is_some(),
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Keyboard (PRO X TKL Lightspeed): reading its state and writing the colours of single keys.

use crate::hidpp::features::{battery, brightness, onboard, per_key, rgb_effects};
use crate::hidpp::{Device, Error, Write};
use crate::keyboard_layout;
use crate::settings::{KeyboardSettings, Lighting};

/// `0x4523` KEYBOARD_DISABLE_CONTROLS (name per Solaar); its functions are not described in any reference.
pub const DISABLE_CONTROLS: u16 = 0x4523;
/// `0x4540` KEYBOARD_LAYOUT_2: the keyboard layout (the reply format is not decoded).
pub const LAYOUT: u16 = 0x4540;
/// `0x8060` Report Rate (per Solaar): fn1 is the current polling interval in ms.
const REPORT_RATE: u16 = 0x8060;

#[derive(Debug)]
pub struct Status {
    pub battery: Result<battery::Battery, Error>,
    pub brightness_info: Result<brightness::Info, Error>,
    pub brightness: Result<u16, Error>,
    pub illumination: Option<Result<bool, Error>>,
    pub rgb: Result<rgb_effects::Info, Error>,
    /// Polling interval, ms.
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
        has_per_key: dev.feature(per_key::ID).is_some(),
        has_disable_controls: dev.feature(DISABLE_CONTROLS).is_some(),
    }
}

/// The colour of keys without their own colour: the static effect's colour, or black when the lighting is off.
/// With animated effects (breathe, cycle, wave, ripple) key colours are not applied, so
/// `None`: how a per-key frame behaves over an animation is not verified.
pub fn key_base(lighting: Option<Lighting>) -> Option<[u8; 3]> {
    match lighting? {
        Lighting::Static { color } => Some(color),
        Lighting::Off => Some([0, 0, 0]),
        _ => None,
    }
}

/// Writes of the key colours from the settings over the effect (a `0x8081` frame); empty if the effect
/// doesn't allow them or there are no colours of your own. `zones` are the zones the keyboard has.
pub fn key_writes(kb: &KeyboardSettings, zones: &[u8]) -> Vec<Write> {
    if key_base(kb.lighting).is_none() {
        return Vec::new();
    }
    let keys: Vec<(u8, [u8; 3])> = kb
        .key_colors()
        .into_iter()
        .filter_map(|(name, rgb)| {
            let zone = keyboard_layout::by_name(&name)?.zone();
            zones.contains(&zone).then_some((zone, rgb))
        })
        .collect();
    per_key::frame(&keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_colours_only_over_static_or_off() {
        let mut kb = KeyboardSettings {
            lighting: Some(Lighting::Static {
                color: [0, 0, 0xff],
            }),
            ..KeyboardSettings::default()
        };
        kb.set_key("w", Some([0xff, 0, 0]));
        kb.set_key("lctrl", Some([0xff, 0, 0]));
        let zones: Vec<u8> = (1..=111).collect();
        let w = key_writes(&kb, &zones);
        let params: Vec<_> = w.iter().map(|w| w.params.clone()).collect();
        assert_eq!(
            params,
            vec![
                vec![23, 23, 0xff, 0, 0],
                vec![104, 104, 0xff, 0, 0],
                vec![0]
            ]
        );
        assert_eq!(
            key_writes(&kb, &[23]).len(),
            2,
            "нет зоны — клавиша пропускается"
        );
        kb.lighting = Some(Lighting::Cycle {
            period_ms: 5000,
            intensity: 100,
        });
        assert!(key_writes(&kb, &zones).is_empty());
        assert_eq!(key_base(Some(Lighting::Off)), Some([0, 0, 0]));
        assert_eq!(key_base(None), None);
    }
}

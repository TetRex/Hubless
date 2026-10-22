// SPDX-License-Identifier: GPL-3.0-or-later
//! Presets for the headset's built-in equalizer: built-in curves and your own ones from `devices.toml`.
//!
//! The selected preset isn't stored anywhere: the curve lives in the headset, and the preset is recognized from it.
//! If the curve matches none of them, the list gets a last item "Custom", and that one is selected.

use crate::headset_eq::BANDS;
use crate::i18n::tr;
use crate::settings::EqPreset;

pub type Gains = [i8; BANDS];

/// Built-in curves, dB per band at 80 Hz, 240 Hz, 750 Hz, 2.2 kHz, 6.6 kHz. The names are the English
/// originals for [`tr`].
pub const BUILT_IN: [(&str, Gains); 4] = [
    ("Flat", [0, 0, 0, 0, 0]),
    // Lows, without raising the mids.
    ("Bass boost", [6, 3, 0, 0, 0]),
    // Speech: less rumble, clearer consonants (1–4 kHz).
    ("Voice", [-4, -2, 1, 4, 2]),
    // Footsteps and reloads in shooters: quieter explosions, brighter upper mids.
    ("FPS", [-4, -2, 0, 4, 3]),
];

/// A preset in the list: the name in the interface language, and the curve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub gains: Gains,
    /// Your own (it can be deleted), otherwise built-in.
    pub custom: bool,
}

/// All presets: the built-in ones first, then your own ones in the order they were added.
pub fn all(custom: &[EqPreset]) -> Vec<Preset> {
    let built_in = BUILT_IN.iter().map(|(name, gains)| Preset {
        name: tr(name).to_owned(),
        gains: *gains,
        custom: false,
    });
    let own = custom.iter().filter_map(|p| {
        Some(Preset {
            name: p.name.clone(),
            gains: p.gains.as_slice().try_into().ok()?,
            custom: true,
        })
    });
    built_in.chain(own).collect()
}

/// The list to choose from and the index of the current curve in it. If the curve matches no preset,
/// the list ends with a "Custom" item, and that one is selected.
pub fn choices(presets: &[Preset], current: &Gains) -> (Vec<String>, usize) {
    let mut names: Vec<String> = presets.iter().map(|p| p.name.clone()).collect();
    let index = presets
        .iter()
        .position(|p| p.gains == *current)
        .unwrap_or_else(|| {
            names.push(tr("Custom").to_owned());
            names.len() - 1
        });
    (names, index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_curves_fit_the_headset() {
        for (name, gains) in BUILT_IN {
            assert!(
                gains
                    .iter()
                    .all(|g| crate::headset_eq::GAIN_RANGE.contains(g)),
                "{name}"
            );
        }
    }

    #[test]
    fn current_curve_is_found_or_custom() {
        let own = vec![EqPreset {
            name: "Моя".into(),
            gains: vec![1, 1, 1, 1, 1],
        }];
        let presets = all(&own);
        assert_eq!(presets.len(), BUILT_IN.len() + 1);
        assert!(presets.last().unwrap().custom);
        let (names, i) = choices(&presets, &[0; BANDS]);
        assert_eq!((names.len(), i), (presets.len(), 0));
        let (names, i) = choices(&presets, &[1; BANDS]);
        assert_eq!((names[i].as_str(), i), ("Моя", BUILT_IN.len()));
        let (names, i) = choices(&presets, &[2, 0, 0, 0, 0]);
        assert_eq!((names.len(), i), (presets.len() + 1, presets.len()));
    }
}

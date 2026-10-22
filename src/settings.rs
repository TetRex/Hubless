// SPDX-License-Identifier: GPL-3.0-or-later
//! Device settings for host mode: `%APPDATA%\Hubless\devices.toml`.
//!
//! The app writes this file itself (and the CLI write commands), separately from the hand-edited
//! `config.toml`. In host mode the app applies these values every time the mouse connects.
//!
//! Profiles: `[mouse]`/`[keyboard]` at the root of the file are the "Default" profile, the others are `[[profile]]`
//! with a name and a list of exe names for automatic switching. `active` is the profile selected by hand;
//! automatic switching by the foreground window lives only in the tray's memory ([`set_auto_profile`]).
//! [`DeviceSettings::load`] returns the active profile's settings, and [`DeviceSettings::update`]
//! changes that same profile.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::config::config_dir;
use crate::hidpp::features::rgb_effects::Direction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The settings come from the mouse's memory; the app applies nothing.
    Onboard,
    /// The app applies the settings every time the device connects.
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiftOff {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseSettings {
    pub mode: Option<Mode>,
    pub dpi: Option<u16>,
    pub dpi_y: Option<u16>,
    pub lift_off: Option<LiftOff>,
    pub rate_hz: Option<u32>,
}

/// The keyboard lighting Hubless holds (`None`: the keyboard controls it itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "effect")]
pub enum Lighting {
    Off,
    Static {
        #[serde(with = "rgb_hex")]
        color: [u8; 3],
    },
    Breathe {
        #[serde(with = "rgb_hex")]
        color: [u8; 3],
        period_ms: u16,
        intensity: u8,
    },
    Cycle {
        period_ms: u16,
        intensity: u8,
    },
    Wave {
        period_ms: u16,
        direction: Direction,
        intensity: u8,
    },
    Ripple {
        #[serde(with = "rgb_hex")]
        color: [u8; 3],
        period: u16,
    },
}

impl Lighting {
    /// The effect's colour, if the effect has one.
    pub fn color(&self) -> Option<[u8; 3]> {
        match self {
            Self::Static { color } | Self::Breathe { color, .. } | Self::Ripple { color, .. } => {
                Some(*color)
            }
            Self::Off | Self::Cycle { .. } | Self::Wave { .. } => None,
        }
    }

    /// The same effect with another colour; an effect without a colour becomes a static colour.
    pub fn with_color(self, rgb: [u8; 3]) -> Self {
        match self {
            Self::Breathe {
                period_ms,
                intensity,
                ..
            } => Self::Breathe {
                color: rgb,
                period_ms,
                intensity,
            },
            Self::Ripple { period, .. } => Self::Ripple { color: rgb, period },
            _ => Self::Static { color: rgb },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardSettings {
    pub brightness: Option<u16>,
    pub lighting: Option<Lighting>,
    /// Turn the lighting off after this many idle minutes (`0` = never, unset = as in the firmware).
    /// Works while Hubless controls the lighting, and not earlier than the keyboard reports idle
    /// (5 minutes on the PRO X TKL).
    pub lighting_off_min: Option<u16>,
    /// Colours of single keys over the static effect (or with the lighting off):
    /// key name → `rrggbb`, `[keyboard.keys]`. The names are the same as in `hubless keyboard keys`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
}

impl KeyboardSettings {
    /// Key colours (strings that don't parse as `rrggbb` are skipped).
    pub fn key_colors(&self) -> BTreeMap<String, [u8; 3]> {
        self.keys
            .iter()
            .filter_map(|(name, hex)| {
                let rgb = crate::hidpp::features::rgb_effects::parse_rgb(hex)?;
                Some((name.to_ascii_lowercase(), rgb))
            })
            .collect()
    }

    /// Sets (`Some`) or removes (`None`) a key's colour.
    pub fn set_key(&mut self, name: &str, rgb: Option<[u8; 3]>) {
        let name = name.to_ascii_lowercase();
        match rgb {
            Some([r, g, b]) => {
                self.keys.insert(name, format!("{r:02x}{g:02x}{b:02x}"));
            }
            None => {
                self.keys.remove(&name);
            }
        }
    }
}

/// Headset settings of a profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeadsetSettings {
    /// The built-in equalizer curve, dB per band from low to high (unset: the "Default" profile's curve).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eq: Option<Vec<i8>>,
}

impl HeadsetSettings {
    pub fn is_empty(&self) -> bool {
        self.eq.is_none()
    }
}

/// The settings of one profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    pub mouse: MouseSettings,
    pub keyboard: KeyboardSettings,
    #[serde(skip_serializing_if = "HeadsetSettings::is_empty")]
    pub headset: HeadsetSettings,
}

/// The name of the profile at the root of the file.
pub const BASE_PROFILE: &str = "Default";

/// No more of your own palette colours than fit in the second row of circles in the panel.
pub const MAX_COLORS: usize = 10;

/// A limit on your own equalizer presets: the list in the panel should stay short.
pub const MAX_EQ_PRESETS: usize = 20;

/// Your own preset for the headset's built-in equalizer: a name and band gains, dB (from low to high).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EqPreset {
    pub name: String,
    pub gains: Vec<i8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Exe names (without the path, case-insensitive) whose foreground window turns the profile on by itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exe: Vec<String>,
    #[serde(flatten)]
    pub settings: DeviceSettings,
}

/// Panel cards: the order and hidden devices (shared by all profiles).
/// Devices are identified by [`DeviceKind::key`](crate::state::DeviceKind::key).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelLayout {
    /// The card order; devices not listed follow in the normal order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// Devices not shown in the panel.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
}

impl PanelLayout {
    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.hidden.is_empty()
    }

    pub fn is_hidden(&self, key: &str) -> bool {
        self.hidden.iter().any(|k| k == key)
    }

    pub fn set_hidden(&mut self, key: &str, hidden: bool) {
        self.hidden.retain(|k| k != key);
        if hidden {
            self.hidden.push(key.to_owned());
        }
    }

    /// The device's place in the order (those not listed come after all listed ones).
    pub fn rank(&self, key: &str) -> usize {
        self.order
            .iter()
            .position(|k| k == key)
            .unwrap_or(self.order.len())
    }
}

/// The whole `devices.toml` file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsFile {
    /// The profile selected by hand (`None` means "Default").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// Your own keyboard lighting colours for the panel palette, `"rrggbb"` (shared by all profiles).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<String>,
    /// The keyboard layout in the key colour editor: `ansi` (no value) or `iso`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyboard_layout: Option<String>,
    /// The order and visibility of the panel cards.
    #[serde(skip_serializing_if = "PanelLayout::is_empty")]
    pub panel: PanelLayout,
    #[serde(flatten)]
    pub base: DeviceSettings,
    /// Your own headset equalizer presets (shared by all profiles).
    #[serde(rename = "eq_preset", skip_serializing_if = "Vec::is_empty")]
    pub eq_presets: Vec<EqPreset>,
    #[serde(rename = "profile", skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<Profile>,
}

/// The profile switched on automatically by the foreground window (only in the tray process).
static AUTO_PROFILE: Mutex<Option<String>> = Mutex::new(None);

/// Switches on a profile by the foreground window (`None` goes back to the one selected by hand).
pub fn set_auto_profile(name: Option<String>) {
    *AUTO_PROFILE.lock().unwrap_or_else(|e| e.into_inner()) = name;
}

fn auto_profile() -> Option<String> {
    AUTO_PROFILE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

impl SettingsFile {
    pub fn path() -> PathBuf {
        config_dir().join("devices.toml")
    }

    /// A missing or broken file gives empty settings (the error goes to the log).
    pub fn load() -> Self {
        match std::fs::read_to_string(Self::path()) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!("devices.toml: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        // The user sees the header, so it is in English, like the CLI and the documentation.
        let header =
            "# Hubless: device settings for host mode. This file is written by the app.\n\n";
        std::fs::create_dir_all(config_dir()).map_err(|e| e.to_string())?;
        std::fs::write(Self::path(), format!("{header}{text}")).map_err(|e| e.to_string())
    }

    /// The names of all profiles, "Default" first.
    pub fn names(&self) -> Vec<String> {
        std::iter::once(BASE_PROFILE.to_owned())
            .chain(self.profiles.iter().map(|p| p.name.clone()))
            .collect()
    }

    /// The active profile: the automatic one, otherwise the one selected by hand (a missing one means "Default").
    pub fn current_name(&self) -> String {
        auto_profile()
            .or_else(|| self.active.clone())
            .filter(|n| self.find(n).is_some())
            .unwrap_or_else(|| BASE_PROFILE.to_owned())
    }

    fn find(&self, name: &str) -> Option<&DeviceSettings> {
        if name == BASE_PROFILE {
            return Some(&self.base);
        }
        self.profiles
            .iter()
            .find(|p| p.name == name)
            .map(|p| &p.settings)
    }

    fn find_mut(&mut self, name: &str) -> Option<&mut DeviceSettings> {
        if name == BASE_PROFILE {
            return Some(&mut self.base);
        }
        self.profiles
            .iter_mut()
            .find(|p| p.name == name)
            .map(|p| &mut p.settings)
    }

    pub fn current(&self) -> &DeviceSettings {
        let name = self.current_name();
        self.find(&name).unwrap_or(&self.base)
    }

    pub fn current_mut(&mut self) -> &mut DeviceSettings {
        let name = self.current_name();
        if self.find(&name).is_none() {
            return &mut self.base;
        }
        self.find_mut(&name).expect("профиль найден выше")
    }

    /// A new profile is a copy of the active one.
    pub fn add(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("empty profile name".into());
        }
        if self.find(name).is_some() {
            return Err(format!("profile \"{name}\" already exists"));
        }
        let settings = self.current().clone();
        self.profiles.push(Profile {
            name: name.to_owned(),
            exe: Vec::new(),
            settings,
        });
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        if name == BASE_PROFILE {
            return Err(format!("profile \"{BASE_PROFILE}\" cannot be deleted"));
        }
        let before = self.profiles.len();
        self.profiles.retain(|p| p.name != name);
        if self.profiles.len() == before {
            return Err(format!("no profile \"{name}\""));
        }
        if self.active.as_deref() == Some(name) {
            self.active = None;
        }
        Ok(())
    }

    /// Selects a profile by hand.
    pub fn select(&mut self, name: &str) -> Result<(), String> {
        if self.find(name).is_none() {
            return Err(format!("no profile \"{name}\""));
        }
        self.active = (name != BASE_PROFILE).then(|| name.to_owned());
        Ok(())
    }

    /// Binds an exe to a profile (the file name without the path, for example `cs2.exe`).
    pub fn bind(&mut self, name: &str, exe: &str) -> Result<(), String> {
        let exe = exe.trim().to_lowercase();
        if exe.is_empty() {
            return Err("empty exe name".into());
        }
        let profile = self
            .profiles
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| {
                format!("no profile \"{name}\" (\"{BASE_PROFILE}\" cannot be bound to an exe)")
            })?;
        if !profile.exe.contains(&exe) {
            profile.exe.push(exe);
        }
        Ok(())
    }

    /// Removes the binding of an exe from a profile.
    pub fn unbind(&mut self, name: &str, exe: &str) -> Result<(), String> {
        let profile = self
            .profiles
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("no profile \"{name}\""))?;
        profile.exe.retain(|e| e != exe);
        Ok(())
    }

    /// Renames a profile ("Default" can't be renamed).
    pub fn rename(&mut self, old: &str, new: &str) -> Result<(), String> {
        let new = new.trim();
        if new.is_empty() {
            return Err("empty profile name".into());
        }
        if old == BASE_PROFILE {
            return Err(format!("profile \"{BASE_PROFILE}\" cannot be renamed"));
        }
        if new != old && self.find(new).is_some() {
            return Err(format!("profile \"{new}\" already exists"));
        }
        let profile = self
            .profiles
            .iter_mut()
            .find(|p| p.name == old)
            .ok_or_else(|| format!("no profile \"{old}\""))?;
        profile.name = new.to_owned();
        if self.active.as_deref() == Some(old) {
            self.active = Some(new.to_owned());
        }
        Ok(())
    }

    /// Your own palette colours (strings that don't parse as `rrggbb` are skipped).
    pub fn custom_colors(&self) -> Vec<[u8; 3]> {
        self.colors
            .iter()
            .filter_map(|s| crate::hidpp::features::rgb_effects::parse_rgb(s))
            .collect()
    }

    /// Adds your own colour at the end of the palette (a repeat isn't added).
    pub fn add_color(&mut self, rgb: [u8; 3]) -> Result<(), String> {
        let colors = self.custom_colors();
        if colors.contains(&rgb) {
            return Ok(());
        }
        if colors.len() >= MAX_COLORS {
            return Err(format!("no more than {MAX_COLORS} colours"));
        }
        self.colors = colors
            .into_iter()
            .chain(std::iter::once(rgb))
            .map(|[r, g, b]| format!("{r:02x}{g:02x}{b:02x}"))
            .collect();
        Ok(())
    }

    /// Removes your own colour from the palette by value.
    pub fn remove_color_value(&mut self, rgb: [u8; 3]) {
        if let Some(i) = self.custom_colors().iter().position(|c| *c == rgb) {
            self.remove_color(i);
        }
    }

    /// Removes your own colour by its index in the palette.
    pub fn remove_color(&mut self, index: usize) {
        let mut colors = self.custom_colors();
        if index < colors.len() {
            colors.remove(index);
        }
        self.colors = colors
            .into_iter()
            .map(|[r, g, b]| format!("{r:02x}{g:02x}{b:02x}"))
            .collect();
    }

    /// The equalizer curve of profile `name`: its own, otherwise the "Default" profile's.
    pub fn eq_for(&self, name: &str) -> Option<Vec<i8>> {
        self.find(name)
            .and_then(|p| p.headset.eq.clone())
            .or_else(|| self.base.headset.eq.clone())
            .filter(|g| g.len() == crate::headset_eq::BANDS)
    }

    /// Saves an equalizer curve in profile `name`. The first curve saved in another profile also gives
    /// "Default" the curve that was on before (`previous`), so switching back to it restores that curve.
    pub fn set_eq(&mut self, name: &str, gains: &[i8], previous: Option<&[i8]>) {
        if name != BASE_PROFILE
            && self.base.headset.eq.is_none()
            && let Some(previous) = previous
        {
            self.base.headset.eq = Some(previous.to_vec());
        }
        if let Some(p) = self.find_mut(name) {
            p.headset.eq = Some(gains.to_vec());
        }
    }

    /// Your own equalizer presets with a name and the full number of bands (ones broken by hand are skipped).
    pub fn custom_eq_presets(&self) -> Vec<EqPreset> {
        self.eq_presets
            .iter()
            .filter(|p| p.gains.len() == crate::headset_eq::BANDS && !p.name.trim().is_empty())
            .cloned()
            .collect()
    }

    /// Saves a curve as your own preset; a preset with the same name is overwritten.
    pub fn save_eq_preset(&mut self, name: &str, gains: &[i8]) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("empty preset name".into());
        }
        if let Some(p) = self.eq_presets.iter_mut().find(|p| p.name == name) {
            p.gains = gains.to_vec();
            return Ok(());
        }
        if self.eq_presets.len() >= MAX_EQ_PRESETS {
            return Err(format!("no more than {MAX_EQ_PRESETS} presets"));
        }
        self.eq_presets.push(EqPreset {
            name: name.to_owned(),
            gains: gains.to_vec(),
        });
        Ok(())
    }

    pub fn remove_eq_preset(&mut self, name: &str) {
        self.eq_presets.retain(|p| p.name != name);
    }

    /// The profile bound to an exe (case-insensitive comparison).
    pub fn profile_for_exe(&self, exe: &str) -> Option<&str> {
        let exe = exe.to_lowercase();
        self.profiles
            .iter()
            .find(|p| p.exe.contains(&exe))
            .map(|p| p.name.as_str())
    }
}

/// A colour in the file as a string `"ff8000"`.
mod rgb_hex {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(c: &[u8; 3], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 3], D::Error> {
        let s = String::deserialize(d)?;
        let s = s.strip_prefix('#').unwrap_or(&s);
        let v = u32::from_str_radix(s, 16)
            .ok()
            .filter(|_| s.len() == 6)
            .ok_or_else(|| D::Error::custom(format!("colour must be RRGGBB: {s}")))?;
        Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
    }
}

impl DeviceSettings {
    pub fn path() -> PathBuf {
        SettingsFile::path()
    }

    /// The active profile's settings.
    pub fn load() -> Self {
        SettingsFile::load().current().clone()
    }

    /// Changes the active profile and saves the file; returns the profile's new settings.
    pub fn update(f: impl FnOnce(&mut DeviceSettings)) -> Result<DeviceSettings, String> {
        let mut file = SettingsFile::load();
        f(file.current_mut());
        file.save()?;
        Ok(file.current().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_partial() {
        let s = DeviceSettings {
            keyboard: KeyboardSettings {
                brightness: Some(50),
                lighting: Some(Lighting::Static {
                    color: [0xff, 0x80, 0x00],
                }),
                ..KeyboardSettings::default()
            },
            mouse: MouseSettings {
                mode: Some(Mode::Host),
                dpi: Some(1600),
                dpi_y: Some(1600),
                lift_off: Some(LiftOff::High),
                rate_hz: None,
            },
            headset: HeadsetSettings {
                eq: Some(vec![6, 3, 0, 0, 0]),
            },
        };
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<DeviceSettings>(&text).unwrap(), s);
        assert!(text.contains("color = \"ff8000\""), "{text}");
        let partial: DeviceSettings = toml::from_str("[mouse]\nmode = \"host\"\n").unwrap();
        assert_eq!(partial.mouse.mode, Some(Mode::Host));
        assert_eq!(partial.mouse.dpi, None);
    }

    #[test]
    fn animated_lighting_roundtrip() {
        let s = DeviceSettings {
            keyboard: KeyboardSettings {
                brightness: None,
                lighting: Some(Lighting::Wave {
                    period_ms: 5000,
                    direction: Direction::CenterOut,
                    intensity: 100,
                }),
                ..KeyboardSettings::default()
            },
            ..Default::default()
        };
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("effect = \"wave\""), "{text}");
        assert!(text.contains("direction = \"center-out\""), "{text}");
        assert_eq!(toml::from_str::<DeviceSettings>(&text).unwrap(), s);
    }

    #[test]
    fn profiles_roundtrip_and_old_files() {
        // An old file without profiles is read as the "Default" profile.
        let old: SettingsFile = toml::from_str("[mouse]\ndpi = 800\n").unwrap();
        assert_eq!(old.current().mouse.dpi, Some(800));
        assert_eq!(old.names(), vec![BASE_PROFILE.to_owned()]);

        let mut f = old.clone();
        f.add("CS2").unwrap();
        f.bind("CS2", "CS2.exe").unwrap();
        f.select("CS2").unwrap();
        f.current_mut().mouse.dpi = Some(1600);
        let text = toml::to_string_pretty(&f).unwrap();
        let back: SettingsFile = toml::from_str(&text).unwrap();
        assert_eq!(back, f, "{text}");
        assert_eq!(back.current().mouse.dpi, Some(1600));
        assert_eq!(back.base.mouse.dpi, Some(800));
        assert_eq!(back.profile_for_exe("cs2.EXE"), Some("CS2"));
        assert!(f.add("CS2").is_err());
        assert!(f.remove(BASE_PROFILE).is_err());
        f.rename("CS2", "Игры").unwrap();
        assert_eq!(
            f.current_name(),
            "Игры",
            "активный профиль переименован вместе с ним"
        );
        assert_eq!(f.profile_for_exe("cs2.exe"), Some("Игры"));
        assert!(f.rename(BASE_PROFILE, "X").is_err());
        f.unbind("Игры", "cs2.exe").unwrap();
        assert_eq!(f.profile_for_exe("cs2.exe"), None);
        f.remove("Игры").unwrap();
        assert_eq!(f.current_name(), BASE_PROFILE);
    }

    #[test]
    fn custom_colors() {
        let mut f: SettingsFile = toml::from_str("[mouse]\ndpi = 800\n").unwrap();
        f.add_color([0xff, 0x80, 0x00]).unwrap();
        f.add_color([0xff, 0x80, 0x00]).unwrap();
        f.add_color([0, 0x10, 0xaa]).unwrap();
        assert_eq!(f.colors, vec!["ff8000".to_owned(), "0010aa".to_owned()]);
        let text = toml::to_string_pretty(&f).unwrap();
        let back: SettingsFile = toml::from_str(&text).unwrap();
        assert_eq!(
            back.custom_colors(),
            vec![[0xff, 0x80, 0x00], [0, 0x10, 0xaa]],
            "{text}"
        );
        assert_eq!(back.base.mouse.dpi, Some(800));
        f.remove_color(0);
        assert_eq!(f.colors, vec!["0010aa".to_owned()]);
        for i in 0..MAX_COLORS as u8 {
            let _ = f.add_color([i, i, 1]);
        }
        assert_eq!(f.colors.len(), MAX_COLORS);
        assert!(f.add_color([9, 9, 9]).is_err());
    }

    #[test]
    fn key_colours_roundtrip() {
        let mut f: SettingsFile = toml::from_str(
            "[keyboard]
brightness = 50

[keyboard.lighting]
effect = \"static\"
color = \"0080ff\"
",
        )
        .unwrap();
        f.base.keyboard.set_key("W", Some([0xff, 0, 0]));
        f.base.keyboard.set_key("lctrl", Some([0, 0xff, 0]));
        f.base.keyboard.set_key("lctrl", None);
        f.base.keyboard.keys.insert("a".into(), "nonsense".into());
        let text = toml::to_string_pretty(&f).unwrap();
        let back: SettingsFile = toml::from_str(&text).unwrap();
        let keys = back.base.keyboard.key_colors();
        assert_eq!(keys.len(), 1, "{text}");
        assert_eq!(keys.get("w"), Some(&[0xff, 0, 0]));
        assert_eq!(back.base.keyboard.brightness, Some(50));
        assert!(back.base.keyboard.lighting.is_some(), "{text}");
    }

    #[test]
    fn eq_curve_per_profile() {
        let mut f: SettingsFile =
            toml::from_str("[mouse]\ndpi = 800\n\n[[profile]]\nname = \"Game\"\n").unwrap();
        assert_eq!(
            f.eq_for("Game"),
            None,
            "nothing saved yet: the curve isn't touched"
        );
        f.set_eq("Game", &[6, 3, 0, 0, 0], Some(&[0, 0, 0, 0, 0]));
        assert_eq!(f.eq_for("Game"), Some(vec![6, 3, 0, 0, 0]));
        assert_eq!(
            f.eq_for(BASE_PROFILE),
            Some(vec![0; 5]),
            "Default keeps the curve that was on before"
        );
        f.set_eq(BASE_PROFILE, &[1, 1, 1, 1, 1], None);
        f.set_eq("Game", &[2, 2, 2, 2, 2], Some(&[9, 9, 9, 9, 9]));
        assert_eq!(
            f.eq_for(BASE_PROFILE),
            Some(vec![1; 5]),
            "Default isn't overwritten"
        );
        let text = toml::to_string_pretty(&f).unwrap();
        let back: SettingsFile = toml::from_str(&text).unwrap();
        assert_eq!(back.eq_for("Game"), Some(vec![2; 5]), "{text}");
        assert_eq!(back.base.mouse.dpi, Some(800));
        // A profile without its own curve gets Default's.
        f.profiles.push(Profile {
            name: "Work".into(),
            ..Profile::default()
        });
        assert_eq!(f.eq_for("Work"), Some(vec![1; 5]));
    }

    #[test]
    fn eq_presets_roundtrip() {
        let mut f: SettingsFile =
            toml::from_str("[mouse]\ndpi = 800\n\n[[profile]]\nname = \"Игры\"\n").unwrap();
        f.save_eq_preset(" Моя ", &[1, 2, 3, 4, 5]).unwrap();
        f.save_eq_preset("Моя", &[0, 0, 0, 0, -3]).unwrap();
        f.save_eq_preset("Музыка", &[3, 1, 0, 1, 3]).unwrap();
        assert!(f.save_eq_preset("  ", &[0; 5]).is_err());
        let text = toml::to_string_pretty(&f).unwrap();
        let back: SettingsFile = toml::from_str(&text).unwrap();
        let presets = back.custom_eq_presets();
        assert_eq!(presets.len(), 2, "{text}");
        assert_eq!(
            (presets[0].name.as_str(), presets[0].gains.as_slice()),
            ("Моя", [0, 0, 0, 0, -3].as_slice())
        );
        assert_eq!(back.base.mouse.dpi, Some(800));
        assert_eq!(back.profiles.len(), 1);
        f.remove_eq_preset("Моя");
        assert_eq!(f.custom_eq_presets().len(), 1);
        // A preset with the wrong number of bands (edited by hand) is skipped.
        f.eq_presets.push(EqPreset {
            name: "x".into(),
            gains: vec![1, 2],
        });
        assert_eq!(f.custom_eq_presets().len(), 1);
    }

    #[test]
    fn panel_layout_roundtrip() {
        let mut f: SettingsFile = toml::from_str("[mouse]\ndpi = 800\n").unwrap();
        assert!(f.panel.is_empty());
        assert!(!toml::to_string_pretty(&f).unwrap().contains("panel"));
        f.panel.order = vec!["headset".into(), "mouse".into()];
        f.panel.set_hidden("keyboard", true);
        f.panel.set_hidden("keyboard", true);
        assert_eq!(f.panel.hidden, vec!["keyboard".to_owned()]);
        let text = toml::to_string_pretty(&f).unwrap();
        let back: SettingsFile = toml::from_str(&text).unwrap();
        assert_eq!(back, f, "{text}");
        assert_eq!(back.base.mouse.dpi, Some(800));
        assert_eq!(back.panel.rank("headset"), 0);
        assert_eq!(back.panel.rank("keyboard"), 2, "не перечисленные — в конце");
        assert!(back.panel.is_hidden("keyboard"));
        f.panel.set_hidden("keyboard", false);
        assert!(!f.panel.is_hidden("keyboard"));
    }
}

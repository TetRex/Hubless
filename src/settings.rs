// SPDX-License-Identifier: GPL-3.0-or-later
//! Настройки устройств для режима host: `%APPDATA%\Hubless\devices.toml`.
//!
//! Файл пишет само приложение (CLI-команды записи), отдельно от редактируемого вручную
//! `config.toml`. В режиме host приложение применяет эти значения при каждом подключении мыши.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::config_dir;
use crate::hidpp::features::rgb_effects::Direction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Настройки берутся из памяти мыши, приложение ничего не применяет.
    Onboard,
    /// Приложение применяет настройки при каждом подключении.
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

/// Подсветка клавиатуры, которую держит Hubless (`None` — управляет сама клавиатура).
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
    /// Цвет эффекта, если он у эффекта есть.
    pub fn color(&self) -> Option<[u8; 3]> {
        match self {
            Self::Static { color } | Self::Breathe { color, .. } | Self::Ripple { color, .. } => {
                Some(*color)
            }
            Self::Off | Self::Cycle { .. } | Self::Wave { .. } => None,
        }
    }

    /// Тот же эффект с другим цветом; эффект без цвета становится статичным цветом.
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
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    pub mouse: MouseSettings,
    pub keyboard: KeyboardSettings,
}

/// Цвет в файле как строка `"ff8000"`.
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
            .ok_or_else(|| D::Error::custom(format!("цвет должен быть RRGGBB: {s}")))?;
        Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
    }
}

impl DeviceSettings {
    pub fn path() -> PathBuf {
        config_dir().join("devices.toml")
    }

    /// Отсутствующий или повреждённый файл — пустые настройки (ошибка в лог).
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
        let header = "# Hubless: настройки устройств для режима host. Файл пишет приложение.\n\n";
        std::fs::create_dir_all(config_dir()).map_err(|e| e.to_string())?;
        std::fs::write(Self::path(), format!("{header}{text}")).map_err(|e| e.to_string())
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
            },
            mouse: MouseSettings {
                mode: Some(Mode::Host),
                dpi: Some(1600),
                dpi_y: Some(1600),
                lift_off: Some(LiftOff::High),
                rate_hz: None,
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
            },
            ..Default::default()
        };
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("effect = \"wave\""), "{text}");
        assert!(text.contains("direction = \"center-out\""), "{text}");
        assert_eq!(toml::from_str::<DeviceSettings>(&text).unwrap(), s);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Настройки устройств для режима host: `%APPDATA%\Hubless\devices.toml`.
//!
//! Файл пишет само приложение (CLI-команды записи), отдельно от редактируемого вручную
//! `config.toml`. В режиме host приложение применяет эти значения при каждом подключении мыши.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::config_dir;

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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    pub mouse: MouseSettings,
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
        let partial: DeviceSettings = toml::from_str("[mouse]\nmode = \"host\"\n").unwrap();
        assert_eq!(partial.mouse.mode, Some(Mode::Host));
        assert_eq!(partial.mouse.dpi, None);
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Пути приложения и конфиг `%APPDATA%\ProTray\config.toml`.

use std::path::PathBuf;
use std::{env, fs, io};

use serde::{Deserialize, Serialize};

pub const APP_DIR: &str = "ProTray";

/// `%APPDATA%\ProTray` — конфиг, бэкапы onboard-памяти.
pub fn config_dir() -> PathBuf {
    known_dir("APPDATA").join(APP_DIR)
}

/// `%LOCALAPPDATA%\ProTray\logs`.
pub fn log_dir() -> PathBuf {
    known_dir("LOCALAPPDATA").join(APP_DIR).join("logs")
}

fn known_dir(var: &str) -> PathBuf {
    env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Уровень логирования: error | warn | info | debug | trace.
    pub log_level: String,
    /// Hex-дамп HID-трафика на уровне debug.
    pub hid_trace: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "info".into(),
            hid_trace: false,
        }
    }
}

const DEFAULT_FILE: &str = r#"# ProTray
# Уровень логирования: error | warn | info | debug | trace
log_level = "info"

# Hex-дамп HID-трафика в лог (уровень debug)
hid_trace = false
"#;

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// Читает конфиг; при отсутствии создаёт файл по умолчанию.
    /// Ошибка разбора не фатальна: возвращаются значения по умолчанию и текст ошибки,
    /// а файл пользователя не перезаписывается.
    pub fn load() -> (Self, Option<String>) {
        let path = Self::path();
        match fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(cfg) => (cfg, None),
                Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let err = fs::create_dir_all(config_dir())
                    .and_then(|()| fs::write(&path, DEFAULT_FILE))
                    .err()
                    .map(|e| format!("не удалось создать {}: {e}", path.display()));
                (Self::default(), err)
            }
            Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_matches_default_struct() {
        let cfg: Config = toml::from_str(DEFAULT_FILE).unwrap();
        let def = Config::default();
        assert_eq!(cfg.log_level, def.log_level);
        assert_eq!(cfg.hid_trace, def.hid_trace);
    }

    #[test]
    fn missing_keys_use_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.log_level, "info");
    }
}

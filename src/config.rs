// SPDX-License-Identifier: GPL-3.0-or-later
//! Пути приложения и конфиг `%APPDATA%\Hubless\config.toml`.

use std::path::PathBuf;
use std::{env, fs, io};

use serde::{Deserialize, Serialize};

pub const APP_DIR: &str = "Hubless";

/// `%APPDATA%\Hubless` — конфиг, бэкапы onboard-памяти.
pub fn config_dir() -> PathBuf {
    known_dir("APPDATA").join(APP_DIR)
}

/// `%LOCALAPPDATA%\Hubless\logs`.
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
    /// Язык интерфейса: auto (по Windows) | ru | en. Меняется из окна настроек.
    pub language: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "info".into(),
            hid_trace: false,
            language: "auto".into(),
        }
    }
}

const DEFAULT_FILE: &str = r#"# Hubless
# Уровень логирования: error | warn | info | debug | trace
log_level = "info"

# Hex-дамп HID-трафика в лог (уровень debug)
hid_trace = false

# Язык интерфейса: auto (по Windows) | ru | en
language = "auto"
"#;

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// Записать язык интерфейса. Файл правят вручную, поэтому меняется (или добавляется)
    /// только строка `language = ...`, комментарии и остальные ключи остаются как были.
    pub fn save_language(code: &str) -> io::Result<()> {
        let path = Self::path();
        let text = fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_FILE.to_owned());
        fs::create_dir_all(config_dir())?;
        fs::write(&path, with_language(&text, code))
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
                    .map(|e| format!("could not create {}: {e}", path.display()));
                (Self::default(), err)
            }
            Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
        }
    }
}

/// Текст конфига с `language = "<code>"`: строка заменяется на месте или дописывается в конец.
fn with_language(text: &str, code: &str) -> String {
    let line = format!("language = \"{code}\"");
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let key = l.split('=').next().unwrap_or("").trim();
            if key == "language" && !l.trim_start().starts_with('#') {
                found = true;
                line.clone()
            } else {
                l.to_owned()
            }
        })
        .collect();
    if !found {
        out.push(line);
    }
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_line_is_replaced_or_appended() {
        let replaced = with_language(DEFAULT_FILE, "ru");
        assert!(replaced.contains("language = \"ru\""));
        assert!(
            replaced.contains("# Язык интерфейса"),
            "комментарии на месте"
        );
        assert_eq!(toml::from_str::<Config>(&replaced).unwrap().language, "ru");
        let appended = with_language("log_level = \"debug\"\n", "en");
        let cfg: Config = toml::from_str(&appended).unwrap();
        assert_eq!(
            (cfg.log_level.as_str(), cfg.language.as_str()),
            ("debug", "en")
        );
    }

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

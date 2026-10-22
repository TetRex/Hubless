// SPDX-License-Identifier: GPL-3.0-or-later
//! App paths and the config `%APPDATA%\Hubless\config.toml`.

use std::path::PathBuf;
use std::{env, fs, io};

use serde::{Deserialize, Serialize};

pub const APP_DIR: &str = "Hubless";

/// `%APPDATA%\Hubless`: config, onboard memory backups.
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
    /// Log level: error | warn | info | debug | trace.
    pub log_level: String,
    /// Hex dump of HID traffic at debug level.
    pub hid_trace: bool,
    /// Interface language: auto (the Windows language) | ru | en. Changed from the settings window.
    pub language: String,
    /// Low battery threshold, %: a dot on the icon and a notification. Changed from the settings window.
    pub low_battery: u8,
    /// Notify when a charging device has fully charged.
    pub notify_charged: bool,
    /// Check for updates at startup (a request to GitHub). Off by default: without it
    /// Hubless goes online only when the button is pressed.
    pub check_updates: bool,
}

/// Low battery thresholds to choose from in the settings window, %.
pub const LOW_BATTERY_CHOICES: [u8; 6] = [5, 10, 15, 20, 25, 30];
pub const DEFAULT_LOW_BATTERY: u8 = 15;

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "info".into(),
            hid_trace: false,
            language: "auto".into(),
            low_battery: DEFAULT_LOW_BATTERY,
            notify_charged: true,
            check_updates: false,
        }
    }
}

// The user sees the file's comments, so they are in English, like the CLI and the documentation.
const DEFAULT_FILE: &str = r#"# Hubless
# Log level: error | warn | info | debug | trace
log_level = "info"

# Hex dump of HID traffic in the log (at debug level)
hid_trace = false

# Interface language: auto (the Windows language) | ru | en
language = "auto"

# Low battery threshold, %: a dot on the tray icon and a notification
low_battery = 15

# Notify when a device has fully charged
notify_charged = true

# Check for updates at startup (a request to GitHub)
check_updates = false
"#;

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("config.toml")
    }

    /// Writes the interface language. The file is edited by hand, so only the `language = ...` line
    /// changes (or is added); comments and the other keys stay as they were.
    pub fn save_language(code: &str) -> io::Result<()> {
        Self::save_key("language", &format!("\"{code}\""))
    }

    pub fn save_low_battery(percent: u8) -> io::Result<()> {
        Self::save_key("low_battery", &percent.to_string())
    }

    pub fn save_notify_charged(on: bool) -> io::Result<()> {
        Self::save_key("notify_charged", &on.to_string())
    }

    pub fn save_check_updates(on: bool) -> io::Result<()> {
        Self::save_key("check_updates", &on.to_string())
    }

    /// The low battery threshold within allowed limits (the file is edited by hand).
    pub fn low_battery_percent(&self) -> u8 {
        self.low_battery.clamp(1, 50)
    }

    fn save_key(key: &str, value: &str) -> io::Result<()> {
        let path = Self::path();
        let text = fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_FILE.to_owned());
        fs::create_dir_all(config_dir())?;
        fs::write(&path, with_key(&text, key, value))
    }

    /// Reads the config; if it doesn't exist, creates the default file.
    /// A parse error isn't fatal: the defaults and the error text are returned,
    /// and the user's file isn't overwritten.
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

/// The config text with `<key> = <value>`: the line is replaced in place or appended at the end.
fn with_key(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let k = l.split('=').next().unwrap_or("").trim();
            if k == key && !l.trim_start().starts_with('#') {
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
        let with_language =
            |text: &str, code: &str| with_key(text, "language", &format!("\"{code}\""));
        let replaced = with_language(DEFAULT_FILE, "ru");
        assert!(replaced.contains("language = \"ru\""));
        assert!(
            replaced.contains("# Interface language"),
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
        assert_eq!(cfg.low_battery, def.low_battery);
        assert_eq!(cfg.notify_charged, def.notify_charged);
        assert_eq!(cfg.check_updates, def.check_updates);
        assert!(LOW_BATTERY_CHOICES.contains(&DEFAULT_LOW_BATTERY));
    }

    #[test]
    fn new_keys_are_written_into_old_files() {
        let old = "log_level = \"info\"\nlanguage = \"ru\"\n";
        let text = with_key(old, "check_updates", "true");
        let text = with_key(&text, "low_battery", "20");
        let cfg: Config = toml::from_str(&text).unwrap();
        assert!(cfg.check_updates);
        assert_eq!((cfg.low_battery, cfg.language.as_str()), (20, "ru"));
        assert!(cfg.notify_charged, "нет ключа — значение по умолчанию");
    }

    #[test]
    fn missing_keys_use_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.log_level, "info");
    }
}

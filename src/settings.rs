// SPDX-License-Identifier: GPL-3.0-or-later
//! Настройки устройств для режима host: `%APPDATA%\Hubless\devices.toml`.
//!
//! Файл пишет само приложение (CLI-команды записи), отдельно от редактируемого вручную
//! `config.toml`. В режиме host приложение применяет эти значения при каждом подключении мыши.
//!
//! Профили: `[mouse]`/`[keyboard]` в корне файла — профиль «Обычный», остальные — `[[profile]]`
//! с именем и списком exe для автопереключения. `active` — профиль, выбранный вручную;
//! автопереключение по активному окну живёт только в памяти трея ([`set_auto_profile`]).
//! [`DeviceSettings::load`] возвращает настройки действующего профиля, [`DeviceSettings::update`]
//! меняет его же.

use std::path::PathBuf;
use std::sync::Mutex;

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

/// Настройки одного профиля.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    pub mouse: MouseSettings,
    pub keyboard: KeyboardSettings,
}

/// Имя профиля из корня файла.
pub const BASE_PROFILE: &str = "Default";

/// Своих цветов в палитре — не больше, чем помещается во второй ряд кружков панели.
pub const MAX_COLORS: usize = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Имена exe (без пути, без учёта регистра), при активном окне которых профиль включается сам.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exe: Vec<String>,
    #[serde(flatten)]
    pub settings: DeviceSettings,
}

/// Карточки панели: порядок и скрытые устройства (общие для всех профилей).
/// Устройства — по ключу [`DeviceKind::key`](crate::state::DeviceKind::key).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelLayout {
    /// Порядок карточек; не перечисленные идут следом в обычном порядке.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// Устройства, которых нет в панели.
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

    /// Место устройства в порядке (не перечисленные — после всех перечисленных).
    pub fn rank(&self, key: &str) -> usize {
        self.order
            .iter()
            .position(|k| k == key)
            .unwrap_or(self.order.len())
    }
}

/// Файл `devices.toml` целиком.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsFile {
    /// Профиль, выбранный вручную (`None` — «Обычный»).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    /// Свои цвета подсветки клавиатуры для палитры панели, `"rrggbb"` (общие для профилей).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<String>,
    /// Порядок и видимость карточек панели.
    #[serde(skip_serializing_if = "PanelLayout::is_empty")]
    pub panel: PanelLayout,
    #[serde(flatten)]
    pub base: DeviceSettings,
    #[serde(rename = "profile", skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<Profile>,
}

/// Профиль, включённый автопереключением по активному окну (только в процессе трея).
static AUTO_PROFILE: Mutex<Option<String>> = Mutex::new(None);

/// Включить профиль по активному окну (`None` — вернуться к выбранному вручную).
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

    /// Имена всех профилей, «Обычный» первым.
    pub fn names(&self) -> Vec<String> {
        std::iter::once(BASE_PROFILE.to_owned())
            .chain(self.profiles.iter().map(|p| p.name.clone()))
            .collect()
    }

    /// Действующий профиль: автопереключение, иначе выбранный вручную (несуществующий — «Обычный»).
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

    /// Новый профиль — копия действующего.
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

    /// Выбрать профиль вручную.
    pub fn select(&mut self, name: &str) -> Result<(), String> {
        if self.find(name).is_none() {
            return Err(format!("no profile \"{name}\""));
        }
        self.active = (name != BASE_PROFILE).then(|| name.to_owned());
        Ok(())
    }

    /// Привязать exe к профилю (имя файла без пути, например `cs2.exe`).
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

    /// Убрать привязку exe от профиля.
    pub fn unbind(&mut self, name: &str, exe: &str) -> Result<(), String> {
        let profile = self
            .profiles
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("no profile \"{name}\""))?;
        profile.exe.retain(|e| e != exe);
        Ok(())
    }

    /// Переименовать профиль («Обычный» не переименовывается).
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

    /// Свои цвета палитры (строки, которые не разбираются как `rrggbb`, пропускаются).
    pub fn custom_colors(&self) -> Vec<[u8; 3]> {
        self.colors
            .iter()
            .filter_map(|s| crate::hidpp::features::rgb_effects::parse_rgb(s))
            .collect()
    }

    /// Добавить свой цвет в конец палитры (повтор не добавляется).
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

    /// Убрать свой цвет из палитры по значению.
    pub fn remove_color_value(&mut self, rgb: [u8; 3]) {
        if let Some(i) = self.custom_colors().iter().position(|c| *c == rgb) {
            self.remove_color(i);
        }
    }

    /// Убрать свой цвет по индексу в палитре.
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

    /// Профиль, привязанный к exe (сравнение без учёта регистра).
    pub fn profile_for_exe(&self, exe: &str) -> Option<&str> {
        let exe = exe.to_lowercase();
        self.profiles
            .iter()
            .find(|p| p.exe.contains(&exe))
            .map(|p| p.name.as_str())
    }
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
            .ok_or_else(|| D::Error::custom(format!("colour must be RRGGBB: {s}")))?;
        Ok([(v >> 16) as u8, (v >> 8) as u8, v as u8])
    }
}

impl DeviceSettings {
    pub fn path() -> PathBuf {
        SettingsFile::path()
    }

    /// Настройки действующего профиля.
    pub fn load() -> Self {
        SettingsFile::load().current().clone()
    }

    /// Изменить действующий профиль и сохранить файл; возвращает новые настройки профиля.
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

    #[test]
    fn profiles_roundtrip_and_old_files() {
        // Старый файл без профилей читается как профиль «Обычный».
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

// SPDX-License-Identifier: GPL-3.0-or-later
//! Общая модель состояния устройств — источник данных для панели и трея.

use crate::i18n::tr;
use crate::settings::PanelLayout;

/// Порог низкого заряда (индикатор на иконке, уведомление).
pub const LOW_BATTERY: u8 = 15;

/// Порядок вариантов — порядок карточек по умолчанию.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
}

impl DeviceKind {
    /// Ключ устройства в `devices.toml` (раскладка панели).
    pub fn key(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Keyboard => "keyboard",
            Self::Headset => "headset",
        }
    }

    /// Название типа — имя карточки, если устройство своего имени не сообщило.
    pub fn title(self) -> &'static str {
        match self {
            Self::Mouse => tr("Mouse"),
            Self::Keyboard => tr("Keyboard"),
            Self::Headset => tr("Headset"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Wireless,
    Usb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryState {
    pub percent: u8,
    pub charging: bool,
}

impl BatteryState {
    pub fn is_low(self) -> bool {
        self.percent < LOW_BATTERY && !self.charging
    }
}

#[derive(Debug, Clone)]
pub struct DeviceState {
    pub kind: DeviceKind,
    pub name: String,
    pub connected: bool,
    pub link: Option<Link>,
    pub battery: Option<BatteryState>,
    /// Текущие значения настроек (для контролов панели).
    pub controls: crate::devices::manager::Controls,
    /// Последняя ошибка записи — короткое сообщение в карточке.
    pub error: Option<String>,
}

impl DeviceState {
    fn new(kind: DeviceKind) -> Self {
        Self {
            kind,
            name: String::new(),
            connected: false,
            link: None,
            battery: None,
            controls: Default::default(),
            error: None,
        }
    }

    /// Имя для панели и трея: от устройства, иначе название типа.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            self.kind.title()
        } else {
            &self.name
        }
    }

    /// «94% ⚡» или «—».
    pub fn battery_text(&self) -> Option<String> {
        self.battery
            .map(|b| format!("{}%{}", b.percent, if b.charging { " ⚡" } else { "" }))
    }
}

#[derive(Debug, Clone, Default)]
pub struct AppState {
    /// Найденные устройства (по одному на тип), в порядке типов. Карточка есть, пока
    /// устройство подключено или спарено с подключённым ресивером/адаптером.
    pub devices: Vec<DeviceState>,
    /// Отображаемые имена профилей и индекс действующего.
    pub profiles: Vec<String>,
    pub profile: usize,
    /// Порядок и скрытые карточки панели (из `devices.toml`).
    pub layout: PanelLayout,
}

impl AppState {
    /// Карточка устройства; нет — добавить (на место по типу).
    pub fn device_entry(&mut self, kind: DeviceKind) -> &mut DeviceState {
        let at = match self.devices.binary_search_by_key(&kind, |d| d.kind) {
            Ok(i) => i,
            Err(i) => {
                self.devices.insert(i, DeviceState::new(kind));
                i
            }
        };
        &mut self.devices[at]
    }

    /// Устройство пропало (нет ни его, ни ресивера или адаптера) — убрать карточку.
    pub fn remove_device(&mut self, kind: DeviceKind) {
        self.devices.retain(|d| d.kind != kind);
    }

    /// Все устройства в порядке, заданном в настройках.
    pub fn ordered(&self) -> Vec<&DeviceState> {
        let mut list: Vec<&DeviceState> = self.devices.iter().collect();
        // Сортировка устойчивая: не перечисленные в порядке остаются в обычном порядке.
        list.sort_by_key(|d| self.layout.rank(d.kind.key()));
        list
    }

    /// Новый порядок (ключи всех устройств), если устройство с места `from` переставить на
    /// место `to`; `None` — индекс вне списка или место то же.
    pub fn moved(&self, from: usize, to: usize) -> Option<Vec<String>> {
        let mut keys: Vec<String> = self
            .ordered()
            .iter()
            .map(|d| d.kind.key().to_owned())
            .collect();
        if from >= keys.len() || to >= keys.len() || from == to {
            return None;
        }
        let key = keys.remove(from);
        keys.insert(to, key);
        // Устройства, которых сейчас нет, сохраняют место в списке — в конце.
        for k in &self.layout.order {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        Some(keys)
    }

    /// Карточки панели: по порядку, без скрытых.
    pub fn panel_devices(&self) -> Vec<&DeviceState> {
        let mut list = self.ordered();
        list.retain(|d| !self.layout.is_hidden(d.kind.key()));
        list
    }

    /// Строки для tooltip трея: по одной на подключённое устройство, в порядке панели.
    pub fn tray_lines(&self) -> Vec<String> {
        self.ordered()
            .into_iter()
            .filter(|d| d.connected)
            .map(|d| match d.battery_text() {
                Some(b) => format!("{}: {b}", d.display_name()),
                None => d.display_name().to_owned(),
            })
            .collect()
    }

    pub fn any_low_battery(&self) -> bool {
        self.devices
            .iter()
            .any(|d| d.connected && d.battery.is_some_and(BatteryState::is_low))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Все три устройства, как у автора.
    fn three() -> AppState {
        let mut s = AppState::default();
        for kind in [DeviceKind::Headset, DeviceKind::Mouse, DeviceKind::Keyboard] {
            s.device_entry(kind);
        }
        s
    }

    #[test]
    fn devices_appear_and_disappear() {
        let mut s = AppState::default();
        assert!(s.panel_devices().is_empty(), "до поиска карточек нет");
        s.device_entry(DeviceKind::Headset).name = "A50".into();
        s.device_entry(DeviceKind::Mouse);
        let kinds: Vec<_> = s.devices.iter().map(|d| d.kind).collect();
        assert_eq!(
            kinds,
            [DeviceKind::Mouse, DeviceKind::Headset],
            "порядок по типу"
        );
        assert_eq!(
            s.devices[0].display_name(),
            "Mouse",
            "без имени — название типа"
        );
        s.device_entry(DeviceKind::Headset);
        assert_eq!(s.devices.len(), 2, "повторно не добавляется");
        s.remove_device(DeviceKind::Mouse);
        assert_eq!(s.devices.len(), 1);
        assert_eq!(s.devices[0].display_name(), "A50");
    }

    #[test]
    fn low_battery_ignores_charging_and_disconnected() {
        let mut s = AppState::default();
        let m = s.device_entry(DeviceKind::Mouse);
        m.name = "PRO X 2".into();
        m.connected = true;
        m.battery = Some(BatteryState {
            percent: 10,
            charging: true,
        });
        assert!(!s.any_low_battery());
        s.device_entry(DeviceKind::Mouse).battery = Some(BatteryState {
            percent: 10,
            charging: false,
        });
        assert!(s.any_low_battery());
        assert_eq!(s.tray_lines(), vec!["PRO X 2: 10%".to_owned()]);
        s.device_entry(DeviceKind::Mouse).connected = false;
        assert!(!s.any_low_battery());
        assert!(s.tray_lines().is_empty());
    }

    #[test]
    fn layout_orders_and_hides() {
        let mut s = three();
        let kinds = |list: Vec<&DeviceState>| list.iter().map(|d| d.kind).collect::<Vec<_>>();
        assert_eq!(
            kinds(s.panel_devices()),
            [DeviceKind::Mouse, DeviceKind::Keyboard, DeviceKind::Headset]
        );
        s.layout.order = vec!["headset".into()];
        s.layout.set_hidden("mouse", true);
        assert_eq!(
            kinds(s.ordered()),
            [DeviceKind::Headset, DeviceKind::Mouse, DeviceKind::Keyboard]
        );
        assert_eq!(
            kinds(s.panel_devices()),
            [DeviceKind::Headset, DeviceKind::Keyboard]
        );
        assert_eq!(s.moved(0, 3), None);
        assert_eq!(s.moved(1, 1), None);
        // headset, mouse, keyboard → mouse, keyboard, headset
        assert_eq!(
            s.moved(0, 2),
            Some(vec!["mouse".into(), "keyboard".into(), "headset".into()])
        );
        assert_eq!(
            s.moved(2, 0),
            Some(vec!["keyboard".into(), "headset".into(), "mouse".into()])
        );
        // Отсутствующее устройство не теряет место в сохранённом порядке.
        s.layout.order = vec!["headset".into(), "mouse".into(), "keyboard".into()];
        s.remove_device(DeviceKind::Keyboard);
        assert_eq!(
            s.moved(1, 0),
            Some(vec!["mouse".into(), "headset".into(), "keyboard".into()])
        );
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Общая модель состояния устройств — источник данных для панели и трея.

use crate::settings::PanelLayout;

/// Порог низкого заряда (индикатор на иконке, уведомление).
pub const LOW_BATTERY: u8 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// «94% ⚡» или «—».
    pub fn battery_text(&self) -> Option<String> {
        self.battery
            .map(|b| format!("{}%{}", b.percent, if b.charging { " ⚡" } else { "" }))
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub devices: Vec<DeviceState>,
    /// Отображаемые имена профилей и индекс действующего.
    pub profiles: Vec<String>,
    pub profile: usize,
    /// Порядок и скрытые карточки панели (из `devices.toml`).
    pub layout: PanelLayout,
}

impl Default for AppState {
    /// Известные устройства, все отключены до первого опроса.
    fn default() -> Self {
        let device = |kind, name: &str| DeviceState {
            kind,
            name: name.into(),
            connected: false,
            link: None,
            battery: None,
            controls: Default::default(),
            error: None,
        };
        Self {
            devices: vec![
                device(DeviceKind::Mouse, "PRO X Superlight 2"),
                device(DeviceKind::Keyboard, "PRO X TKL"),
                device(DeviceKind::Headset, "PRO X 2 LIGHTSPEED"),
            ],
            profiles: Vec::new(),
            profile: 0,
            layout: PanelLayout::default(),
        }
    }
}

impl AppState {
    pub fn device_mut(&mut self, kind: DeviceKind) -> Option<&mut DeviceState> {
        self.devices.iter_mut().find(|d| d.kind == kind)
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
                Some(b) => format!("{}: {b}", d.name),
                None => d.name.clone(),
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

    #[test]
    fn low_battery_ignores_charging_and_disconnected() {
        let mut s = AppState::default();
        let m = s.device_mut(DeviceKind::Mouse).unwrap();
        m.connected = true;
        m.battery = Some(BatteryState {
            percent: 10,
            charging: true,
        });
        assert!(!s.any_low_battery());
        s.device_mut(DeviceKind::Mouse).unwrap().battery = Some(BatteryState {
            percent: 10,
            charging: false,
        });
        assert!(s.any_low_battery());
        assert_eq!(s.tray_lines(), vec!["PRO X Superlight 2: 10%".to_owned()]);
        s.device_mut(DeviceKind::Mouse).unwrap().connected = false;
        assert!(!s.any_low_battery());
        assert!(s.tray_lines().is_empty());
    }

    #[test]
    fn layout_orders_and_hides() {
        let mut s = AppState::default();
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
    }
}

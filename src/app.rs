// SPDX-License-Identifier: GPL-3.0-or-later
//! Состояние приложения на главном потоке и диспетчер событий.
//!
//! Все внешние источники (tray-icon, скрытое окно, таймеры, панель) шлют [`AppEvent`]
//! через [`post`]: событие выполняется позже в event loop Slint, без реентерабельности
//! внутри оконных процедур.

use std::cell::RefCell;

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::config::Config;
use crate::config::log_dir;
use crate::devices::manager::{self, Command, Role, Snapshot};
use crate::foreground::ForegroundHook;
use crate::hidden_window::HiddenWindow;
use crate::i18n::{self, tr};
use crate::panel::PanelController;
use crate::report;
use crate::settings::{self, DeviceSettings, Lighting, PanelLayout, SettingsFile};
use crate::settings_window::{self, SettingsAction, SettingsController};
use crate::state::{AppState, BatteryState, DeviceKind, LOW_BATTERY, Link};
use crate::theme::SystemTheme;
use crate::tray::{self, Tray, autostart};
use crate::win::wide;

#[derive(Debug, Clone, Copy)]
pub enum MenuCommand {
    Settings,
    ToggleAutostart,
    OpenLogs,
    Exit,
}

/// Изменение настройки из панели.
#[derive(Debug, Clone, Copy)]
pub enum PanelAction {
    /// DPI мыши (переводит мышь в режим host).
    Dpi(u16),
    /// Любое значение DPI с ползунка — применяется после паузы, подгоняется к допустимому.
    DpiDrag(f32),
    /// Любое значение DPI из поля ввода — сразу, подгоняется к допустимому.
    DpiExact(f32),
    /// Частота опроса мыши, Гц (переводит мышь в режим host).
    Rate(u32),
    /// `true` — настройки мыши применяет Hubless (host), `false` — из памяти мыши (onboard).
    MouseHost(bool),
    /// Lift-off мыши (в режиме host).
    LiftOff(settings::LiftOff),
    /// Яркость клавиатуры — применяется после паузы (debounce).
    Brightness(u16),
    /// Эффект подсветки по индексу списка в карточке (0 — управляет клавиатура).
    Effect(i32),
    /// Статичный цвет подсветки.
    Color([u8; 3]),
    /// Sidetone гарнитуры 0–100 — после паузы (debounce).
    Sidetone(u8),
    /// Автоотключение гарнитуры, минуты (0 — никогда).
    AutoOff(u8),
    /// Профиль настроек по индексу в списке шапки панели.
    Profile(i32),
}

/// Эффект для пункта списка в карточке клавиатуры (порядок — как в `device_card.slint`),
/// параметры анимаций — по умолчанию (как в Solaar).
fn effect_by_index(i: i32, color: [u8; 3]) -> Option<Lighting> {
    use crate::hidpp::features::rgb_effects::{Direction, RIPPLE_PERIOD_DEFAULT};
    const PERIOD_MS: u16 = 5000;
    const INTENSITY: u8 = 100;
    Some(match i {
        1 => Lighting::Static { color },
        2 => Lighting::Breathe {
            color,
            period_ms: PERIOD_MS,
            intensity: INTENSITY,
        },
        3 => Lighting::Cycle {
            period_ms: PERIOD_MS,
            intensity: INTENSITY,
        },
        4 => Lighting::Wave {
            period_ms: PERIOD_MS,
            direction: Direction::Right,
            intensity: INTENSITY,
        },
        5 => Lighting::Ripple {
            color,
            period: RIPPLE_PERIOD_DEFAULT,
        },
        _ => return None,
    })
}

/// Индекс пункта списка эффектов для текущей подсветки (0 — управляет клавиатура).
pub fn effect_index(l: Option<Lighting>) -> i32 {
    match l {
        None | Some(Lighting::Off) => 0,
        Some(Lighting::Static { .. }) => 1,
        Some(Lighting::Breathe { .. }) => 2,
        Some(Lighting::Cycle { .. }) => 3,
        Some(Lighting::Wave { .. }) => 4,
        Some(Lighting::Ripple { .. }) => 5,
    }
}

/// Ближайшее к `value` значение DPI из списка допустимых (список по возрастанию).
fn nearest_dpi(supported: &[u16], value: f32) -> Option<u16> {
    if !value.is_finite() {
        return None;
    }
    supported.iter().copied().min_by(|a, b| {
        let da = (f32::from(*a) - value).abs();
        let db = (f32::from(*b) - value).abs();
        da.total_cmp(&db)
    })
}

/// Пауза перед записью яркости: слайдер шлёт значения непрерывно.
const SLIDER_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);
/// Цвет подсветки, когда свой цвет включают впервые.
const DEFAULT_COLOR: [u8; 3] = [0xFF, 0x80, 0x00];

#[derive(Debug)]
pub enum AppEvent {
    /// Левый клик по иконке (прямоугольник иконки в физ. px).
    TrayClick(tray_icon::Rect),
    Menu(MenuCommand),
    /// Запрос от второго экземпляра.
    ShowPanel,
    /// Esc в панели.
    PanelDismiss,
    PanelFocusLost,
    /// Высота панели изменилась.
    PanelResized,
    /// Панель скрыта дольше порога — уничтожить окно.
    PanelIdle,
    ThemeChanged,
    DevicesChanged,
    /// `hubless --test-toast`: показать тестовое уведомление.
    TestNotification,
    /// Состояние устройства от device manager (`None` — не найдено).
    Device(Role, Option<Snapshot>),
    /// Действие в карточке панели.
    Panel(PanelAction),
    /// Яркость после паузы debounce.
    BrightnessSettled(u16),
    /// Sidetone после паузы debounce.
    SidetoneSettled(u8),
    /// DPI с ползунка после паузы debounce.
    DpiSettled(f32),
    /// Сменилось активное окно (имя exe в нижнем регистре).
    Foreground(String),
    /// Открыть окно настроек (шестерёнка в панели); `Some` — на разделе с этим номером.
    OpenSettings(Option<usize>),
    /// Действие в окне настроек.
    Settings(SettingsAction),
    /// Отчёт об устройствах собран (в фоновом потоке).
    ReportReady(Result<report::Report, String>),
}

struct App {
    tray: Tray,
    panel: PanelController,
    state: AppState,
    /// Уведомление о низком заряде уже показано (сбрасывается при зарядке или заряде ≥ порога).
    low_notified: bool,
    devices: manager::Handle,
    /// Debounce слайдеров яркости и sidetone.
    brightness_timer: slint::Timer,
    sidetone_timer: slint::Timer,
    dpi_timer: slint::Timer,
    /// Профиль, включённый по активному окну (`None` — выбранный вручную).
    auto_profile: Option<String>,
    settings: SettingsController,
    _foreground: Option<ForegroundHook>,
    _hidden: HiddenWindow,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

pub fn init() -> Result<(), Box<dyn std::error::Error>> {
    let theme = SystemTheme::read();
    tracing::info!(?theme, "системная тема");
    let app = App {
        tray: Tray::new(theme.taskbar_light)?,
        panel: PanelController::new(theme),
        state: AppState::default(),
        low_notified: false,
        devices: manager::spawn(|role, s| post(AppEvent::Device(role, s))),
        brightness_timer: slint::Timer::default(),
        sidetone_timer: slint::Timer::default(),
        dpi_timer: slint::Timer::default(),
        auto_profile: None,
        settings: SettingsController::default(),
        _foreground: ForegroundHook::install(),
        _hidden: HiddenWindow::create()?,
    };
    APP.set(Some(app));
    APP.with_borrow_mut(|app| {
        if let Some(app) = app {
            app.refresh_profiles();
        }
    });
    Ok(())
}

pub fn shutdown() {
    tray::remove_handlers();
    // Drop убирает иконку из трея и закрывает окна.
    drop(APP.take());
}

/// Поставить событие в очередь event loop (можно из любого потока).
pub fn post(ev: AppEvent) {
    if let Err(e) = slint::invoke_from_event_loop(move || dispatch(ev)) {
        tracing::warn!("invoke_from_event_loop: {e}");
    }
}

fn dispatch(ev: AppEvent) {
    APP.with_borrow_mut(|app| {
        if let Some(app) = app {
            app.handle(ev);
        }
    });
}

impl App {
    fn handle(&mut self, ev: AppEvent) {
        tracing::debug!(?ev, "event");
        match ev {
            AppEvent::TrayClick(rect) => self.panel.toggle(Some(rect), &self.state),
            AppEvent::ShowPanel => self.panel.show(self.tray.rect(), &self.state),
            AppEvent::PanelDismiss | AppEvent::PanelFocusLost => self.panel.hide(),
            AppEvent::PanelIdle => self.panel.destroy_if_hidden(),
            AppEvent::PanelResized => self.panel.reposition(),
            AppEvent::ThemeChanged => {
                let theme = SystemTheme::read();
                tracing::info!(?theme, "тема изменилась");
                self.tray.set_taskbar_light(theme.taskbar_light);
                self.panel.set_theme(theme);
            }
            AppEvent::DevicesChanged => {
                tracing::debug!("HID hotplug");
                self.devices.send(Command::Rescan);
                // Сюда же приходит «настройки изменились» от CLI — профили могли поменяться.
                self.refresh_profiles();
            }
            AppEvent::Foreground(exe) => self.on_foreground(exe),
            AppEvent::OpenSettings(page) => self.open_settings(page),
            AppEvent::Settings(action) => self.on_settings(action),
            AppEvent::ReportReady(result) => self.settings.report_finished(result),
            AppEvent::Device(role, s) => self.on_device(role, s),
            AppEvent::Panel(action) => self.on_panel(action),
            AppEvent::DpiSettled(v) => self.set_free_dpi(v),
            AppEvent::SidetoneSettled(v) => {
                self.devices.send_to(Role::Headset, Command::Sidetone(v));
            }
            AppEvent::BrightnessSettled(v) => {
                self.change(Role::Keyboard, |s| s.keyboard.brightness = Some(v));
            }
            AppEvent::TestNotification => self.tray.notify(
                tr("Hubless: notification test"),
                tr("This is what the low battery warning looks like."),
            ),
            AppEvent::Menu(cmd) => self.menu(cmd),
        }
    }

    fn on_device(&mut self, role: Role, s: Option<Snapshot>) {
        tracing::info!(?role, ?s, "устройство");
        let kind = match role {
            Role::Mouse => DeviceKind::Mouse,
            Role::Keyboard => DeviceKind::Keyboard,
            Role::Headset => DeviceKind::Headset,
        };
        // Нет снимка — устройства (и его ресивера или адаптера) нет: карточка исчезает.
        match s {
            None => self.state.remove_device(kind),
            Some(s) => {
                let m = self.state.device_entry(kind);
                m.connected = s.connected;
                m.link = s
                    .connected
                    .then_some(if s.wired { Link::Usb } else { Link::Wireless });
                m.battery = s.battery.map(|b| BatteryState {
                    percent: b.percent,
                    charging: b.charging.is_charging() || b.external_power,
                });
                m.name = s.name;
                m.controls = s.controls;
                m.error = s.error;
            }
        }
        self.refresh_status();
    }

    fn on_panel(&mut self, action: PanelAction) {
        match action {
            PanelAction::Dpi(v) => self.change(Role::Mouse, |s| {
                s.mouse.mode = Some(settings::Mode::Host);
                s.mouse.dpi = Some(v);
                s.mouse.dpi_y = Some(v);
            }),
            PanelAction::DpiDrag(v) => {
                self.dpi_timer
                    .start(slint::TimerMode::SingleShot, SLIDER_DEBOUNCE, move || {
                        post(AppEvent::DpiSettled(v));
                    });
            }
            PanelAction::DpiExact(v) => {
                self.dpi_timer.stop();
                self.set_free_dpi(v);
            }
            PanelAction::Rate(hz) => self.change(Role::Mouse, |s| {
                s.mouse.mode = Some(settings::Mode::Host);
                s.mouse.rate_hz = Some(hz);
            }),
            PanelAction::MouseHost(on) => self.change(Role::Mouse, |s| {
                s.mouse.mode = Some(if on {
                    settings::Mode::Host
                } else {
                    settings::Mode::Onboard
                });
            }),
            PanelAction::LiftOff(l) => self.change(Role::Mouse, |s| {
                s.mouse.mode = Some(settings::Mode::Host);
                s.mouse.lift_off = Some(l);
            }),
            PanelAction::Brightness(v) => {
                self.brightness_timer.start(
                    slint::TimerMode::SingleShot,
                    SLIDER_DEBOUNCE,
                    move || {
                        post(AppEvent::BrightnessSettled(v));
                    },
                );
            }
            PanelAction::Effect(i) => self.change(Role::Keyboard, |s| {
                let color = s
                    .keyboard
                    .lighting
                    .and_then(|l| l.color())
                    .unwrap_or(DEFAULT_COLOR);
                s.keyboard.lighting = effect_by_index(i, color);
            }),
            PanelAction::Color(color) => self.change(Role::Keyboard, |s| {
                let current = s.keyboard.lighting.unwrap_or(Lighting::Static { color });
                s.keyboard.lighting = Some(current.with_color(color));
            }),
            PanelAction::Sidetone(v) => {
                self.sidetone_timer.start(
                    slint::TimerMode::SingleShot,
                    SLIDER_DEBOUNCE,
                    move || {
                        post(AppEvent::SidetoneSettled(v));
                    },
                );
            }
            PanelAction::AutoOff(m) => self.devices.send_to(Role::Headset, Command::AutoOff(m)),
            PanelAction::Profile(i) => {
                let mut file = SettingsFile::load();
                let Some(name) = file.names().get(i.max(0) as usize).cloned() else {
                    return;
                };
                if let Err(e) = file.select(&name).and_then(|()| file.save()) {
                    tracing::error!("профиль: {e}");
                    return;
                }
                // Ручной выбор отменяет автопереключение до следующей смены окна.
                self.auto_profile = None;
                settings::set_auto_profile(None);
                tracing::info!(profile = name, "профиль выбран вручную");
                self.profile_changed();
            }
        }
    }

    /// Активное окно сменилось: включить профиль, привязанный к его exe, или вернуться к ручному.
    fn on_foreground(&mut self, exe: String) {
        let target = SettingsFile::load()
            .profile_for_exe(&exe)
            .map(str::to_owned);
        if target == self.auto_profile {
            return;
        }
        tracing::info!(exe, profile = ?target, "профиль по активному окну");
        self.auto_profile.clone_from(&target);
        settings::set_auto_profile(target);
        self.profile_changed();
    }

    /// Профиль сменился: применить его настройки и обновить шапку панели.
    fn profile_changed(&mut self) {
        self.devices.send_to(Role::Mouse, Command::Apply);
        self.devices.send_to(Role::Keyboard, Command::Apply);
        self.refresh_profiles();
    }

    /// Перечитать профили и раскладку панели из `devices.toml`.
    fn refresh_profiles(&mut self) {
        let file = SettingsFile::load();
        if file.panel != self.state.layout {
            self.state.layout = file.panel.clone();
            self.refresh_status();
        }
        let names = file.names();
        let current = file.current_name();
        self.state.profile = names.iter().position(|n| *n == current).unwrap_or(0);
        self.state.profiles = names
            .iter()
            .enumerate()
            .map(|(i, n)| settings_window::display_name(i, n))
            .collect();
        self.panel.refresh(&self.state);
        self.settings.refresh(None);
    }

    fn open_settings(&mut self, page: Option<usize>) {
        self.panel.hide();
        self.settings.open(self.panel.theme(), &self.state, page);
    }

    /// Изменить порядок или видимость карточек в `devices.toml` и показать результат.
    fn edit_layout(&mut self, f: impl FnOnce(&mut PanelLayout)) {
        let mut file = SettingsFile::load();
        f(&mut file.panel);
        if let Err(e) = file.save() {
            tracing::error!("devices.toml: {e}");
            self.settings.set_error(&e);
            return;
        }
        tracing::info!(order = ?file.panel.order, hidden = ?file.panel.hidden, "раскладка панели");
        self.state.layout = file.panel;
        self.refresh_status();
    }

    fn on_settings(&mut self, action: SettingsAction) {
        let names = SettingsFile::load().names();
        let name_at = |i: i32| names.get(i.max(0) as usize).cloned();
        match action {
            SettingsAction::Closed => self.settings.close(),
            SettingsAction::Select(i) => self.settings.refresh(Some(i.max(0) as usize)),
            SettingsAction::RefreshRunning => self.settings.refresh_running(),
            SettingsAction::CreateReport => {
                self.settings.report_started();
                // Опрос устройств занимает секунды — не на главном потоке.
                let spawned = std::thread::Builder::new()
                    .name("device-report".into())
                    .spawn(|| post(AppEvent::ReportReady(report::create())));
                if let Err(e) = spawned {
                    self.settings.report_finished(Err(format!("thread: {e}")));
                }
            }
            SettingsAction::OpenReports => open_folder(&report::reports_dir()),
            SettingsAction::CopyReport => {
                let copied = self
                    .settings
                    .report()
                    .is_some_and(|r| report::copy_to_clipboard(&r.text));
                self.settings.set_report_status(if copied {
                    tr("Copied to the clipboard")
                } else {
                    tr("Couldn't copy to the clipboard")
                });
            }
            SettingsAction::OpenIssue => open_folder(std::path::Path::new(report::ISSUE_URL)),
            SettingsAction::Autostart(on) => {
                if let Err(e) = autostart::set_enabled(on) {
                    tracing::error!("автозапуск: {e}");
                }
                self.tray.set_autostart_checked(autostart::is_enabled());
            }
            SettingsAction::Experimental(on) => {
                if let Err(e) = Config::save_experimental(on) {
                    tracing::error!("config.toml: {e}");
                    self.settings.set_error(&e.to_string());
                    return;
                }
                tracing::info!(on, "экспериментальная поддержка устройств");
                // Перечитать контролы (кнопки DPI и частоты у мышей со старыми фичами).
                self.devices.send_to(Role::Mouse, Command::Apply);
            }
            SettingsAction::MoveDevice(from, to) => {
                if let Some(order) = self.state.moved(from.max(0) as usize, to.max(0) as usize) {
                    self.edit_layout(|l| l.order = order);
                }
                // Строки уже с новым порядком (или со старым при ошибке) — сбросить перетаскивание.
                self.settings.finish_drop();
            }
            SettingsAction::ShowDevice(i, shown) => {
                let Some(key) = self
                    .state
                    .ordered()
                    .get(i.max(0) as usize)
                    .map(|d| d.kind.key())
                else {
                    return;
                };
                self.edit_layout(|l| l.set_hidden(key, !shown));
            }
            SettingsAction::Language(i) => {
                let choice = settings_window::choice_of(i);
                if let Err(e) = Config::save_language(choice.as_str()) {
                    tracing::error!("config.toml: {e}");
                }
                i18n::set(choice.resolve());
                tracing::info!(language = choice.as_str(), "язык интерфейса");
                self.tray.relabel();
                self.refresh_status();
                self.refresh_profiles();
            }
            SettingsAction::Create => {
                let name = settings_window::new_profile_name(&names);
                let index = names.len();
                if self.edit_profiles(|f| f.add(&name)) {
                    self.refresh_profiles();
                    self.settings.refresh(Some(index));
                }
            }
            SettingsAction::Rename(i, new) => {
                let Some(old) = name_at(i) else { return };
                let new = new.trim().to_owned();
                if new != old && (names.contains(&new) || new == tr("Default")) {
                    self.settings
                        .set_error(tr("A profile with this name already exists"));
                    return;
                }
                if self.edit_profiles(|f| f.rename(&old, &new)) {
                    if self.auto_profile.as_deref() == Some(old.as_str()) {
                        self.auto_profile = Some(new.clone());
                        settings::set_auto_profile(Some(new));
                    }
                    self.refresh_profiles();
                }
            }
            SettingsAction::Delete(i) => {
                let Some(name) = name_at(i) else { return };
                if self.edit_profiles(|f| f.remove(&name)) {
                    if self.auto_profile.as_deref() == Some(name.as_str()) {
                        self.auto_profile = None;
                        settings::set_auto_profile(None);
                    }
                    self.profile_changed();
                    self.settings.refresh(Some(0));
                }
            }
            SettingsAction::Use(i) => {
                let Some(name) = name_at(i) else { return };
                if self.edit_profiles(|f| f.select(&name)) {
                    // Ручной выбор отменяет автопереключение до следующей смены окна.
                    self.auto_profile = None;
                    settings::set_auto_profile(None);
                    tracing::info!(profile = name, "профиль выбран вручную");
                    self.profile_changed();
                }
            }
            SettingsAction::AddProgram(i, exe) => {
                let Some(name) = name_at(i) else { return };
                if exe.trim().is_empty() {
                    return;
                }
                if self.edit_profiles(|f| f.bind(&name, &exe)) {
                    self.settings.refresh(None);
                }
            }
            SettingsAction::RemoveProgram(i, exe) => {
                let Some(name) = name_at(i) else { return };
                if self.edit_profiles(|f| f.unbind(&name, &exe)) {
                    self.settings.refresh(None);
                }
            }
        }
    }

    /// Изменить профили в `devices.toml`; ошибка — сообщением в окне настроек.
    fn edit_profiles(&mut self, f: impl FnOnce(&mut SettingsFile) -> Result<(), String>) -> bool {
        let mut file = SettingsFile::load();
        match f(&mut file).and_then(|()| file.save()) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("профили: {e}");
                self.settings.set_error(&e);
                false
            }
        }
    }

    /// Любое значение DPI: подогнать к ближайшему, которое принимает мышь, и применить (host).
    fn set_free_dpi(&mut self, value: f32) {
        let supported = self
            .state
            .devices
            .iter()
            .find_map(|d| match &d.controls {
                manager::Controls::Mouse { dpi_supported, .. } => Some(dpi_supported.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let Some(v) = nearest_dpi(&supported, value) else {
            return;
        };
        self.change(Role::Mouse, |s| {
            s.mouse.mode = Some(settings::Mode::Host);
            s.mouse.dpi = Some(v);
            s.mouse.dpi_y = Some(v);
        });
    }

    /// Изменить `devices.toml` и попросить поток устройства применить его.
    fn change(&mut self, role: Role, f: impl FnOnce(&mut DeviceSettings)) {
        if let Err(e) = DeviceSettings::update(f) {
            tracing::error!("devices.toml: {e}");
            return;
        }
        self.devices.send_to(role, Command::Apply);
    }

    /// Трей, уведомление о низком заряде и панель по текущему состоянию.
    fn refresh_status(&mut self) {
        let low = self.state.any_low_battery();
        self.tray.set_status(&self.state.tray_lines(), low);
        if low && !self.low_notified {
            let names: Vec<String> = self
                .state
                .devices
                .iter()
                .filter(|d| d.connected && d.battery.is_some_and(BatteryState::is_low))
                .map(|d| {
                    format!(
                        "{} — {}",
                        d.display_name(),
                        d.battery_text().unwrap_or_default()
                    )
                })
                .collect();
            self.tray.notify(
                &tr("Battery below {}%").replace("{}", &LOW_BATTERY.to_string()),
                &names.join("\n"),
            );
        }
        self.low_notified = low;
        self.panel.refresh(&self.state);
        self.settings.update_devices(&self.state);
    }

    fn menu(&mut self, cmd: MenuCommand) {
        match cmd {
            MenuCommand::Settings => self.open_settings(None),
            MenuCommand::ToggleAutostart => {
                let enable = !autostart::is_enabled();
                if let Err(e) = autostart::set_enabled(enable) {
                    tracing::error!("автозапуск: {e}");
                }
                let enabled = autostart::is_enabled();
                tracing::info!(enabled, "автозапуск");
                self.tray.set_autostart_checked(enabled);
            }
            MenuCommand::OpenLogs => open_folder(&log_dir()),
            MenuCommand::Exit => {
                tracing::info!("выход");
                self.panel.destroy();
                let _ = slint::quit_event_loop();
            }
        }
    }
}

fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let path = wide(path);
    // SAFETY: строки живут до конца вызова.
    let res = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(path.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW возвращает значение > 32 при успехе.
    if res.0 as isize <= 32 {
        tracing::warn!("ShellExecuteW: {}", res.0 as isize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_snaps_to_nearest_supported() {
        let list = [100, 101, 102, 5000, 5050, 5100];
        assert_eq!(nearest_dpi(&list, 5030.0), Some(5050));
        assert_eq!(nearest_dpi(&list, 5010.0), Some(5000));
        assert_eq!(nearest_dpi(&list, 50.0), Some(100));
        assert_eq!(nearest_dpi(&list, 99999.0), Some(5100));
        assert_eq!(nearest_dpi(&list, f32::NAN), None);
        assert_eq!(nearest_dpi(&[], 800.0), None);
    }

    #[test]
    fn effect_list_roundtrip() {
        assert_eq!(effect_index(effect_by_index(0, DEFAULT_COLOR)), 0);
        for i in 1..=5 {
            assert_eq!(effect_index(effect_by_index(i, DEFAULT_COLOR)), i);
        }
    }

    #[test]
    fn color_keeps_effect() {
        let breathe = effect_by_index(2, DEFAULT_COLOR).unwrap();
        let blue = breathe.with_color([0, 0xAA, 0xFF]);
        assert_eq!(effect_index(Some(blue)), 2);
        assert_eq!(blue.color(), Some([0, 0xAA, 0xFF]));
        // У цикла нет цвета — выбор цвета делает статичный.
        let cycle = effect_by_index(3, DEFAULT_COLOR).unwrap();
        assert_eq!(effect_index(Some(cycle.with_color([1, 2, 3]))), 1);
    }
}

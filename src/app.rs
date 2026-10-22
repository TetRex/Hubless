// SPDX-License-Identifier: GPL-3.0-or-later
//! App state on the main thread and the event dispatcher.
//!
//! All external sources (tray-icon, the hidden window, timers, the panel) send an [`AppEvent`]
//! through [`post`]: the event runs later in the Slint event loop, with no reentrancy
//! inside window procedures.

use std::cell::RefCell;

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use crate::config::Config;
use crate::config::log_dir;
use crate::devices::manager::{self, Command, Role, Snapshot};
use crate::eq_presets;
use crate::foreground::ForegroundHook;
use crate::hidden_window::HiddenWindow;
use crate::hidpp::features::battery::ChargeState;
use crate::i18n::{self, tr};
use crate::panel::PanelController;
use crate::report;
use crate::settings::{self, DeviceSettings, Lighting, PanelLayout, SettingsFile};
use crate::settings_window::{self, SettingsAction, SettingsController};
use crate::state::{AppState, BatteryState, DeviceKind, Link};
use crate::theme::SystemTheme;
use crate::tray::{self, Tray, autostart};
use crate::win::wide;

/// A setting change from the panel.
#[derive(Debug, Clone, Copy)]
pub enum PanelAction {
    /// Mouse DPI (switches the mouse to host mode).
    Dpi(u16),
    /// Any DPI value from the slider: applied after a pause, snapped to an allowed value.
    DpiDrag(f32),
    /// Any DPI value from the input field: applied right away, snapped to an allowed value.
    DpiExact(f32),
    /// Mouse polling rate, Hz (switches the mouse to host mode).
    Rate(u32),
    /// `true`: Hubless applies the mouse settings (host); `false`: they come from the mouse's memory (onboard).
    MouseHost(bool),
    /// Mouse lift-off (in host mode).
    LiftOff(settings::LiftOff),
    /// Keyboard brightness: applied after a pause (debounce).
    Brightness(u16),
    /// Lighting effect: 1 static … 5 ripple (the card's list item + 1); 0 hands the
    /// lighting back to the keyboard (the panel has no such item).
    Effect(i32),
    /// A static lighting colour.
    Color([u8; 3]),
    /// Your own colour: add it to the palette (and turn it on), or remove it from the palette.
    AddColor([u8; 3]),
    RemoveColor([u8; 3]),
    /// Headset sidetone 0–100: after a pause (debounce).
    Sidetone(u8),
    /// Headset auto power-off, minutes (0 = never).
    AutoOff(u8),
    /// A headset equalizer preset by its index in the list (built-in ones, then your own).
    EqPreset(i32),
    /// A settings profile by its index in the panel header list.
    Profile(i32),
}

/// The effect for an item of the keyboard card's list (the order is as in `device_card.slint`);
/// animation parameters are the defaults (as in Solaar).
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

/// The effect list index for the current lighting (0: the keyboard controls it).
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

/// The allowed DPI value nearest to `value` (the list is ascending).
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

/// A pause before writing brightness: the slider sends values continuously.
const SLIDER_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);
/// A pause before saving the equalizer curve in the headset (the last write carries the save flag).
const EQ_SAVE_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
/// The update check at startup isn't immediate: after logging in to Windows the network doesn't come up instantly.
const STARTUP_UPDATE_DELAY: std::time::Duration = std::time::Duration::from_secs(30);
/// The lighting colour when your own colour is turned on for the first time.
const DEFAULT_COLOR: [u8; 3] = [0xFF, 0x80, 0x00];

#[derive(Debug)]
pub enum AppEvent {
    /// A click on the icon (the icon's rectangle in physical px).
    TrayClick(tray_icon::Rect),
    /// Exit: the button in the panel or `hubless --quit`.
    Exit,
    /// A request from a second instance.
    ShowPanel,
    /// Esc in the panel.
    PanelDismiss,
    PanelFocusLost,
    /// The panel height changed.
    PanelResized,
    /// The panel has been hidden for longer than the threshold: destroy the window.
    PanelIdle,
    ThemeChanged,
    DevicesChanged,
    /// `hubless --test-toast`: show a test notification.
    TestNotification,
    /// Started by the installer: the downloaded file can be deleted now.
    Installed,
    /// Device state from the device manager (`None`: not found).
    Device(Role, Option<Snapshot>),
    /// An action in a panel card.
    Panel(PanelAction),
    /// Brightness after the debounce pause.
    BrightnessSettled(u16),
    /// Sidetone after the debounce pause.
    SidetoneSettled(u8),
    /// Headset equalizer after the debounce pause (the live write) and after the save pause.
    EqSettled,
    EqSave,
    /// DPI from the slider after the debounce pause.
    DpiSettled(f32),
    /// The foreground window changed (exe name in lowercase).
    Foreground(String),
    /// Open the settings window (the gear in the panel); `Some` opens the section with this number.
    OpenSettings(Option<usize>),
    /// An action in the settings window.
    Settings(SettingsAction),
    /// The update check finished (on a background thread): from the button in the settings and at startup.
    UpdateChecked(Result<crate::update::Release, String>),
    StartupUpdate(Result<crate::update::Release, String>),
    /// The device report is ready (on a background thread).
    ReportReady(Result<report::Report, String>),
}

struct App {
    tray: Tray,
    panel: PanelController,
    state: AppState,
    /// The low battery notification was already shown (reset when charging or when the charge is ≥ the threshold).
    low_notified: bool,
    /// Charging devices that were charging while Hubless ran (the "charged" notification is only
    /// for them), and those it was already shown for. Both are reset when the device is taken off the charger.
    charging_seen: Vec<DeviceKind>,
    charged_notified: Vec<DeviceKind>,
    /// `config.toml`: battery threshold, notifications, update check.
    config: Config,
    devices: manager::Handle,
    /// Debounce for the brightness and sidetone sliders.
    brightness_timer: slint::Timer,
    sidetone_timer: slint::Timer,
    /// Equalizer: the curve being dragged in the settings, and the write and save timers.
    eq_gains: Option<[i8; crate::headset_eq::BANDS]>,
    eq_timer: slint::Timer,
    eq_save_timer: slint::Timer,
    /// The profile the pending equalizer edit belongs to (`None`: the curve came from a profile switch,
    /// nothing to save), and the curve that was on before the edit started.
    eq_profile: Option<String>,
    eq_before: Option<[i8; crate::headset_eq::BANDS]>,
    dpi_timer: slint::Timer,
    /// The profile switched on by the foreground window (`None`: the one selected by hand).
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
    let config = Config::load().0;
    let check_updates = config.check_updates;
    let app = App {
        tray: Tray::new(theme.taskbar_light)?,
        panel: PanelController::new(theme),
        state: AppState {
            low_battery: config.low_battery_percent(),
            ..AppState::default()
        },
        low_notified: false,
        charging_seen: Vec::new(),
        charged_notified: Vec::new(),
        config,
        devices: manager::spawn(|role, s| post(AppEvent::Device(role, s))),
        brightness_timer: slint::Timer::default(),
        sidetone_timer: slint::Timer::default(),
        eq_gains: None,
        eq_timer: slint::Timer::default(),
        eq_save_timer: slint::Timer::default(),
        eq_profile: None,
        eq_before: None,
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
    if check_updates {
        check_updates_later();
    }
    Ok(())
}

/// Checks for updates after [`STARTUP_UPDATE_DELAY`] (turned on in the settings).
fn check_updates_later() {
    let spawned = std::thread::Builder::new()
        .name("update-check".into())
        .spawn(|| {
            std::thread::sleep(STARTUP_UPDATE_DELAY);
            post(AppEvent::StartupUpdate(crate::update::latest()));
        });
    if let Err(e) = spawned {
        tracing::warn!("проверка обновлений при запуске: {e}");
    }
}

pub fn shutdown() {
    tray::remove_handlers();
    // Drop removes the tray icon and closes the windows.
    drop(APP.take());
}

/// Puts an event into the event loop queue (from any thread).
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
                // "Settings changed" from the CLI arrives here too, so the profiles may have changed.
                self.refresh_profiles();
            }
            AppEvent::Foreground(exe) => self.on_foreground(exe),
            AppEvent::OpenSettings(page) => self.open_settings(page),
            AppEvent::Settings(action) => self.on_settings(action),
            AppEvent::ReportReady(result) => self.settings.report_finished(result),
            AppEvent::UpdateChecked(result) => self.settings.update_finished(result),
            AppEvent::StartupUpdate(result) => self.startup_update(result),
            AppEvent::Device(role, s) => self.on_device(role, s),
            AppEvent::Panel(action) => self.on_panel(action),
            AppEvent::DpiSettled(v) => self.set_free_dpi(v),
            AppEvent::SidetoneSettled(v) => {
                self.devices.send_to(Role::Headset, Command::Sidetone(v));
            }
            AppEvent::EqSettled => {
                if let Some(gains) = self.eq_gains {
                    self.devices
                        .send_to(Role::Headset, Command::Eq(gains, false));
                }
            }
            AppEvent::EqSave => {
                // Like G Hub: the last write carries the "save in the headset" flag.
                if let Some(gains) = self.eq_gains.take() {
                    tracing::info!(?gains, "эквалайзер: сохранить в гарнитуре");
                    self.devices
                        .send_to(Role::Headset, Command::Eq(gains, true));
                    // The curve goes into the state right away, so the window won't show the old one before the headset replies.
                    self.set_state_eq(gains);
                    self.settings.set_eq_editing(false);
                    if let Some(name) = self.eq_profile.take() {
                        self.save_profile_eq(&name, gains);
                    }
                    self.eq_before = None;
                }
            }
            AppEvent::BrightnessSettled(v) => {
                self.change(Role::Keyboard, |s| s.keyboard.brightness = Some(v));
            }
            AppEvent::TestNotification => self.tray.notify(
                tr("Hubless: notification test"),
                tr("This is what the low battery warning looks like."),
            ),
            AppEvent::Installed => self.tray.notify(
                tr("Hubless is installed"),
                tr("You can delete the downloaded file. Hubless is in the Start menu, and you can remove it in Settings → Apps."),
            ),
            AppEvent::Exit => self.exit(),
        }
    }

    fn on_device(&mut self, role: Role, s: Option<Snapshot>) {
        tracing::info!(?role, ?s, "устройство");
        let kind = match role {
            Role::Mouse => DeviceKind::Mouse,
            Role::Keyboard => DeviceKind::Keyboard,
            Role::Headset => DeviceKind::Headset,
        };
        let had_eq = self.state.headset_eq().is_some();
        // No snapshot: the device (and its receiver or adapter) is gone, so the card disappears.
        match s {
            None => self.state.remove_device(kind),
            Some(s) => {
                let m = self.state.device_entry(kind);
                m.connected = s.connected;
                m.link = s
                    .connected
                    .then_some(if s.wired { Link::Usb } else { Link::Wireless });
                m.battery = s.battery.map(|b| {
                    let charging = b.charging.is_charging() || b.external_power;
                    BatteryState {
                        percent: b.percent,
                        charging,
                        full: b.charging == ChargeState::Full || (charging && b.percent >= 100),
                    }
                });
                m.name = s.name;
                m.controls = s.controls;
                m.error = s.error;
            }
        }
        // The headset returns the saved curve: a write without the "save" flag isn't visible to reads
        // (2026-10-22). Until the edit is saved, we keep it in the state; otherwise the selected preset
        // jumps back to the old one for those seconds.
        if kind == DeviceKind::Headset
            && let Some(gains) = self.eq_gains
        {
            self.set_state_eq(gains);
        }
        // The headset connected (or Hubless started): give it the active profile's curve.
        if kind == DeviceKind::Headset && !had_eq && self.state.headset_eq().is_some() {
            self.apply_profile_eq();
        }
        self.check_charged(kind);
        self.refresh_status();
    }

    /// The "charged" notification: one per charge, and only if the device was charging while Hubless ran,
    /// so it doesn't appear at startup with a device that is already charged on the cable.
    fn check_charged(&mut self, kind: DeviceKind) {
        let battery = self
            .state
            .devices
            .iter()
            .find(|d| d.kind == kind && d.connected)
            .and_then(|d| d.battery);
        match battery {
            Some(b) if b.full => {
                if self.charging_seen.contains(&kind) && !self.charged_notified.contains(&kind) {
                    self.charged_notified.push(kind);
                    let name = self
                        .state
                        .devices
                        .iter()
                        .find(|d| d.kind == kind)
                        .map(|d| d.display_name().to_owned())
                        .unwrap_or_default();
                    tracing::info!(name, "заряжено полностью");
                    if self.config.notify_charged {
                        self.tray.notify(tr("Fully charged"), &name);
                    }
                }
            }
            Some(b) if b.charging => {
                if !self.charging_seen.contains(&kind) {
                    self.charging_seen.push(kind);
                }
            }
            _ => {
                self.charging_seen.retain(|k| *k != kind);
                self.charged_notified.retain(|k| *k != kind);
            }
        }
    }

    /// The result of the update check at startup: a new version gives a notification and the "Download" button
    /// in the "General" section.
    fn startup_update(&mut self, result: Result<crate::update::Release, String>) {
        match result {
            Ok(r) if crate::update::is_newer(&r.version, env!("CARGO_PKG_VERSION")) => {
                tracing::info!(version = r.version, "доступна новая версия");
                self.tray.notify(
                    &tr("Hubless {} is available").replace("{}", &r.version),
                    tr("Download it in Settings → General."),
                );
                self.settings.found_update(r);
            }
            Ok(r) => tracing::info!(latest = r.version, "обновлений нет"),
            Err(e) => tracing::warn!("проверка обновлений при запуске: {e}"),
        }
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
            PanelAction::AddColor(color) => {
                // A new colour goes into the palette and straight onto the keyboard.
                if self.edit_colors(|f| f.add_color(color)) {
                    self.on_panel(PanelAction::Color(color));
                }
            }
            PanelAction::RemoveColor(color) => {
                self.edit_colors(|f| {
                    f.remove_color_value(color);
                    Ok(())
                });
            }
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
            PanelAction::EqPreset(i) => {
                // The "Custom" item at the end of the list isn't a preset: nothing to change.
                let presets = eq_presets::all(&self.state.eq_presets);
                let Some(preset) = presets.get(i.max(0) as usize) else {
                    return;
                };
                let current = self.eq_gains.or_else(|| self.headset_eq());
                if current.is_none_or(|g| g == preset.gains) {
                    return;
                }
                tracing::info!(preset = preset.name, gains = ?preset.gains, "эквалайзер: пресет");
                self.settings.show_eq_gains(&preset.gains);
                self.user_eq_changed(preset.gains, std::time::Duration::ZERO);
            }
            PanelAction::Profile(i) => {
                let mut file = SettingsFile::load();
                let Some(name) = file.names().get(i.max(0) as usize).cloned() else {
                    return;
                };
                if let Err(e) = file.select(&name).and_then(|()| file.save()) {
                    tracing::error!("профиль: {e}");
                    return;
                }
                // Choosing by hand cancels automatic switching until the next window change.
                self.auto_profile = None;
                settings::set_auto_profile(None);
                tracing::info!(profile = name, "профиль выбран вручную");
                self.profile_changed();
            }
        }
    }

    /// The foreground window changed: switch on the profile bound to its exe, or go back to the one chosen by hand.
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

    /// The profile changed: apply its settings and update the panel header.
    fn profile_changed(&mut self) {
        self.devices.send_to(Role::Mouse, Command::Apply);
        self.devices.send_to(Role::Keyboard, Command::Apply);
        self.apply_profile_eq();
        self.refresh_profiles();
    }

    /// Puts the active profile's equalizer curve on the headset (its own, otherwise Default's): on a
    /// profile switch and when the headset connects. A profile without a curve leaves the headset as is.
    fn apply_profile_eq(&mut self) {
        // A pending edit belongs to the previous profile: save it there first.
        if let (Some(name), Some(gains)) = (self.eq_profile.take(), self.eq_gains) {
            self.save_profile_eq(&name, gains);
        }
        self.eq_before = None;
        let file = SettingsFile::load();
        let Some(target) = file
            .eq_for(&file.current_name())
            .and_then(|g| <[i8; crate::headset_eq::BANDS]>::try_from(g).ok())
        else {
            return;
        };
        // No headset: it gets the curve when it connects.
        let Some(current) = self.eq_gains.or_else(|| self.headset_eq()) else {
            return;
        };
        if current == target {
            return;
        }
        tracing::info!(gains = ?target, "эквалайзер: кривая профиля");
        self.settings.show_eq_gains(&target);
        self.eq_changed(target, std::time::Duration::ZERO);
    }

    /// An equalizer change by the user (a preset or the sliders): it also goes into the active profile
    /// when the curve is saved in the headset.
    fn user_eq_changed(
        &mut self,
        gains: [i8; crate::headset_eq::BANDS],
        delay: std::time::Duration,
    ) {
        if self.eq_profile.is_none() {
            self.eq_before = self.headset_eq();
        }
        self.eq_profile = Some(SettingsFile::load().current_name());
        self.eq_changed(gains, delay);
    }

    /// Saves the equalizer curve in profile `name` (see [`SettingsFile::set_eq`]).
    fn save_profile_eq(&mut self, name: &str, gains: [i8; crate::headset_eq::BANDS]) {
        let mut file = SettingsFile::load();
        file.set_eq(name, &gains, self.eq_before.as_ref().map(|g| g.as_slice()));
        match file.save() {
            Ok(()) => tracing::info!(profile = name, ?gains, "эквалайзер: сохранён в профиле"),
            Err(e) => tracing::error!("devices.toml: {e}"),
        }
    }

    /// Rereads the profiles and the panel layout from `devices.toml`.
    fn refresh_profiles(&mut self) {
        let file = SettingsFile::load();
        let colors = file.custom_colors();
        let eq_presets = file.custom_eq_presets();
        if file.panel != self.state.layout
            || colors != self.state.colors
            || eq_presets != self.state.eq_presets
        {
            self.state.layout = file.panel.clone();
            self.state.colors = colors;
            self.state.eq_presets = eq_presets;
            self.refresh_status();
        }
        // Key colours of the active profile (taking automatic switching by window into account) and the layout.
        let key_colors = DeviceSettings::load().keyboard.key_colors();
        let key_layout = crate::keyboard_layout::Layout::parse(
            file.keyboard_layout.as_deref().unwrap_or_default(),
        );
        if key_colors != self.state.key_colors || key_layout != self.state.key_layout {
            self.state.key_colors = key_colors;
            self.state.key_layout = key_layout;
            self.settings.update_devices(&self.state);
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

    /// Changes your own palette colours in `devices.toml` and shows them in the panel.
    fn edit_colors(&mut self, f: impl FnOnce(&mut SettingsFile) -> Result<(), String>) -> bool {
        let mut file = SettingsFile::load();
        if let Err(e) = f(&mut file).and_then(|()| file.save()) {
            tracing::warn!("цвета палитры: {e}");
            return false;
        }
        self.refresh_profiles();
        true
    }

    /// The headset's equalizer curve from the last poll.
    fn headset_eq(&self) -> Option<[i8; crate::headset_eq::BANDS]> {
        self.state.headset_eq()
    }

    /// The headset's equalizer curve in the app state (before the headset replies).
    fn set_state_eq(&mut self, gains: [i8; crate::headset_eq::BANDS]) {
        for d in &mut self.state.devices {
            if let manager::Controls::Headset { eq: Some(eq), .. } = &mut d.controls {
                for ((_, g), new) in eq.iter_mut().zip(gains) {
                    *g = new;
                }
            }
        }
    }

    /// A new equalizer curve: write it after a `delay` pause, and save it in the headset after
    /// [`EQ_SAVE_DELAY`] of quiet (intermediate writes don't save, as with G Hub).
    fn eq_changed(&mut self, gains: [i8; crate::headset_eq::BANDS], delay: std::time::Duration) {
        self.eq_gains = Some(gains);
        self.settings.set_eq_editing(true);
        // The curve goes into the state right away, so the selected preset is visible before the headset replies.
        self.set_state_eq(gains);
        self.refresh_status();
        self.eq_timer
            .start(slint::TimerMode::SingleShot, delay, || {
                post(AppEvent::EqSettled);
            });
        self.eq_save_timer
            .start(slint::TimerMode::SingleShot, EQ_SAVE_DELAY, || {
                post(AppEvent::EqSave);
            });
    }

    /// Key colour (`None` removes the key's own colour): saved in the active profile (the one applied to the
    /// keyboard), shown in the settings window and painted on the keyboard; keys without their own colour get
    /// the effect's colour.
    fn set_keys(
        &mut self,
        keys: &[&'static crate::keyboard_layout::Key],
        rgb: Option<[u8; 3]>,
        base: Option<[u8; 3]>,
    ) {
        let saved = DeviceSettings::update(|s| {
            for k in keys {
                s.keyboard.set_key(k.name, rgb);
            }
        });
        if let Err(e) = saved {
            tracing::error!("devices.toml: {e}");
            self.settings.set_error(&e);
            return;
        }
        for k in keys {
            match rgb {
                Some(c) => {
                    self.state.key_colors.insert(k.name.to_owned(), c);
                }
                None => {
                    self.state.key_colors.remove(k.name);
                }
            }
        }
        if keys.len() > 1 {
            tracing::info!(count = keys.len(), ?rgb, "цвета клавиш");
        } else {
            tracing::debug!(keys = ?keys.iter().map(|k| k.name).collect::<Vec<_>>(), ?rgb, "цвет клавиши");
        }
        self.settings.update_devices(&self.state);
        if let Some(color) = rgb.or(base) {
            let frame = keys.iter().map(|k| (k.zone(), color)).collect();
            self.devices.send_to(Role::Keyboard, Command::Keys(frame));
        }
    }

    /// Changes the order or visibility of the cards in `devices.toml` and shows the result.
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
                // Querying the devices takes seconds, so not on the main thread.
                let spawned = std::thread::Builder::new()
                    .name("device-report".into())
                    .spawn(|| post(AppEvent::ReportReady(report::create())));
                if let Err(e) = spawned {
                    self.settings.report_finished(Err(format!("thread: {e}")));
                }
            }
            SettingsAction::OpenReports => open_folder(&report::reports_dir()),
            SettingsAction::CheckUpdates => {
                self.settings.update_started();
                // The request to GitHub takes seconds, so not on the main thread.
                let spawned = std::thread::Builder::new()
                    .name("update-check".into())
                    .spawn(|| post(AppEvent::UpdateChecked(crate::update::latest())));
                if let Err(e) = spawned {
                    self.settings.update_finished(Err(format!("thread: {e}")));
                }
            }
            SettingsAction::OpenUpdate => {
                let url = self
                    .settings
                    .update_url()
                    .unwrap_or(crate::update::RELEASES_URL)
                    .to_owned();
                open_folder(std::path::Path::new(&url));
            }
            SettingsAction::OpenLogs => open_folder(&log_dir()),
            SettingsAction::DeleteLogs => {
                let message = match crate::logging::clear() {
                    Ok(n) => tr("Log files deleted: {}").replace("{}", &n.to_string()),
                    Err(e) => {
                        tracing::warn!("удаление логов: {e}");
                        tr("Couldn't delete the logs").to_owned()
                    }
                };
                self.settings.set_logs_status(&message);
            }
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
                tracing::info!(enabled = autostart::is_enabled(), "автозапуск");
            }
            SettingsAction::MoveDevice(from, to) => {
                if let Some(order) = self.state.moved(from.max(0) as usize, to.max(0) as usize) {
                    self.edit_layout(|l| l.order = order);
                }
                // The rows already have the new order (or the old one on error): reset the drag.
                self.settings.finish_drop();
            }
            SettingsAction::MouseHost(on) => self.on_panel(PanelAction::MouseHost(on)),
            SettingsAction::LiftOff(v) => self.on_panel(PanelAction::LiftOff(match v {
                0 => settings::LiftOff::Low,
                1 => settings::LiftOff::Medium,
                _ => settings::LiftOff::High,
            })),
            SettingsAction::AutoOff(m) => {
                self.on_panel(PanelAction::AutoOff(m.clamp(0, 255) as u8));
            }
            SettingsAction::EqBand(i, gain) => {
                let Some(mut gains) = self.eq_gains.or_else(|| self.headset_eq()) else {
                    return;
                };
                let Some(slot) = gains.get_mut(i.max(0) as usize) else {
                    return;
                };
                let gain = gain.clamp(
                    i32::from(*crate::headset_eq::GAIN_RANGE.start()),
                    i32::from(*crate::headset_eq::GAIN_RANGE.end()),
                ) as i8;
                // The slider moved within the same step (or it was set from the headset):
                // nothing to write. Without this check, setting the value triggered a new write.
                if *slot == gain {
                    return;
                }
                *slot = gain;
                self.user_eq_changed(gains, SLIDER_DEBOUNCE);
            }
            SettingsAction::EqPreset(i) => self.on_panel(PanelAction::EqPreset(i)),
            SettingsAction::SaveEqPreset(name) => {
                let Some(gains) = self.eq_gains.or_else(|| self.headset_eq()) else {
                    return;
                };
                let mut file = SettingsFile::load();
                let name = name.trim().to_owned();
                if file.eq_presets.len() >= settings::MAX_EQ_PRESETS
                    && !file.eq_presets.iter().any(|p| p.name == name)
                {
                    self.settings.set_error(
                        &tr("No more than {} presets")
                            .replace("{}", &settings::MAX_EQ_PRESETS.to_string()),
                    );
                    return;
                }
                if let Err(e) = file
                    .save_eq_preset(&name, &gains)
                    .and_then(|()| file.save())
                {
                    tracing::warn!("пресет эквалайзера: {e}");
                    self.settings.set_error(tr("Couldn't save the preset"));
                    return;
                }
                tracing::info!(name, ?gains, "эквалайзер: свой пресет сохранён");
                self.settings.set_error("");
                self.refresh_profiles();
            }
            SettingsAction::DeleteEqPreset(i) => {
                let presets = eq_presets::all(&self.state.eq_presets);
                let Some(preset) = presets.get(i.max(0) as usize).filter(|p| p.custom) else {
                    return;
                };
                let mut file = SettingsFile::load();
                file.remove_eq_preset(&preset.name);
                if let Err(e) = file.save() {
                    tracing::warn!("пресет эквалайзера: {e}");
                    return;
                }
                tracing::info!(name = preset.name, "эквалайзер: свой пресет удалён");
                self.refresh_profiles();
            }
            SettingsAction::LightingOff(m) => {
                let m = m.clamp(0, i32::from(u16::MAX)) as u16;
                tracing::info!(minutes = m, "гашение подсветки клавиатуры");
                self.change(Role::Keyboard, |s| s.keyboard.lighting_off_min = Some(m));
            }
            SettingsAction::KeyLayout(i) => {
                let layout = if i == 1 {
                    crate::keyboard_layout::Layout::Iso
                } else {
                    crate::keyboard_layout::Layout::Ansi
                };
                let mut file = SettingsFile::load();
                file.keyboard_layout = (layout != crate::keyboard_layout::Layout::Ansi)
                    .then(|| layout.as_str().into());
                if let Err(e) = file.save() {
                    tracing::error!("devices.toml: {e}");
                    self.settings.set_error(&e);
                    return;
                }
                tracing::info!(layout = layout.as_str(), "раскладка клавиатуры");
                self.refresh_profiles();
            }
            SettingsAction::PaintKey(x, y, rgb) => {
                let Some((keys, base)) = self.state.keyboard_keys() else {
                    return;
                };
                let Some(key) = crate::keyboard_layout::key_at(&keys, x, y) else {
                    return;
                };
                // Dragging over the same key, or the same colour: nothing to write.
                if self.state.key_colors.get(key.name).copied() == rgb {
                    return;
                }
                self.set_keys(&[key], rgb, base);
            }
            SettingsAction::FillKeys(rgb) => {
                let Some((keys, base)) = self.state.keyboard_keys() else {
                    return;
                };
                self.set_keys(&keys, rgb, base);
            }
            SettingsAction::LowBattery(i) => {
                let Some(&percent) = crate::config::LOW_BATTERY_CHOICES.get(i.max(0) as usize)
                else {
                    return;
                };
                if let Err(e) = Config::save_low_battery(percent) {
                    tracing::error!("config.toml: {e}");
                }
                tracing::info!(percent, "порог низкого заряда");
                self.config.low_battery = percent;
                self.state.low_battery = percent;
                self.refresh_status();
            }
            SettingsAction::NotifyCharged(on) => {
                if let Err(e) = Config::save_notify_charged(on) {
                    tracing::error!("config.toml: {e}");
                }
                self.config.notify_charged = on;
            }
            SettingsAction::UpdateOnStart(on) => {
                if let Err(e) = Config::save_check_updates(on) {
                    tracing::error!("config.toml: {e}");
                }
                tracing::info!(on, "проверка обновлений при запуске");
                self.config.check_updates = on;
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
                    // Choosing by hand cancels automatic switching until the next window change.
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

    /// Changes the profiles in `devices.toml`; an error becomes a message in the settings window.
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

    /// Any DPI value: snap it to the nearest one the mouse accepts and apply it (host).
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

    /// Changes `devices.toml` and asks the device thread to apply it.
    fn change(&mut self, role: Role, f: impl FnOnce(&mut DeviceSettings)) {
        if let Err(e) = DeviceSettings::update(f) {
            tracing::error!("devices.toml: {e}");
            return;
        }
        self.devices.send_to(role, Command::Apply);
    }

    /// The tray, the low battery notification and the panel from the current state.
    fn refresh_status(&mut self) {
        let low = self.state.any_low_battery();
        self.tray.set_status(&self.state.tray_lines(), low);
        if low && !self.low_notified {
            let names: Vec<String> = self
                .state
                .low_battery_devices()
                .map(|d| {
                    format!(
                        "{} — {}",
                        d.display_name(),
                        d.battery_text().unwrap_or_default()
                    )
                })
                .collect();
            self.tray.notify(
                &tr("Battery below {}%").replace("{}", &self.state.low_battery.to_string()),
                &names.join("\n"),
            );
        }
        self.low_notified = low;
        self.panel.refresh(&self.state);
        self.settings.update_devices(&self.state);
    }

    fn exit(&mut self) {
        tracing::info!("выход");
        self.panel.destroy();
        let _ = slint::quit_event_loop();
    }
}

fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    let path = wide(path);
    // SAFETY: the strings live until the end of the call.
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
    // ShellExecuteW returns a value > 32 on success.
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
        // Cycle has no colour: choosing a colour makes the effect static.
        let cycle = effect_by_index(3, DEFAULT_COLOR).unwrap();
        assert_eq!(effect_index(Some(cycle.with_color([1, 2, 3]))), 1);
    }
}

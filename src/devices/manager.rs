// SPDX-License-Identifier: GPL-3.0-or-later
//! Device manager: one thread per device (mouse, keyboard, headset), each owning its own
//! HID handle. The headset has its own protocol; its thread is in `headset.rs`.
//!
//! Everything is event-driven: a thread sleeps on the command channel (no device) or on the transport
//! channel (a device is present) until a notification, a command or a timer: the battery every 5 minutes,
//! keyboard lighting sleep. The state goes to the UI through a callback only when it changes.
//!
//! Keyboard: if `devices.toml` sets the lighting, Hubless takes over its control
//! (as Solaar does) and handles idle/sleep itself on `onUserActivity` events; on exit it hands
//! the control back to the keyboard. Colours of single keys (`0x8081`) are applied as a frame over
//! the static effect, after taking over and after every wake-up.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::devices::discovery::{self, Found, TYPE_KEYBOARD, TYPE_MOUSE};
use crate::devices::{keyboard, mouse};
use crate::hidpp::Write;
use crate::hidpp::features::battery::{self, Battery};
use crate::hidpp::features::{brightness, dpi, onboard, per_key, report_rate, rgb_effects};
use crate::hidpp::message::{ERROR_10, ERROR_20, Report};
use crate::hidpp::root::{self, Protocol};
use crate::hidpp::transport::{Event, Inbound};
use crate::settings::DeviceSettings;

const BATTERY_POLL: Duration = Duration::from_secs(5 * 60);
/// WM_DEVICECHANGE arrives in bursts, so we wait for quiet before rescanning.
const RESCAN_DEBOUNCE: Duration = Duration::from_millis(800);

/// Rechecks of a sleeping device after it started waiting (counted from discovery); after that only
/// receiver notifications, no polling.
const SLEEP_RECHECKS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(8),
];

/// The receiver's HID++ 1.0 notification about the link to a device (`sub_id 0x41`);
/// bit 6 of the first parameter means "link not established".
const DEVICE_CONNECTION: u8 = 0x41;
const LINK_NOT_ESTABLISHED: u8 = 0x40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Mouse,
    Keyboard,
    Headset,
}

impl Role {
    fn device_type(self) -> u8 {
        match self {
            Self::Mouse => TYPE_MOUSE,
            Self::Keyboard => TYPE_KEYBOARD,
            Self::Headset => unreachable!("гарнитура — не HID++, у неё свой поток"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    pub connected: bool,
    pub wired: bool,
    pub battery: Option<Battery>,
    /// Current setting values for the panel.
    pub controls: Controls,
    /// The last write error (shown in the card).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Controls {
    #[default]
    None,
    Mouse {
        /// The app applies the settings (host), not the mouse's memory.
        host: bool,
        dpi: Option<u16>,
        /// DPI values for the panel's quick buttons, from those the mouse supports.
        dpi_presets: Vec<u16>,
        /// All DPI values the mouse accepts (ascending), for the slider and the field.
        dpi_supported: Vec<u16>,
        /// Lift-off (`None`: the mouse can't do it, or the value isn't recognized).
        lift_off: Option<crate::settings::LiftOff>,
        /// Polling rate, Hz, and quick buttons from those supported on the current connection.
        rate_hz: Option<u32>,
        rate_presets: Vec<u32>,
        /// There is a host/onboard switch (not for mice with the old features, see
        /// [`mouse::host_mode_supported`]).
        host_mode: bool,
    },
    Keyboard {
        brightness: Option<u16>,
        brightness_range: Option<(u16, u16)>,
        /// The lighting Hubless holds (`None`: the keyboard controls it).
        lighting: Option<crate::settings::Lighting>,
        /// Zones of single keys (`0x8081`, the keyboard's zone map); empty if the feature is missing.
        key_zones: Vec<u8>,
        /// After how many idle minutes the lighting turns off (`0` = never); `None` means the keyboard
        /// controls the lighting.
        lighting_off_min: Option<u16>,
    },
    Headset {
        sidetone: Option<u8>,
        /// Auto power-off, minutes (0 = never).
        auto_off: Option<u8>,
        /// The built-in equalizer: band frequency, Hz, and gain, dB (`None`: not read).
        eq: Option<Vec<(u16, i8)>>,
    },
}

/// An event from a feature the mouse card values depend on: onboard profiles (profile change
/// or DPI by the button), DPI, polling rate. Per Solaar, the mouse sends such events itself.
fn settings_event(dev: &crate::hidpp::Device, r: &Report) -> bool {
    [
        onboard::ID,
        dpi::ID,
        dpi::OLD_ID,
        report_rate::ID,
        report_rate::OLD_ID,
    ]
    .into_iter()
    .any(|id| dev.feature(id).is_some_and(|f| f.index == r.sub_id()))
}

/// Quick DPI and polling rate buttons in the mouse card (section 7 of the plan).
const DPI_PRESETS: [u16; 4] = [400, 800, 1600, 3200];
const RATE_PRESETS: [u32; 4] = [1000, 2000, 4000, 8000];

#[derive(Debug, Clone)]
pub enum Command {
    Rescan,
    /// Applies `devices.toml` now (a change from the panel), without rescanning.
    Apply,
    /// Headset: sidetone 0–100 (stored in the headset itself).
    Sidetone(u8),
    /// Headset: auto power-off, minutes (0 = never).
    AutoOff(u8),
    /// Headset: equalizer band gains, dB; `true` saves them in the headset.
    Eq([i8; crate::headset_eq::BANDS], bool),
    /// Keyboard: repaint keys (the `0x8081` zone and a colour); the settings are already saved.
    Keys(Vec<(u8, [u8; 3])>),
    Quit,
}

type Callback = Arc<dyn Fn(Role, Option<Snapshot>) + Send + Sync>;
type Waker = Arc<Mutex<Option<Sender<Inbound>>>>;

struct WorkerHandle {
    role: Role,
    cmd: Sender<Command>,
    waker: Waker,
    thread: Option<JoinHandle<()>>,
}

pub struct Handle {
    workers: Vec<WorkerHandle>,
}

impl Handle {
    pub fn send(&self, c: Command) {
        for w in &self.workers {
            w.send(c.clone());
        }
    }

    /// A command to one device.
    pub fn send_to(&self, role: Role, c: Command) {
        if let Some(w) = self.workers.iter().find(|w| w.role == role) {
            w.send(c);
        }
    }
}

impl WorkerHandle {
    fn send(&self, c: Command) {
        let _ = self.cmd.send(c);
        if let Some(waker) = self.waker.lock().ok().and_then(|w| w.clone()) {
            let _ = waker.send(Inbound::Wake);
        }
    }
}

impl Drop for Handle {
    /// Stops the threads and waits for them: the keyboard must get its lighting back in time.
    fn drop(&mut self) {
        self.send(Command::Quit);
        for w in &mut self.workers {
            if let Some(t) = w.thread.take() {
                let _ = t.join();
            }
        }
    }
}

pub fn spawn(on_update: impl Fn(Role, Option<Snapshot>) + Send + Sync + 'static) -> Handle {
    let callback: Callback = Arc::new(on_update);
    let mut workers: Vec<WorkerHandle> = [Role::Mouse, Role::Keyboard]
        .into_iter()
        .map(|role| {
            let (cmd, rx) = mpsc::channel();
            let waker: Waker = Arc::default();
            let thread = std::thread::Builder::new()
                .name(format!("device-{role:?}").to_lowercase())
                .spawn({
                    let (waker, callback) = (waker.clone(), callback.clone());
                    move || Worker::new(role, rx, waker, callback).run()
                })
                .expect("spawn device worker");
            WorkerHandle {
                role,
                cmd,
                waker,
                thread: Some(thread),
            }
        })
        .collect();
    workers.push(spawn_headset(callback));
    let handle = Handle { workers };
    handle.send(Command::Rescan);
    handle
}

/// The headset thread: no HID++ transport; it reads commands between adapter reads.
fn spawn_headset(callback: Callback) -> WorkerHandle {
    let (cmd, rx) = mpsc::channel();
    let on_update = Box::new(move |s| callback(Role::Headset, s));
    let thread = std::thread::Builder::new()
        .name("device-headset".into())
        .spawn(move || super::headset::Worker::new(rx, on_update).run())
        .expect("spawn headset worker");
    WorkerHandle {
        role: Role::Headset,
        cmd,
        waker: Arc::default(),
        thread: Some(thread),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LightState {
    Active,
    Idle,
    Sleeping,
}

/// Lighting controlled by Hubless.
struct Lighting {
    /// The effect for each zone that supports it (on the G815, the logo and the keys).
    effects: Vec<Write>,
    /// The frame of single key colours over the effect (empty if there are no colours of your own).
    keys: Vec<Write>,
    /// After how many seconds the keyboard reports idle (the firmware timer).
    idle_s: u16,
    /// After how many idle seconds to turn the lighting off (`None`: never): from the settings, by
    /// default the firmware sleep timer. It can't be earlier than `idle_s`, since we learn about idle from the
    /// keyboard.
    off_after_s: Option<u32>,
    state: LightState,
    sleep_at: Option<Instant>,
}

struct Worker {
    role: Role,
    rx: Receiver<Command>,
    waker: Waker,
    on_update: Callback,
    api: Option<HidApi>,
    dev: Option<Found>,
    snapshot: Option<Snapshot>,
    next_poll: Instant,
    lighting: Option<Lighting>,
    /// The last write error, for the panel card.
    error: Option<String>,
    /// A receiver with a sleeping device (when there is no `dev`).
    sleeping: Option<discovery::Sleeping>,
    /// The single key zone map of the found keyboard (read once).
    key_zones: Option<Vec<u8>>,
    /// When the device started waiting and how many rechecks ([`SLEEP_RECHECKS`]) have been done.
    sleep_since: Instant,
    sleep_rechecks: usize,
}

impl Worker {
    fn new(role: Role, rx: Receiver<Command>, waker: Waker, on_update: Callback) -> Self {
        Self {
            role,
            rx,
            waker,
            on_update,
            api: None,
            dev: None,
            snapshot: None,
            next_poll: Instant::now(),
            lighting: None,
            error: None,
            sleeping: None,
            key_zones: None,
            sleep_since: Instant::now(),
            sleep_rechecks: 0,
        }
    }

    fn run(mut self) {
        tracing::info!(role = ?self.role, "поток устройства запущен");
        loop {
            let quit = if self.dev.is_some() {
                self.wait_device()
            } else if self.sleeping.is_some() {
                self.wait_sleeping()
            } else {
                match self.rx.recv() {
                    Ok(c) => self.command(c),
                    Err(_) => true,
                }
            };
            if quit {
                break;
            }
        }
        self.release_lighting();
        tracing::info!(role = ?self.role, "поток устройства остановлен");
    }

    /// Handles a command; `true` means exit.
    fn command(&mut self, c: Command) -> bool {
        match c {
            Command::Quit => true,
            Command::Apply => {
                self.error = None;
                self.apply_settings();
                self.publish_controls(true);
                false
            }
            Command::Keys(keys) => {
                self.paint_keys(&keys);
                false
            }
            Command::Sidetone(_) | Command::AutoOff(_) | Command::Eq(..) => false,
            Command::Rescan => {
                loop {
                    match self.rx.recv_timeout(RESCAN_DEBOUNCE) {
                        Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => return true,
                        Ok(_) => continue,
                        Err(RecvTimeoutError::Timeout) => break,
                    }
                }
                self.rescan();
                false
            }
        }
    }

    fn rescan(&mut self) {
        // The device is still there: just reread and apply the settings (they may have been changed from the CLI).
        if let Some(d) = &self.dev
            && matches!(root::ping(&d.transport, d.index), Ok(Protocol::V2(..)))
        {
            self.apply_settings();
            self.publish_controls(false);
            return;
        }
        self.drop_device();
        let api = match &mut self.api {
            Some(api) => {
                if let Err(e) = api.refresh_devices() {
                    tracing::warn!("hidapi refresh: {e}");
                }
                api
            }
            None => match HidApi::new() {
                Ok(api) => self.api.insert(api),
                Err(e) => {
                    tracing::error!("hidapi: {e}");
                    return;
                }
            },
        };
        let started = Instant::now();
        let (found, sleeping) = discovery::find_or_sleeping(api, self.role.device_type());
        let found = found.into_iter().next();
        tracing::info!(
            role = ?self.role,
            ms = started.elapsed().as_millis() as u64,
            name = found.as_ref().map(|f| f.name.as_str()),
            sleeping = ?sleeping.as_ref().map(|s| &s.slots),
            "поиск устройства"
        );
        if let Some(s) = &sleeping {
            // The device is asleep: listen to the receiver; when it wakes up, a connection notification arrives.
            *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(s.transport.waker());
        }
        // A sleeping device: a "not connected" card with the name from the receiver (if empty, the panel
        // shows the type name).
        // If the receiver didn't give a name, keep the previous one if the device was already found.
        let known_name = self
            .snapshot
            .as_ref()
            .map(|p| p.name.clone())
            .filter(|n| !n.is_empty());
        let asleep = sleeping.as_ref().map(|s| Snapshot {
            name: s.name.clone().or(known_name).unwrap_or_default(),
            connected: false,
            wired: false,
            battery: None,
            controls: Controls::None,
            error: None,
        });
        if sleeping.is_some() {
            self.sleep_since = Instant::now();
            self.sleep_rechecks = 0;
        }
        self.sleeping = sleeping;
        match found {
            Some(f) => {
                *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(f.transport.waker());
                self.dev = Some(f);
                self.key_zones = None;
                self.apply_settings();
                self.poll_battery();
                self.publish_controls(false);
            }
            None => self.set_snapshot(asleep),
        }
    }

    fn drop_device(&mut self) {
        self.dev = None;
        self.sleeping = None;
        self.lighting = None;
        self.key_zones = None;
        *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The device is asleep behind a receiver: wait for a connection notification for its slot, or a command.
    /// `true` means exit.
    fn wait_sleeping(&mut self) -> bool {
        let Some(s) = &self.sleeping else {
            return false;
        };
        let recheck_at = SLEEP_RECHECKS
            .get(self.sleep_rechecks)
            .map(|&d| self.sleep_since + d);
        let timeout = recheck_at.map_or(Duration::from_secs(3600), |t| {
            t.saturating_duration_since(Instant::now())
        });
        match s.transport.next_event(timeout) {
            Event::Report(r) => {
                let from_slot = s.slots.contains(&r.device());
                // "Link established", or any message from the device itself (not an error reply from the
                // receiver): the connection notification may have come before we started listening.
                let online = from_slot
                    && if r.sub_id() == DEVICE_CONNECTION {
                        r.params()[0] & LINK_NOT_ESTABLISHED == 0
                    } else {
                        !matches!(r.sub_id(), ERROR_10 | ERROR_20)
                    };
                if online {
                    tracing::info!(role = ?self.role, slot = r.device(), "устройство проснулось");
                    self.rescan();
                }
            }
            Event::Timeout if recheck_at.is_some_and(|t| t <= Instant::now()) => {
                // A short series of rechecks after starting to wait: the device may have
                // reconnected (for example, after the cable was unplugged) before we started listening.
                self.sleep_rechecks += 1;
                let awake = s
                    .slots
                    .iter()
                    .any(|&slot| matches!(root::ping(&s.transport, slot), Ok(Protocol::V2(..))));
                if awake {
                    tracing::info!(role = ?self.role, "устройство на связи (перепроверка)");
                    self.rescan();
                }
            }
            Event::Wake => {
                while let Ok(c) = self.rx.try_recv() {
                    if self.command(c) {
                        return true;
                    }
                }
            }
            Event::Closed => {
                tracing::info!(role = ?self.role, "ресивер отключён");
                self.drop_device();
                self.set_snapshot(None);
            }
            Event::Timeout => {}
        }
        false
    }

    /// Waits for a device event; `true` means exit.
    fn wait_device(&mut self) -> bool {
        let Some(d) = &self.dev else { return false };
        let deadline = match self.lighting.as_ref().and_then(|l| l.sleep_at) {
            Some(sleep_at) => sleep_at.min(self.next_poll),
            None => self.next_poll,
        };
        match d
            .transport
            .next_event(deadline.saturating_duration_since(Instant::now()))
        {
            Event::Report(r) => self.on_report(&r),
            Event::Wake => {
                while let Ok(c) = self.rx.try_recv() {
                    if self.command(c) {
                        return true;
                    }
                }
            }
            Event::Closed => {
                tracing::info!(role = ?self.role, "устройство отключено");
                self.drop_device();
                self.set_snapshot(None);
            }
            Event::Timeout => {
                let now = Instant::now();
                if self
                    .lighting
                    .as_ref()
                    .and_then(|l| l.sleep_at)
                    .is_some_and(|t| t <= now)
                {
                    self.lighting_sleep();
                }
                if self.next_poll <= now {
                    self.poll_battery();
                }
            }
        }
        false
    }

    fn on_report(&mut self, r: &Report) {
        let Some(d) = &self.dev else { return };
        let dev = d.device();
        if let Some(b) = battery::from_event(&dev, r) {
            tracing::debug!(role = ?self.role, ?b, "событие батареи");
            self.update(|s| {
                s.battery = Some(b);
                s.connected = true;
            });
        } else if let Some(active) = rgb_effects::activity_event(&dev, r) {
            self.on_activity(active);
        } else if r.device() == d.index && r.address() & 0x0F == 0 && settings_event(&dev, r) {
            // The mouse changed DPI, profile or polling rate by itself (with a button): reread for the panel.
            tracing::debug!(role = ?self.role, ?r, "событие настроек");
            self.publish_controls(false);
        } else if r.sub_id() == DEVICE_CONNECTION && r.device() == d.index {
            let online = r.params()[0] & LINK_NOT_ESTABLISHED == 0;
            tracing::info!(role = ?self.role, online, "связь с ресивером");
            if online {
                self.apply_settings();
                self.poll_battery();
            } else {
                self.update(|s| s.connected = false);
            }
        } else {
            tracing::debug!(role = ?self.role, ?r, "уведомление");
        }
    }

    /// The battery and the link to the device. Without a battery feature (the wired G815, user report
    /// 2026-10-20) the link is checked with a ping: before, the battery read error made such a
    /// device inactive forever.
    fn poll_battery(&mut self) {
        self.next_poll = Instant::now() + BATTERY_POLL;
        let Some(d) = &self.dev else { return };
        let dev = d.device();
        let has_battery =
            dev.feature(battery::ID).is_some() || dev.feature(battery::VOLTAGE_ID).is_some();
        let result = if has_battery {
            battery::status(&dev).map(Some)
        } else {
            root::ping(&d.transport, d.index).map(|_| None)
        };
        let was_offline = self.snapshot.as_ref().is_some_and(|s| !s.connected);
        if result.is_ok() && was_offline {
            // The device woke up/reconnected without a receiver notification.
            self.apply_settings();
        }
        match result {
            Ok(b) => self.update(|s| {
                s.battery = b;
                s.connected = true;
            }),
            // A device behind a receiver may be asleep: that is "offline", not an error.
            Err(e) => {
                tracing::debug!(role = ?self.role, "батарея: {e}");
                self.update(|s| s.connected = false);
            }
        }
    }

    // --- Settings from devices.toml ---

    fn apply(&mut self, w: &Write) -> bool {
        let Some(d) = &self.dev else { return false };
        match d.device().apply(w) {
            Ok(_) => {
                tracing::info!(role = ?self.role, what = %w.what, "применено");
                true
            }
            Err(e) => {
                tracing::warn!(role = ?self.role, what = %w.what, "не применено: {e}");
                self.error = Some(format!("{}: {e}", w.what));
                false
            }
        }
    }

    /// Rereads the current setting values and hands them to the panel.
    /// `force` hands over the snapshot even if it didn't change (the panel rolls back its slider).
    fn publish_controls(&mut self, force: bool) {
        let key_zones = if self.role == Role::Keyboard {
            self.key_zones()
        } else {
            Vec::new()
        };
        let Some(d) = &self.dev else { return };
        let dev = d.device();
        let settings = DeviceSettings::load();
        let controls = match self.role {
            Role::Mouse => {
                let params = dpi::parameters(&dev).ok();
                let lod_supported = dpi::capabilities(&dev).is_ok_and(|c| c.lift_off);
                // The old features (0x2201, 0x8060) are on the G903 and other older mice.
                let old_dpi = mouse::old_dpi(&dev);
                let old_rate = mouse::old_rate(&dev);
                let supported = if old_dpi {
                    dpi::old_supported(&dev).unwrap_or_default()
                } else {
                    dpi::supported(&dev, false).unwrap_or_default()
                };
                let rate_presets = if old_rate {
                    report_rate::old_supported_hz(&dev).unwrap_or_default()
                } else {
                    let supported = report_rate::supported(&dev).unwrap_or_default();
                    RATE_PRESETS
                        .into_iter()
                        .filter(|hz| {
                            report_rate::index_of_hz(*hz).is_some_and(|i| supported.contains(&i))
                        })
                        .collect()
                };
                Controls::Mouse {
                    host: matches!(onboard::mode(&dev), Ok(onboard::Mode::Host)),
                    host_mode: mouse::host_mode_supported(&dev),
                    dpi: if old_dpi {
                        dpi::old_current(&dev).ok()
                    } else {
                        params.as_ref().map(|p| p.x)
                    },
                    lift_off: params
                        .filter(|_| lod_supported)
                        .and_then(|p| mouse::lift_off_from_device(p.lift_off)),
                    rate_hz: if old_rate {
                        report_rate::old_current_hz(&dev).ok()
                    } else {
                        report_rate::current(&dev).ok().and_then(report_rate::hz)
                    },
                    rate_presets,
                    dpi_presets: DPI_PRESETS
                        .into_iter()
                        .filter(|v| supported.contains(v))
                        .collect(),
                    dpi_supported: {
                        let mut all = supported.clone();
                        all.sort_unstable();
                        all.dedup();
                        all
                    },
                }
            }
            Role::Keyboard => Controls::Keyboard {
                brightness: brightness::current(&dev).ok(),
                brightness_range: brightness::info(&dev).ok().map(|i| (i.min, i.max)),
                lighting: self.lighting.as_ref().and(settings.keyboard.lighting),
                key_zones,
                lighting_off_min: self
                    .lighting
                    .as_ref()
                    .map(|l| l.off_after_s.map_or(0, |s| (s / 60).max(1) as u16)),
            },
            Role::Headset => Controls::None,
        };
        let error = self.error.clone();
        let before = self.snapshot.clone();
        self.update(|s| {
            s.controls = controls;
            s.error = error;
        });
        if force && self.snapshot == before {
            // The snapshot didn't change, but the panel may have shown an unapplied value: roll it back.
            (self.on_update)(self.role, self.snapshot.clone());
        }
    }

    /// Applies the settings from `devices.toml` (read again, since the CLI changes it).
    fn apply_settings(&mut self) {
        let settings = DeviceSettings::load();
        match self.role {
            Role::Mouse => {
                let Some(d) = &self.dev else { return };
                let dev = d.device();
                let mut writes = mouse::host_writes(&dev, &settings.mouse);
                // Back to onboard: the settings come from the mouse's memory again.
                if settings.mouse.mode == Some(crate::settings::Mode::Onboard)
                    && mouse::host_mode_supported(&dev)
                    && matches!(onboard::mode(&dev), Ok(onboard::Mode::Host))
                {
                    writes.push(onboard::set_mode(onboard::Mode::Onboard));
                }
                for w in writes {
                    self.apply(&w);
                }
            }
            Role::Keyboard => {
                if let Some(b) = settings.keyboard.brightness {
                    self.apply(&brightness::set(b));
                }
                match settings.keyboard.lighting {
                    Some(_) => self.take_lighting(&settings.keyboard),
                    None => self.release_lighting(),
                }
            }
            Role::Headset => {}
        }
    }

    /// Takes over the lighting control and sets the effect on every zone that supports it.
    /// Previously only zone 0: the PRO X TKL has just one, but on the G815 zone 0 is the logo, and the keys
    /// didn't change (user report 2026-10-20).
    fn take_lighting(&mut self, kb: &crate::settings::KeyboardSettings) {
        let Some(setting) = kb.lighting else { return };
        let zones = self.key_zones();
        let Some(d) = &self.dev else { return };
        let dev = d.device();
        let effects: Vec<Write> = rgb_effects::info(&dev)
            .map(|i| {
                i.zones
                    .iter()
                    .filter_map(|z| rgb_effects::effect_for(z, setting))
                    .collect()
            })
            .unwrap_or_default();
        if effects.is_empty() {
            tracing::warn!("подсветка: зоны или эффект не поддерживаются");
            return;
        }
        let (idle_s, sleep_s) = rgb_effects::timers(&dev).unwrap_or((60, 300));
        if !self.apply(&onboard::set_mode(onboard::Mode::Host)) {
            return;
        }
        // The lighting may have stayed "asleep" (for example, after a crash). Through the receiver the
        // command sometimes gets no reply (verified 2026-10-19), so its failure doesn't stop the takeover.
        let error = self.error.clone();
        self.apply(&rgb_effects::power_on());
        self.error = error;
        // At least one zone must accept the effect; a refusal by a single zone goes to the log and the card.
        let ok = self.apply(&rgb_effects::claim())
            && effects.iter().filter(|w| self.apply(w)).count() > 0;
        if !ok {
            // Don't leave the keyboard in host mode without lighting control.
            self.apply(&rgb_effects::release());
            self.apply(&onboard::set_mode(onboard::Mode::Onboard));
            tracing::warn!("захват подсветки не удался — управление возвращено клавиатуре");
            return;
        }
        // Key colours go after the effect: writing an effect resets the per-key frame.
        let keys = keyboard::key_writes(kb, &zones);
        for w in &keys {
            self.apply(w);
        }
        let off_after_s = match kb.lighting_off_min {
            None => Some(u32::from(sleep_s)),
            Some(0) => None,
            Some(m) => Some(u32::from(m) * 60),
        };
        {
            tracing::info!(
                idle_s,
                ?off_after_s,
                keys = kb.keys.len(),
                "подсветкой управляет Hubless"
            );
            self.lighting = Some(Lighting {
                effects,
                keys,
                idle_s,
                off_after_s,
                state: LightState::Active,
                sleep_at: None,
            });
        }
    }

    /// Hands the lighting control back to the keyboard (if we have it).
    fn release_lighting(&mut self) {
        if self.lighting.take().is_some() {
            self.apply(&rgb_effects::release());
            self.apply(&onboard::set_mode(onboard::Mode::Onboard));
            tracing::info!("подсветка возвращена клавиатуре");
        }
    }

    /// The `onUserActivity` event from the keyboard. The firmware sends a burst of identical events,
    /// so we react only to state changes.
    ///
    /// Idle (the keyboard reports it after `idle_s`) only starts a timer: the lighting turns off
    /// `off_after_s` after the last key press (`idle_mode` and sleep together, as with the firmware
    /// timers 300/300, verified 2026-10-19). A key press before that sends nothing; after sleep it's
    /// wake, takeover, effect and key colours.
    fn on_activity(&mut self, active: bool) {
        let Some(l) = &self.lighting else { return };
        let (state, idle_s, off_after_s) = (l.state, l.idle_s, l.off_after_s);
        let writes: Vec<Write> = l.effects.iter().chain(&l.keys).cloned().collect();
        if !active && state == LightState::Active {
            let Some(off_after_s) = off_after_s else {
                tracing::debug!("подсветка: простой, гашение выключено");
                return;
            };
            let delay =
                Duration::from_secs(u64::from(off_after_s.saturating_sub(u32::from(idle_s))));
            tracing::debug!(?delay, "подсветка: простой");
            if let Some(l) = &mut self.lighting {
                l.state = LightState::Idle;
                l.sleep_at = Some(Instant::now() + delay);
            }
            if delay.is_zero() {
                self.lighting_sleep();
            }
        } else if active && state != LightState::Active {
            tracing::debug!("подсветка: активность");
            if state == LightState::Sleeping {
                self.apply(&rgb_effects::power_on());
                self.apply(&rgb_effects::claim());
                for w in &writes {
                    self.apply(w);
                }
            }
            if let Some(l) = &mut self.lighting {
                l.state = LightState::Active;
                l.sleep_at = None;
            }
        }
    }

    /// The single key zone map (read once per found keyboard; a read error
    /// isn't remembered, so we read again next time).
    fn key_zones(&mut self) -> Vec<u8> {
        if let Some(zones) = &self.key_zones {
            return zones.clone();
        }
        let Some(d) = &self.dev else {
            return Vec::new();
        };
        let dev = d.device();
        if dev.feature(per_key::ID).is_none() {
            self.key_zones = Some(Vec::new());
            return Vec::new();
        }
        match per_key::supported_zones(&dev) {
            Ok(zones) => {
                tracing::info!(count = zones.len(), "зоны отдельных клавиш");
                self.key_zones = Some(zones.clone());
                zones
            }
            Err(e) => {
                tracing::warn!("карта зон клавиш: {e}");
                Vec::new()
            }
        }
    }

    /// Repaints keys while Hubless holds the lighting with the static effect (or with it off).
    /// The frame for waking up is rebuilt from the saved settings.
    fn paint_keys(&mut self, keys: &[(u8, [u8; 3])]) {
        let settings = DeviceSettings::load();
        if self.lighting.is_none() || keyboard::key_base(settings.keyboard.lighting).is_none() {
            return;
        }
        let zones = self.key_zones();
        let frame = keyboard::key_writes(&settings.keyboard, &zones);
        if let Some(l) = &mut self.lighting {
            l.keys = frame;
        }
        let keys: Vec<(u8, [u8; 3])> = keys
            .iter()
            .copied()
            .filter(|(zone, _)| zones.contains(zone))
            .collect();
        for w in per_key::frame(&keys) {
            self.apply(&w);
        }
    }

    fn lighting_sleep(&mut self) {
        if self
            .lighting
            .as_ref()
            .is_some_and(|l| l.state == LightState::Idle)
        {
            tracing::debug!("подсветка: сон");
            self.apply(&rgb_effects::idle_mode());
            self.apply(&rgb_effects::power_off());
        }
        if let Some(l) = &mut self.lighting {
            l.state = LightState::Sleeping;
            l.sleep_at = None;
        }
    }

    // --- Snapshot for the UI ---

    fn update(&mut self, f: impl FnOnce(&mut Snapshot)) {
        let Some(d) = &self.dev else { return };
        let mut s = self.snapshot.clone().unwrap_or(Snapshot {
            name: d.name.clone(),
            connected: true,
            wired: !d.via_receiver(),
            battery: None,
            controls: Controls::None,
            error: None,
        });
        s.name.clone_from(&d.name);
        s.wired = !d.via_receiver();
        f(&mut s);
        self.set_snapshot(Some(s));
    }

    fn set_snapshot(&mut self, s: Option<Snapshot>) {
        if self.snapshot != s {
            self.snapshot.clone_from(&s);
            (self.on_update)(self.role, s);
        }
    }
}

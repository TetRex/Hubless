// SPDX-License-Identifier: GPL-3.0-or-later
//! Device manager: по потоку на устройство (мышь, клавиатура, гарнитура), каждый владеет своим
//! HID-handle. Гарнитура — свой протокол, её поток в `headset.rs`.
//!
//! Всё событийно: поток спит на канале команд (нет устройства) или на канале транспорта
//! (есть устройство) до уведомления, команды или таймера — батарея раз в 5 минут,
//! сон подсветки клавиатуры. Состояние отдаётся в UI через колбэк только при изменении.
//!
//! Клавиатура: если в `devices.toml` задана подсветка, Hubless забирает управление ею
//! (как Solaar) и сам ведёт простой/сон по событиям `onUserActivity`; при выходе возвращает
//! управление клавиатуре.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::devices::discovery::{self, Found, TYPE_KEYBOARD, TYPE_MOUSE};
use crate::devices::mouse;
use crate::hidpp::Write;
use crate::hidpp::features::battery::{self, Battery};
use crate::hidpp::features::{brightness, dpi, onboard, rgb_effects};
use crate::hidpp::message::Report;
use crate::hidpp::root::{self, Protocol};
use crate::hidpp::transport::{Event, Inbound};
use crate::settings::DeviceSettings;

const BATTERY_POLL: Duration = Duration::from_secs(5 * 60);
/// WM_DEVICECHANGE приходит пачкой — ждём тишины перед пересканированием.
const RESCAN_DEBOUNCE: Duration = Duration::from_millis(800);

/// HID++ 1.0 уведомление ресивера о связи с устройством (`sub_id 0x41`);
/// бит 6 первого параметра — «связь не установлена».
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
    /// Текущие значения настроек для панели.
    pub controls: Controls,
    /// Последняя ошибка записи (показывается в карточке).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Controls {
    #[default]
    None,
    Mouse {
        /// Настройки применяет приложение (host), а не память мыши.
        host: bool,
        dpi: Option<u16>,
        /// DPI для быстрых кнопок панели — из поддерживаемых мышью.
        dpi_presets: Vec<u16>,
        /// Lift-off (`None` — мышь не умеет или значение не распознано).
        lift_off: Option<crate::settings::LiftOff>,
    },
    Keyboard {
        brightness: Option<u16>,
        brightness_range: Option<(u16, u16)>,
        /// Подсветка, которую держит Hubless (`None` — управляет клавиатура).
        lighting: Option<crate::settings::Lighting>,
    },
    Headset {
        sidetone: Option<u8>,
        /// Автоотключение, минуты (0 — никогда).
        auto_off: Option<u8>,
    },
}

/// Быстрые кнопки DPI в карточке мыши (раздел 7 плана).
const DPI_PRESETS: [u16; 4] = [400, 800, 1600, 3200];

#[derive(Debug, Clone, Copy)]
pub enum Command {
    Rescan,
    /// Применить `devices.toml` сейчас (изменение из панели), без пересканирования.
    Apply,
    /// Гарнитура: sidetone 0–100 (хранится в самой гарнитуре).
    Sidetone(u8),
    /// Гарнитура: автоотключение, минуты (0 — никогда).
    AutoOff(u8),
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
            w.send(c);
        }
    }

    /// Команда одному устройству.
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
    /// Остановить потоки и дождаться их: клавиатура должна успеть вернуть себе подсветку.
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

/// Поток гарнитуры: без HID++ транспорта, команды читает между чтениями адаптера.
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

/// Подсветка, которой управляет Hubless.
struct Lighting {
    effect: Write,
    idle_s: u16,
    sleep_s: u16,
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
    /// Ошибка последней записи — для карточки в панели.
    error: Option<String>,
    /// Ресивер со спящим устройством (когда `dev` нет).
    sleeping: Option<discovery::Sleeping>,
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

    /// Обработать команду; `true` — выход.
    fn command(&mut self, c: Command) -> bool {
        match c {
            Command::Quit => true,
            Command::Apply => {
                self.error = None;
                self.apply_settings();
                self.publish_controls(true);
                false
            }
            Command::Sidetone(_) | Command::AutoOff(_) => false,
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
        // Устройство на месте — только перечитать и применить настройки (их могли поменять из CLI).
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
            // Устройство спит: слушаем ресивер, проснётся — придёт уведомление о связи.
            *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(s.transport.waker());
        }
        self.sleeping = sleeping;
        match found {
            Some(f) => {
                *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(f.transport.waker());
                self.dev = Some(f);
                self.apply_settings();
                self.poll_battery();
                self.publish_controls(false);
            }
            None => self.set_snapshot(None),
        }
    }

    fn drop_device(&mut self) {
        self.dev = None;
        self.sleeping = None;
        self.lighting = None;
        *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Устройство спит за ресивером: ждать уведомления о связи для его слота или команды.
    /// `true` — выход.
    fn wait_sleeping(&mut self) -> bool {
        let Some(s) = &self.sleeping else {
            return false;
        };
        match s.transport.next_event(Duration::from_secs(3600)) {
            Event::Report(r) => {
                let online = r.sub_id() == DEVICE_CONNECTION
                    && s.slots.contains(&r.device())
                    && r.params()[0] & LINK_NOT_ESTABLISHED == 0;
                if online {
                    tracing::info!(role = ?self.role, slot = r.device(), "устройство проснулось");
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
            }
            Event::Timeout => {}
        }
        false
    }

    /// Ждать события устройства; `true` — выход.
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

    fn poll_battery(&mut self) {
        self.next_poll = Instant::now() + BATTERY_POLL;
        let Some(d) = &self.dev else { return };
        let result = battery::status(&d.device());
        let was_offline = self.snapshot.as_ref().is_some_and(|s| !s.connected);
        if result.is_ok() && was_offline {
            // Устройство проснулось/переподключилось без уведомления ресивера.
            self.apply_settings();
        }
        match result {
            Ok(b) => self.update(|s| {
                s.battery = Some(b);
                s.connected = true;
            }),
            // Устройство за ресивером может спать — это «offline», а не ошибка.
            Err(e) => {
                tracing::debug!(role = ?self.role, "батарея: {e}");
                self.update(|s| s.connected = false);
            }
        }
    }

    // --- Настройки из devices.toml ---

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

    /// Перечитать текущие значения настроек и отдать их в панель.
    /// `force` — отдать снимок, даже если он не изменился (панель откатит свой ползунок).
    fn publish_controls(&mut self, force: bool) {
        let Some(d) = &self.dev else { return };
        let dev = d.device();
        let settings = DeviceSettings::load();
        let controls = match self.role {
            Role::Mouse => {
                let supported = dpi::supported(&dev, false).unwrap_or_default();
                let params = dpi::parameters(&dev).ok();
                let lod_supported = dpi::capabilities(&dev).is_ok_and(|c| c.lift_off);
                Controls::Mouse {
                    host: matches!(onboard::mode(&dev), Ok(onboard::Mode::Host)),
                    dpi: params.as_ref().map(|p| p.x),
                    lift_off: params
                        .filter(|_| lod_supported)
                        .and_then(|p| mouse::lift_off_from_device(p.lift_off)),
                    dpi_presets: DPI_PRESETS
                        .into_iter()
                        .filter(|v| supported.contains(v))
                        .collect(),
                }
            }
            Role::Keyboard => Controls::Keyboard {
                brightness: brightness::current(&dev).ok(),
                brightness_range: brightness::info(&dev).ok().map(|i| (i.min, i.max)),
                lighting: self.lighting.as_ref().and(settings.keyboard.lighting),
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
            // Снимок не изменился, но панель могла показать неприменённое значение — откатить.
            (self.on_update)(self.role, self.snapshot.clone());
        }
    }

    /// Применить настройки из `devices.toml` (читается заново — его меняет CLI).
    fn apply_settings(&mut self) {
        let settings = DeviceSettings::load();
        match self.role {
            Role::Mouse => {
                let Some(d) = &self.dev else { return };
                let dev = d.device();
                let mut writes = mouse::host_writes(&dev, &settings.mouse);
                // Возврат к onboard: настройки снова берутся из памяти мыши.
                if settings.mouse.mode == Some(crate::settings::Mode::Onboard)
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
                    Some(l) => self.take_lighting(l),
                    None => self.release_lighting(),
                }
            }
            Role::Headset => {}
        }
    }

    /// Забрать управление подсветкой и задать эффект.
    fn take_lighting(&mut self, setting: crate::settings::Lighting) {
        let Some(d) = &self.dev else { return };
        let dev = d.device();
        let effect = match rgb_effects::info(&dev)
            .ok()
            .and_then(|i| i.zones.into_iter().next())
            .and_then(|z| rgb_effects::effect_for(&z, setting))
        {
            Some(w) => w,
            None => {
                tracing::warn!("подсветка: зона или эффект не поддерживаются");
                return;
            }
        };
        let (idle_s, sleep_s) = rgb_effects::timers(&dev).unwrap_or((60, 300));
        if !self.apply(&onboard::set_mode(onboard::Mode::Host)) {
            return;
        }
        // Подсветка могла остаться «спящей» (например, после аварийного выхода). Через ресивер
        // команда бывает без ответа (проверено 2026-10-19), поэтому её неудача не прерывает захват.
        let error = self.error.clone();
        self.apply(&rgb_effects::power_on());
        self.error = error;
        let ok = self.apply(&rgb_effects::claim()) && self.apply(&effect);
        if !ok {
            // Не оставлять клавиатуру в режиме host без управления подсветкой.
            self.apply(&rgb_effects::release());
            self.apply(&onboard::set_mode(onboard::Mode::Onboard));
            tracing::warn!("захват подсветки не удался — управление возвращено клавиатуре");
            return;
        }
        {
            tracing::info!(idle_s, sleep_s, "подсветкой управляет Hubless");
            self.lighting = Some(Lighting {
                effect,
                idle_s,
                sleep_s,
                state: LightState::Active,
                sleep_at: None,
            });
        }
    }

    /// Вернуть управление подсветкой клавиатуре (если оно у нас).
    fn release_lighting(&mut self) {
        if self.lighting.take().is_some() {
            self.apply(&rgb_effects::release());
            self.apply(&onboard::set_mode(onboard::Mode::Onboard));
            tracing::info!("подсветка возвращена клавиатуре");
        }
    }

    /// Событие `onUserActivity` от клавиатуры. Прошивка шлёт пачку одинаковых событий —
    /// реагируем только на смену состояния.
    fn on_activity(&mut self, active: bool) {
        let Some(l) = &self.lighting else { return };
        let (state, idle_s, sleep_s, effect) = (l.state, l.idle_s, l.sleep_s, l.effect.clone());
        if !active && state == LightState::Active {
            tracing::debug!("подсветка: простой");
            self.apply(&rgb_effects::idle_mode());
            let delay = Duration::from_secs(u64::from(sleep_s.saturating_sub(idle_s)));
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
            }
            self.apply(&rgb_effects::claim());
            self.apply(&effect);
            if let Some(l) = &mut self.lighting {
                l.state = LightState::Active;
                l.sleep_at = None;
            }
        }
    }

    fn lighting_sleep(&mut self) {
        if self
            .lighting
            .as_ref()
            .is_some_and(|l| l.state == LightState::Idle)
        {
            tracing::debug!("подсветка: сон");
            self.apply(&rgb_effects::power_off());
        }
        if let Some(l) = &mut self.lighting {
            l.state = LightState::Sleeping;
            l.sleep_at = None;
        }
    }

    // --- Снимок для UI ---

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

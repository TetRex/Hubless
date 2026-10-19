// SPDX-License-Identifier: GPL-3.0-or-later
//! Device manager: отдельный поток, владеющий HID-устройствами.
//!
//! Всё событийно: поток спит на канале команд (нет устройства) или на канале транспорта
//! (есть устройство) до уведомления, команды или таймера батареи (раз в 5 минут).
//! Состояние отдаётся в UI через колбэк, только когда оно изменилось.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::devices::discovery::{self, Found, TYPE_MOUSE};
use crate::hidpp::features::battery::{self, Battery};
use crate::hidpp::message::Report;
use crate::hidpp::transport::{Event, Inbound};

const BATTERY_POLL: Duration = Duration::from_secs(5 * 60);
/// WM_DEVICECHANGE приходит пачкой — ждём тишины перед пересканированием.
const RESCAN_DEBOUNCE: Duration = Duration::from_millis(800);

/// HID++ 1.0 уведомление ресивера о связи с устройством (`sub_id 0x41`);
/// бит 6 первого параметра — «связь не установлена».
const DEVICE_CONNECTION: u8 = 0x41;
const LINK_NOT_ESTABLISHED: u8 = 0x40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MouseSnapshot {
    pub name: String,
    pub connected: bool,
    pub wired: bool,
    pub battery: Option<Battery>,
}

pub enum Command {
    Rescan,
    Quit,
}

pub struct Handle {
    cmd: Sender<Command>,
    waker: Arc<Mutex<Option<Sender<Inbound>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Handle {
    pub fn send(&self, c: Command) {
        let _ = self.cmd.send(c);
        if let Some(w) = self.waker.lock().ok().and_then(|w| w.clone()) {
            let _ = w.send(Inbound::Wake);
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn spawn(on_mouse: impl Fn(Option<MouseSnapshot>) + Send + 'static) -> Handle {
    let (cmd, rx) = mpsc::channel();
    let waker = Arc::new(Mutex::new(None));
    let thread = std::thread::Builder::new()
        .name("device-manager".into())
        .spawn({
            let waker = waker.clone();
            move || Manager::new(rx, waker, Box::new(on_mouse)).run()
        })
        .expect("spawn device manager");
    let handle = Handle {
        cmd,
        waker,
        thread: Some(thread),
    };
    handle.send(Command::Rescan);
    handle
}

struct Manager {
    rx: Receiver<Command>,
    waker: Arc<Mutex<Option<Sender<Inbound>>>>,
    on_mouse: Box<dyn Fn(Option<MouseSnapshot>) + Send>,
    api: Option<HidApi>,
    mouse: Option<Found>,
    snapshot: Option<MouseSnapshot>,
    next_poll: Instant,
}

impl Manager {
    fn new(
        rx: Receiver<Command>,
        waker: Arc<Mutex<Option<Sender<Inbound>>>>,
        on_mouse: Box<dyn Fn(Option<MouseSnapshot>) + Send>,
    ) -> Self {
        Self {
            rx,
            waker,
            on_mouse,
            api: None,
            mouse: None,
            snapshot: None,
            next_poll: Instant::now(),
        }
    }

    fn run(mut self) {
        tracing::info!("device manager запущен");
        loop {
            let quit = if self.mouse.is_some() {
                self.wait_device()
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
        self.set_mouse(None);
        tracing::info!("device manager остановлен");
    }

    /// Обработать команду; `true` — выход.
    fn command(&mut self, c: Command) -> bool {
        match c {
            Command::Quit => true,
            Command::Rescan => {
                // Дождаться конца пачки hotplug-событий.
                loop {
                    match self.rx.recv_timeout(RESCAN_DEBOUNCE) {
                        Ok(Command::Quit) | Err(RecvTimeoutError::Disconnected) => return true,
                        Ok(Command::Rescan) => continue,
                        Err(RecvTimeoutError::Timeout) => break,
                    }
                }
                self.rescan();
                false
            }
        }
    }

    fn rescan(&mut self) {
        self.mouse = None;
        *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = None;
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
        let found = discovery::find(api, TYPE_MOUSE).into_iter().next();
        tracing::info!(
            ms = started.elapsed().as_millis() as u64,
            mouse = found.as_ref().map(|f| f.name.as_str()),
            "поиск устройств"
        );
        match found {
            Some(f) => {
                *self.waker.lock().unwrap_or_else(|e| e.into_inner()) = Some(f.transport.waker());
                self.mouse = Some(f);
                self.poll_battery();
            }
            None => self.set_mouse(None),
        }
    }

    /// Ждать события устройства; `true` — выход.
    fn wait_device(&mut self) -> bool {
        let Some(m) = &self.mouse else { return false };
        let timeout = self.next_poll.saturating_duration_since(Instant::now());
        match m.transport.next_event(timeout) {
            Event::Report(r) => self.on_report(&r),
            Event::Wake => {
                while let Ok(c) = self.rx.try_recv() {
                    if self.command(c) {
                        return true;
                    }
                }
            }
            Event::Closed => {
                tracing::info!("мышь отключена");
                self.mouse = None;
                self.set_mouse(None);
            }
            Event::Timeout => self.poll_battery(),
        }
        false
    }

    fn on_report(&mut self, r: &Report) {
        let Some(m) = &self.mouse else { return };
        let dev = m.device();
        if let Some(b) = battery::from_event(&dev, r) {
            tracing::debug!(?b, "событие батареи");
            self.update(|s| {
                s.battery = Some(b);
                s.connected = true;
            });
        } else if r.sub_id() == DEVICE_CONNECTION && r.device() == m.index {
            let online = r.params()[0] & LINK_NOT_ESTABLISHED == 0;
            tracing::info!(online, "связь мыши с ресивером");
            if online {
                self.poll_battery();
            } else {
                self.update(|s| s.connected = false);
            }
        } else {
            tracing::debug!(?r, "уведомление");
        }
    }

    fn poll_battery(&mut self) {
        self.next_poll = Instant::now() + BATTERY_POLL;
        let Some(m) = &self.mouse else { return };
        let result = battery::status(&m.device());
        match result {
            Ok(b) => self.update(|s| {
                s.battery = Some(b);
                s.connected = true;
            }),
            // Устройство за ресивером может спать — это «offline», а не ошибка.
            Err(e) => {
                tracing::debug!("батарея: {e}");
                self.update(|s| s.connected = false);
            }
        }
    }

    fn update(&mut self, f: impl FnOnce(&mut MouseSnapshot)) {
        let Some(m) = &self.mouse else { return };
        let mut s = self.snapshot.clone().unwrap_or(MouseSnapshot {
            name: m.name.clone(),
            connected: true,
            wired: !m.via_receiver(),
            battery: None,
        });
        s.name.clone_from(&m.name);
        s.wired = !m.via_receiver();
        f(&mut s);
        self.set_mouse(Some(s));
    }

    fn set_mouse(&mut self, s: Option<MouseSnapshot>) {
        if self.snapshot != s {
            self.snapshot.clone_from(&s);
            (self.on_mouse)(s);
        }
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Поток гарнитуры (PRO X 2 LIGHTSPEED, протокол Centurion — см. `centurion.rs`).
//!
//! Адаптер держит один HID-handle, поэтому чтение событий и запросы идут из одного потока:
//! блокирующее чтение с таймаутом 1 с (как у читателей HID++ транспорта), между чтениями —
//! команды менеджера. Подключение/отключение гарнитуры приходит событием моста,
//! батарея опрашивается раз в 5 минут. Sidetone и автоотключение хранит сама гарнитура —
//! панель шлёт их командами сюда, без `devices.toml`.

use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::centurion::{self, Adapter};
use crate::devices::manager::{Command, Controls, Role, Snapshot};
use crate::hidpp::features::battery::{Battery, ChargeState};

const BATTERY_POLL: Duration = Duration::from_secs(5 * 60);
const READ_SLICE: Duration = Duration::from_secs(1);
const RESCAN_DEBOUNCE: Duration = Duration::from_millis(800);

pub(super) struct Worker {
    rx: Receiver<Command>,
    on_update: Box<dyn Fn(Option<Snapshot>) + Send>,
    api: Option<HidApi>,
    adapter: Option<Adapter>,
    /// Имя гарнитуры — строка продукта адаптера.
    name: String,
    snapshot: Option<Snapshot>,
    next_poll: Instant,
    /// Ошибка последней записи из панели.
    error: Option<String>,
}

impl Worker {
    pub(super) fn new(
        rx: Receiver<Command>,
        on_update: Box<dyn Fn(Option<Snapshot>) + Send>,
    ) -> Self {
        Self {
            rx,
            on_update,
            api: None,
            adapter: None,
            name: String::new(),
            snapshot: None,
            next_poll: Instant::now(),
            error: None,
        }
    }

    pub(super) fn run(mut self) {
        tracing::info!(role = ?Role::Headset, "поток устройства запущен");
        loop {
            let quit = if self.adapter.is_some() {
                self.wait_adapter()
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
        tracing::info!(role = ?Role::Headset, "поток устройства остановлен");
    }

    /// Обработать команду; `true` — выход.
    fn command(&mut self, c: Command) -> bool {
        match c {
            Command::Quit => true,
            Command::Apply => false,
            Command::Sidetone(v) => {
                self.write(|a| {
                    let state = a.sidetone()?;
                    Ok(centurion::set_sidetone(&state, v))
                });
                false
            }
            Command::AutoOff(m) => {
                self.write(|_| Ok(centurion::set_auto_sleep(m)));
                false
            }
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
        // Адаптер на месте — ничего не делать.
        if let Some(a) = &mut self.adapter
            && a.bridge_status().is_ok()
        {
            return;
        }
        self.adapter = None;
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
        let found = Adapter::find(api);
        tracing::info!(
            name = found.as_ref().map(|(_, n)| n.as_str()),
            "поиск адаптера гарнитуры"
        );
        if let Some((adapter, name)) = found {
            self.adapter = Some(adapter);
            self.name = name;
        }
        if self.adapter.is_some() {
            self.poll();
        } else {
            self.set_snapshot(None);
        }
    }

    /// Ждать кадр адаптера или команду; `true` — выход.
    fn wait_adapter(&mut self) -> bool {
        loop {
            match self.rx.try_recv() {
                Ok(c) => {
                    if self.command(c) {
                        return true;
                    }
                    if self.adapter.is_none() {
                        return false;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return true,
            }
        }
        let Some(a) = &mut self.adapter else {
            return false;
        };
        let left = self.next_poll.saturating_duration_since(Instant::now());
        match a.next_event(left.min(READ_SLICE)) {
            Ok(Some(p)) => {
                if let Some(connected) = a.connection_event(&p) {
                    tracing::info!(connected, "гарнитура: событие моста");
                    if connected {
                        self.poll();
                    } else {
                        self.set_offline();
                    }
                } else {
                    tracing::debug!(p = %crate::hidpp::message::hex(&p), "гарнитура: событие");
                }
            }
            Ok(None) => {
                if self.next_poll <= Instant::now() {
                    self.poll();
                }
            }
            Err(e) => {
                tracing::info!("адаптер гарнитуры отключён: {e}");
                self.adapter = None;
                self.set_snapshot(None);
            }
        }
        false
    }

    /// Запись из панели: отправить, затем перечитать состояние (панель откатит неприменённое).
    fn write(
        &mut self,
        build: impl FnOnce(&mut Adapter) -> Result<centurion::Write, centurion::Error>,
    ) {
        let Some(a) = &mut self.adapter else { return };
        let result = build(a).and_then(|w| a.apply(&w).map(|_| w));
        self.error = match result {
            Ok(w) => {
                tracing::info!(what = %w.what, "гарнитура: применено");
                None
            }
            Err(e) => {
                tracing::warn!("гарнитура: не применено: {e}");
                Some(format!("не применено: {e}"))
            }
        };
        let before = self.snapshot.clone();
        self.poll();
        if self.snapshot == before {
            (self.on_update)(self.snapshot.clone());
        }
    }

    /// Прочитать батарею и настройки; гарнитура выключена — карточка «не подключено».
    fn poll(&mut self) {
        self.next_poll = Instant::now() + BATTERY_POLL;
        let Some(a) = &mut self.adapter else { return };
        let battery = a.battery();
        let controls = Controls::Headset {
            sidetone: a.sidetone_level().ok(),
            auto_off: a.auto_sleep_minutes().ok(),
        };
        let error = self.error.clone();
        match battery {
            Ok(b) => self.set_snapshot(Some(Snapshot {
                name: self.name.clone(),
                connected: true,
                wired: false,
                battery: Some(Battery {
                    percent: b.percent,
                    level: 0,
                    charging: if matches!(b.charging_state, 1 | 2) {
                        ChargeState::Charging
                    } else {
                        ChargeState::Discharging
                    },
                    external_power: false,
                }),
                controls,
                error,
            })),
            Err(centurion::Error::Offline) => self.set_offline(),
            Err(e) => {
                tracing::debug!("гарнитура: батарея: {e}");
                self.set_offline();
            }
        }
    }

    fn set_offline(&mut self) {
        self.set_snapshot(Some(Snapshot {
            name: self.name.clone(),
            connected: false,
            wired: false,
            battery: None,
            controls: Default::default(),
            error: None,
        }));
    }

    fn set_snapshot(&mut self, s: Option<Snapshot>) {
        if self.snapshot != s {
            self.snapshot.clone_from(&s);
            (self.on_update)(s);
        }
    }
}

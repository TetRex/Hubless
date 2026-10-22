// SPDX-License-Identifier: GPL-3.0-or-later
//! Headset thread (PRO X 2 LIGHTSPEED, Centurion protocol; see `centurion.rs`).
//!
//! The adapter has a single HID handle, so reading events and sending requests happen on one thread:
//! a blocking read with a 1 s timeout (like the HID++ transport readers), and manager commands
//! between reads. The headset connecting/disconnecting arrives as a bridge event; the battery is
//! polled every 5 minutes. Sidetone and auto power-off are stored in the headset itself, so the
//! panel sends them here as commands, without `devices.toml`.

use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant};

use hidapi::HidApi;

use crate::centurion::{self, Adapter};
use crate::devices::manager::{Command, Controls, Role, Snapshot};
use crate::headset_eq;
use crate::hidpp::features::battery::{Battery, ChargeState};

const BATTERY_POLL: Duration = Duration::from_secs(5 * 60);
const READ_SLICE: Duration = Duration::from_secs(1);
const RESCAN_DEBOUNCE: Duration = Duration::from_millis(800);

pub(super) struct Worker {
    rx: Receiver<Command>,
    on_update: Box<dyn Fn(Option<Snapshot>) + Send>,
    api: Option<HidApi>,
    adapter: Option<Adapter>,
    /// The headset name is the adapter's product string.
    name: String,
    snapshot: Option<Snapshot>,
    next_poll: Instant,
    /// The error of the last write from the panel.
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

    /// Handles a command; `true` means exit.
    fn command(&mut self, c: Command) -> bool {
        match c {
            Command::Quit => true,
            Command::Apply | Command::Keys(_) => false,
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
            Command::Eq(gains, save) => {
                self.write(|a| {
                    // The band frequencies are the ones the headset reports.
                    let eq = a.eq()?;
                    if eq.bands.len() != headset_eq::BANDS {
                        return Err(centurion::Error::Malformed);
                    }
                    let bands = std::array::from_fn(|i| headset_eq::Band {
                        freq: eq.bands[i].freq,
                        gain: gains[i],
                    });
                    Ok(headset_eq::write(&bands, save))
                });
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
        // The adapter is still there: nothing to do.
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

    /// Waits for an adapter frame or a command; `true` means exit.
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

    /// A write from the panel: send it, then reread the state (the panel rolls back what wasn't applied).
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

    /// Reads the battery and settings; if the headset is off, the card shows "not connected".
    fn poll(&mut self) {
        self.next_poll = Instant::now() + BATTERY_POLL;
        let Some(a) = &mut self.adapter else { return };
        let battery = a.battery();
        let controls = Controls::Headset {
            sidetone: a.sidetone_level().ok(),
            auto_off: a.auto_sleep_minutes().ok(),
            eq: a
                .eq()
                .ok()
                .map(|eq| eq.bands.iter().map(|b| (b.freq, b.gain)).collect()),
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

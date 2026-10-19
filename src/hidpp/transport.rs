// SPDX-License-Identifier: GPL-3.0-or-later
//! Транспорт HID++ поверх hidapi.
//!
//! На Windows каждая top-level collection — отдельный путь: short-отчёты живут в коллекции
//! с usage `0x0001`, long — `0x0002` (page `0xFF00`). Открываем все коллекции интерфейса:
//! пишем в ту, что поддерживает нужный размер, читаем ответы со всех.
//! Для чтения у каждой коллекции свой handle и свой поток (blocking read с таймаутом),
//! все входящие отчёты сводятся в один канал.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hidapi::{DeviceInfo, HidApi, HidDevice};

use super::Error;
use super::message::{
    ERROR_10, ERROR_20, LONG, LONG_LEN, Report, SHORT, SHORT_LEN, VERY_LONG, VERY_LONG_LEN, hex,
};

pub const LOGITECH_VID: u16 = 0x046D;
pub const HIDPP_USAGE_PAGE: u16 = 0xFF00;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
/// Быстрый режим для перебора слотов, которые скорее всего пусты: без повтора.
const QUICK_TIMEOUT: Duration = Duration::from_millis(300);
/// Таймаут blocking read в потоке чтения: как часто поток проверяет флаг остановки.
const READ_TIMEOUT_MS: i32 = 1000;

/// HID++ 1.0: чтение короткого и длинного регистра.
const GET_REGISTER: u8 = 0x81;
const GET_LONG_REGISTER: u8 = 0x83;

/// Одна top-level collection интерфейса.
#[derive(Debug, Clone)]
pub struct Collection {
    pub path: CString,
    pub usage_page: u16,
    pub usage: u16,
}

impl Collection {
    /// Размер отчёта HID++, который принимает коллекция (по usage на page 0xFF00).
    fn report_id(&self) -> Option<u8> {
        if self.usage_page != HIDPP_USAGE_PAGE {
            return None;
        }
        match self.usage {
            0x0001 => Some(SHORT),
            0x0002 => Some(LONG),
            0x0004 => Some(VERY_LONG),
            _ => None,
        }
    }
}

/// USB-интерфейс устройства Logitech со всеми его коллекциями.
#[derive(Debug, Clone)]
pub struct Interface {
    pub pid: u16,
    pub interface: i32,
    pub product: String,
    pub serial: Option<String>,
    pub release: u16,
    pub collections: Vec<Collection>,
}

impl Interface {
    pub fn is_hidpp(&self) -> bool {
        self.collections.iter().any(|c| c.report_id().is_some())
    }
}

/// Все интерфейсы Logitech, сгруппированные по устройству и номеру интерфейса.
pub fn enumerate(api: &HidApi) -> Vec<Interface> {
    let mut groups: BTreeMap<String, Interface> = BTreeMap::new();
    for info in api.device_list().filter(|d| d.vendor_id() == LOGITECH_VID) {
        let key = interface_key(info);
        let entry = groups.entry(key).or_insert_with(|| Interface {
            pid: info.product_id(),
            interface: info.interface_number(),
            product: info.product_string().unwrap_or_default().to_owned(),
            serial: info
                .serial_number()
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            release: info.release_number(),
            collections: Vec::new(),
        });
        entry.collections.push(Collection {
            path: info.path().to_owned(),
            usage_page: info.usage_page(),
            usage: info.usage(),
        });
    }
    let mut list: Vec<_> = groups.into_values().collect();
    for i in &mut list {
        i.collections.sort_by_key(|c| (c.usage_page, c.usage));
    }
    list
}

/// Ключ группировки коллекций одного интерфейса. Путь на Windows выглядит так:
/// `\\?\hid#vid_046d&pid_c54d&mi_02&col01#a&df688c6&0&0000#{guid}` — у коллекций одного
/// интерфейса совпадает всё, кроме `&colNN` и номера экземпляра в конце третьей части.
fn interface_key(info: &DeviceInfo) -> String {
    let path = info.path().to_string_lossy().to_lowercase();
    let parts: Vec<&str> = path.split('#').collect();
    if parts.len() < 3 {
        return path;
    }
    let ids = match parts[1].find("&col") {
        Some(i) => &parts[1][..i],
        None => parts[1],
    };
    let instance = parts[2].rsplit_once('&').map_or(parts[2], |(head, _)| head);
    format!("{ids}#{instance}")
}

pub struct Transport {
    writers: Vec<(u8, HidDevice)>,
    rx: Receiver<Report>,
    stop: Arc<AtomicBool>,
    readers: Vec<JoinHandle<()>>,
    /// Асинхронные сообщения устройства, пришедшие во время ожидания ответа.
    notifications: RefCell<Vec<Report>>,
    next_sw_id: Cell<u8>,
    quick: Cell<bool>,
}

impl Transport {
    pub fn open(api: &HidApi, info: &Interface) -> Result<Self, Error> {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut writers = Vec::new();
        let mut readers = Vec::new();

        for c in &info.collections {
            let Some(report_id) = c.report_id() else {
                continue;
            };
            writers.push((report_id, api.open_path(&c.path)?));
            let reader = api.open_path(&c.path)?;
            let (tx, stop) = (tx.clone(), stop.clone());
            readers.push(std::thread::spawn(move || read_loop(&reader, &tx, &stop)));
        }
        if writers.is_empty() {
            return Err(Error::NoHidppCollection);
        }
        Ok(Self {
            writers,
            rx,
            stop,
            readers,
            notifications: RefCell::default(),
            next_sw_id: Cell::new(0x0A),
            quick: Cell::new(false),
        })
    }

    /// Ненулевой sw_id, отличающий наши ответы от уведомлений (у них sw_id = 0).
    fn sw_id(&self) -> u8 {
        let id = self.next_sw_id.get();
        self.next_sw_id.set(if id >= 0x0F { 0x02 } else { id + 1 });
        id
    }

    fn writer(&self, report_id: u8) -> Option<&HidDevice> {
        self.writers
            .iter()
            .find(|(id, _)| *id == report_id)
            .map(|(_, d)| d)
    }

    fn supports(&self, report_id: u8) -> bool {
        self.writer(report_id).is_some()
    }

    /// Запросы внутри `f` — с коротким таймаутом и без повтора.
    pub fn quick<R>(&self, f: impl FnOnce() -> R) -> R {
        let prev = self.quick.replace(true);
        let result = f();
        self.quick.set(prev);
        result
    }

    /// Вызов функции HID++ 2.0. Long-отчёт, если коллекция есть; иначе short при ≤ 3 параметрах.
    pub fn call(
        &self,
        device: u8,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Report, Error> {
        self.call_as(false, device, feature_index, function, params)
    }

    /// То же, но short-отчётом, если параметры помещаются. Ресиверы (HID++ 1.0)
    /// на long-отчёт по индексу 0xFF не отвечают, а на short возвращают ошибку 1.0.
    pub fn call_short(
        &self,
        device: u8,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Report, Error> {
        self.call_as(true, device, feature_index, function, params)
    }

    fn call_as(
        &self,
        short: bool,
        device: u8,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Report, Error> {
        let fits_short = params.len() <= SHORT_LEN - 4 && self.supports(SHORT);
        let report_id = if short && fits_short {
            SHORT
        } else if params.len() <= LONG_LEN - 4 && self.supports(LONG) {
            LONG
        } else if fits_short {
            SHORT
        } else if params.len() <= VERY_LONG_LEN - 4 && self.supports(VERY_LONG) {
            VERY_LONG
        } else {
            return Err(Error::NoHidppCollection);
        };
        let sw_id = self.sw_id();
        let address = (function << 4) | sw_id;
        let request = Report::new(report_id, device, feature_index, address, params);
        self.transact(&request, |r| {
            if r.device() != device {
                return None;
            }
            let p = r.params();
            if r.sub_id() == feature_index && r.address() == address {
                Some(Ok(r.clone()))
            } else if r.sub_id() == ERROR_20 && p[0] == feature_index && p[1] == address {
                Some(Err(Error::Hidpp20(p[2])))
            } else if r.sub_id() == ERROR_10 && p[0] == feature_index && p[1] == address {
                // Так отвечает HID++ 1.0 (ресивер) на запрос 2.0.
                Some(Err(Error::Hidpp10(p[2])))
            } else {
                None
            }
        })
    }

    /// Чтение регистра HID++ 1.0 (только чтение — запись регистров не используется).
    pub fn read_register(
        &self,
        device: u8,
        register: u8,
        long: bool,
        params: [u8; 3],
    ) -> Result<Report, Error> {
        let sub_id = if long {
            GET_LONG_REGISTER
        } else {
            GET_REGISTER
        };
        let request = Report::new(SHORT, device, sub_id, register, &params);
        self.transact(&request, |r| {
            if r.device() != device {
                return None;
            }
            let p = r.params();
            if r.sub_id() == sub_id && r.address() == register {
                Some(Ok(r.clone()))
            } else if r.sub_id() == ERROR_10 && p[0] == sub_id && p[1] == register {
                Some(Err(Error::Hidpp10(p[2])))
            } else {
                None
            }
        })
    }

    /// Отправить запрос и дождаться ответа: таймаут ~1 с, один повтор (в быстром режиме — 300 мс без повтора).
    fn transact(
        &self,
        request: &Report,
        matches: impl Fn(&Report) -> Option<Result<Report, Error>>,
    ) -> Result<Report, Error> {
        let writer = self
            .writer(request.report_id())
            .ok_or(Error::NoHidppCollection)?;
        let (timeout, attempts) = if self.quick.get() {
            (QUICK_TIMEOUT, 1)
        } else {
            (REQUEST_TIMEOUT, 2)
        };
        for attempt in 0..attempts {
            self.drain();
            tracing::debug!(target: "hubless::hidpp", "→ {}", hex(request.as_bytes()));
            writer.write(request.as_bytes())?;
            let deadline = Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match self.rx.recv_timeout(remaining) {
                    Ok(r) => match matches(&r) {
                        Some(result) => return result,
                        None => self.notifications.borrow_mut().push(r),
                    },
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return Err(Error::Disconnected),
                }
            }
            tracing::debug!(target: "hubless::hidpp", attempt, "таймаут");
        }
        Err(Error::Timeout)
    }

    /// Перенести накопившиеся отчёты в уведомления, чтобы не спутать их с ответом.
    fn drain(&self) {
        while let Ok(r) = self.rx.try_recv() {
            self.notifications.borrow_mut().push(r);
        }
    }

    pub fn take_notifications(&self) -> Vec<Report> {
        self.drain();
        std::mem::take(&mut self.notifications.borrow_mut())
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for r in self.readers.drain(..) {
            let _ = r.join();
        }
    }
}

fn read_loop(dev: &HidDevice, tx: &mpsc::Sender<Report>, stop: &AtomicBool) {
    let mut buf = [0u8; VERY_LONG_LEN];
    while !stop.load(Ordering::Relaxed) {
        match dev.read_timeout(&mut buf, READ_TIMEOUT_MS) {
            Ok(0) => {}
            Ok(n) => {
                if let Some(r) = Report::from_bytes(&buf[..n]) {
                    tracing::debug!(target: "hubless::hidpp", "← {}", hex(r.as_bytes()));
                    if tx.send(r).is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::debug!(target: "hubless::hidpp", "read: {e}");
                return;
            }
        }
    }
}

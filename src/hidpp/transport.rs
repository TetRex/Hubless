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
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use hidapi::{DeviceInfo, HidApi, HidDevice};

use super::Error;
use super::message::{
    ERROR_10, ERROR_20, LONG, LONG_LEN, Report, SHORT, SHORT_LEN, VERY_LONG, VERY_LONG_LEN, hex,
};

pub const LOGITECH_VID: u16 = 0x046D;
pub const HIDPP_USAGE_PAGE: u16 = 0xFF00;
/// HID++ у устройств G-серии (отчёт пользователя 2026-10-20: G815, коллекции `FF43:0602/0604`).
pub const HIDPP_G_USAGE_PAGE: u16 = 0xFF43;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
/// Быстрый режим для перебора слотов, которые скорее всего пусты: без повтора.
const QUICK_TIMEOUT: Duration = Duration::from_millis(150);
/// Отказ записи отчёта (`os error 31`, ERROR_GEN_FAILURE): LIGHTSPEED-ресивер иногда кратко
/// отвергает отчёт — так было, пока спящая мышь переподключалась (2026-10-22), и тогда поиск не
/// видел мышь или её имя. Через паузу запись проходит — повторяем.
const WRITE_ATTEMPTS: u32 = 3;
const WRITE_RETRY_DELAY: Duration = Duration::from_millis(50);
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
    /// Размер отчёта HID++, который принимает коллекция: usage `0x0001`/`0x0002`/`0x0004`
    /// на page `0xFF00` или `0x0601`/`0x0602`/`0x0604` на page `0xFF43` (G-серия, по Solaar;
    /// у G815 по кабелю есть только long и very long — short тогда уходит long-отчётом).
    fn report_id(&self) -> Option<u8> {
        let size = match self.usage_page {
            HIDPP_USAGE_PAGE => self.usage,
            HIDPP_G_USAGE_PAGE if self.usage >> 8 == 0x06 => self.usage & 0xFF,
            _ => return None,
        };
        match size {
            0x01 => Some(SHORT),
            0x02 => Some(LONG),
            0x04 => Some(VERY_LONG),
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

/// Что приходит в канал транспорта.
#[derive(Debug)]
pub enum Inbound {
    Report(Report),
    /// Разбудить поток, ждущий событий (есть команда для него).
    Wake,
    /// Поток чтения завершился: устройство отключено.
    Closed,
}

/// Результат ожидания событий устройства.
#[derive(Debug)]
pub enum Event {
    Report(Report),
    Wake,
    Closed,
    Timeout,
}

pub struct Transport {
    writers: Vec<(u8, HidDevice)>,
    rx: Receiver<Inbound>,
    tx: Sender<Inbound>,
    stop: Arc<AtomicBool>,
    /// Асинхронные сообщения устройства, пришедшие во время ожидания ответа.
    notifications: RefCell<Vec<Report>>,
    next_sw_id: Cell<u8>,
    quick: Cell<bool>,
    woken: Cell<bool>,
    closed: Cell<bool>,
}

impl Transport {
    pub fn open(api: &HidApi, info: &Interface) -> Result<Self, Error> {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut writers = Vec::new();

        for c in &info.collections {
            let Some(report_id) = c.report_id() else {
                continue;
            };
            writers.push((report_id, api.open_path(&c.path)?));
            let reader = api.open_path(&c.path)?;
            let (tx, stop) = (tx.clone(), stop.clone());
            // Поток не ждём при закрытии: он сам выйдет по флагу `stop` в течение READ_TIMEOUT_MS.
            std::thread::spawn(move || read_loop(&reader, &tx, &stop));
        }
        if writers.is_empty() {
            return Err(Error::NoHidppCollection);
        }
        Ok(Self {
            writers,
            rx,
            tx,
            stop,
            notifications: RefCell::default(),
            next_sw_id: Cell::new(0x0A),
            quick: Cell::new(false),
            woken: Cell::new(false),
            closed: Cell::new(false),
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

    pub fn call_as(
        &self,
        short: bool,
        device: u8,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Report, Error> {
        let report_id = self.pick_report(short, params.len())?;
        let sw_id = self.sw_id();
        let address = (function << 4) | sw_id;
        let request = Report::new(report_id, device, feature_index, address, params);
        self.transact(&request, |r| {
            if r.device() != device {
                return None;
            }
            if r.sub_id() == feature_index && r.address() == address {
                Some(Ok(r.clone()))
            } else if let Some(code) = error_code(r, ERROR_20, feature_index, address) {
                Some(Err(Error::Hidpp20(code)))
            } else {
                // Так отвечает HID++ 1.0 (ресивер) на запрос 2.0.
                error_code(r, ERROR_10, feature_index, address).map(|c| Err(Error::Hidpp10(c)))
            }
        })
    }

    fn pick_report(&self, short: bool, params_len: usize) -> Result<u8, Error> {
        let fits_short = params_len <= SHORT_LEN - 4 && self.supports(SHORT);
        if short && fits_short {
            Ok(SHORT)
        } else if params_len <= LONG_LEN - 4 && self.supports(LONG) {
            Ok(LONG)
        } else if fits_short {
            Ok(SHORT)
        } else if params_len <= VERY_LONG_LEN - 4 && self.supports(VERY_LONG) {
            Ok(VERY_LONG)
        } else {
            Err(Error::NoHidppCollection)
        }
    }

    /// Запрос, который отправил бы [`Transport::call_as`], — без отправки (sw_id = 0).
    pub fn preview(
        &self,
        short: bool,
        device: u8,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Report, Error> {
        let report_id = self.pick_report(short, params.len())?;
        Ok(Report::new(
            report_id,
            device,
            feature_index,
            function << 4,
            params,
        ))
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
            if r.sub_id() == sub_id && r.address() == register {
                Some(Ok(r.clone()))
            } else {
                error_code(r, ERROR_10, sub_id, register).map(|c| Err(Error::Hidpp10(c)))
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
            write_with_retry(writer, request.as_bytes())?;
            let deadline = Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match self.rx.recv_timeout(remaining) {
                    Ok(Inbound::Report(r)) => match matches(&r) {
                        Some(result) => return result,
                        None => self.notifications.borrow_mut().push(r),
                    },
                    Ok(Inbound::Wake) => self.woken.set(true),
                    Ok(Inbound::Closed) | Err(RecvTimeoutError::Disconnected) => {
                        self.closed.set(true);
                        return Err(Error::Disconnected);
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                }
            }
            tracing::debug!(target: "hubless::hidpp", attempt, "таймаут");
        }
        Err(Error::Timeout)
    }

    /// Перенести накопившиеся отчёты в уведомления, чтобы не спутать их с ответом.
    fn drain(&self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Inbound::Report(r) => self.notifications.borrow_mut().push(r),
                Inbound::Wake => self.woken.set(true),
                Inbound::Closed => self.closed.set(true),
            }
        }
    }

    /// Отправитель, которым другой поток может разбудить [`Transport::next_event`].
    pub fn waker(&self) -> Sender<Inbound> {
        self.tx.clone()
    }

    /// Ждать следующего события устройства (уведомления, пробуждения или отключения).
    pub fn next_event(&self, timeout: Duration) -> Event {
        self.drain();
        if self.closed.get() {
            return Event::Closed;
        }
        if self.woken.replace(false) {
            return Event::Wake;
        }
        if !self.notifications.borrow().is_empty() {
            return Event::Report(self.notifications.borrow_mut().remove(0));
        }
        match self.rx.recv_timeout(timeout) {
            Ok(Inbound::Report(r)) => Event::Report(r),
            Ok(Inbound::Wake) => Event::Wake,
            Ok(Inbound::Closed) | Err(RecvTimeoutError::Disconnected) => {
                self.closed.set(true);
                Event::Closed
            }
            Err(RecvTimeoutError::Timeout) => Event::Timeout,
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
    }
}

fn read_loop(dev: &HidDevice, tx: &Sender<Inbound>, stop: &AtomicBool) {
    let mut buf = [0u8; VERY_LONG_LEN];
    while !stop.load(Ordering::Relaxed) {
        match dev.read_timeout(&mut buf, READ_TIMEOUT_MS) {
            Ok(0) => {}
            Ok(n) => {
                if let Some(r) = Report::from_bytes(&buf[..n]) {
                    tracing::debug!(target: "hubless::hidpp", "← {}", hex(r.as_bytes()));
                    if tx.send(Inbound::Report(r)).is_err() {
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::debug!(target: "hubless::hidpp", "read: {e}");
                let _ = tx.send(Inbound::Closed);
                return;
            }
        }
    }
}

/// Код ошибки из ответа-ошибки на наш запрос. Раскладка (и для 2.0 `0xFF`, и для 1.0 `0x8F`):
/// `[report][device][marker][feature_index | sub_id][address][code]`.
fn error_code(r: &Report, marker: u8, first: u8, address: u8) -> Option<u8> {
    let p = r.params();
    (r.sub_id() == marker && r.address() == first && p[0] == address).then_some(p[1])
}

/// Запись отчёта с повтором при отказе устройства (см. [`WRITE_ATTEMPTS`]).
fn write_with_retry(writer: &HidDevice, bytes: &[u8]) -> Result<(), Error> {
    let mut attempt = 1;
    loop {
        match writer.write(bytes) {
            Ok(_) => return Ok(()),
            Err(e) if attempt < WRITE_ATTEMPTS => {
                tracing::debug!(target: "hubless::hidpp", attempt, "запись отвергнута: {e}");
                std::thread::sleep(WRITE_RETRY_DELAY);
                attempt += 1;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

    #[test]
    fn hidpp20_error_matches_request() {
        // Реальный ответ мыши на setDpi с lift-off 0 (2026-10-19): INVALID_ARGUMENT.
        let mut b = vec![0x11, 0x01, 0xFF, 0x09, 0x6E, 0x02];
        b.resize(LONG_LEN, 0);
        let r = Report::from_bytes(&b).unwrap();
        assert_eq!(error_code(&r, ERROR_20, 0x09, 0x6E), Some(0x02));
        assert_eq!(error_code(&r, ERROR_20, 0x09, 0x6F), None);
        assert_eq!(error_code(&r, ERROR_10, 0x09, 0x6E), None);
    }

    #[test]
    fn hidpp_collections_on_both_pages() {
        let c = |usage_page, usage| Collection {
            path: CString::default(),
            usage_page,
            usage,
        };
        assert_eq!(c(0xFF00, 0x0001).report_id(), Some(SHORT));
        assert_eq!(c(0xFF00, 0x0002).report_id(), Some(LONG));
        // G815 по кабелю (отчёт пользователя 2026-10-20).
        assert_eq!(c(0xFF43, 0x0602).report_id(), Some(LONG));
        assert_eq!(c(0xFF43, 0x0604).report_id(), Some(VERY_LONG));
        assert_eq!(c(0xFF43, 0x0002).report_id(), None);
        assert_eq!(c(0xFFBC, 0x0088).report_id(), None);
    }

    #[test]
    fn hidpp10_error_matches_register() {
        let r = Report::from_bytes(&[0x10, 0xFF, 0x8F, 0x83, 0xB5, 0x03, 0x00]).unwrap();
        assert_eq!(error_code(&r, ERROR_10, 0x83, 0xB5), Some(0x03));
    }
}

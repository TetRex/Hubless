// SPDX-License-Identifier: GPL-3.0-or-later
//! The HID++ transport on top of hidapi.
//!
//! On Windows every top-level collection is a separate path: short reports live in the collection
//! with usage `0x0001`, long ones in `0x0002` (page `0xFF00`). We open all collections of the interface,
//! write to the one that supports the needed size, and read replies from all of them.
//! For reading, each collection has its own handle and its own thread (blocking read with a timeout),
//! and all incoming reports go into one channel.

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
/// HID++ on G-series devices (user report 2026-10-20: G815, collections `FF43:0602/0604`).
pub const HIDPP_G_USAGE_PAGE: u16 = 0xFF43;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
/// Quick mode for probing slots that are most likely empty: no retry.
const QUICK_TIMEOUT: Duration = Duration::from_millis(150);
/// A report write failure (`os error 31`, ERROR_GEN_FAILURE): a LIGHTSPEED receiver sometimes briefly
/// rejects a report. That happened while a sleeping mouse was reconnecting (2026-10-22), and then discovery
/// didn't see the mouse or its name. After a pause the write goes through, so we retry.
const WRITE_ATTEMPTS: u32 = 3;
const WRITE_RETRY_DELAY: Duration = Duration::from_millis(50);
/// The blocking read timeout in the reader thread: how often the thread checks the stop flag.
const READ_TIMEOUT_MS: i32 = 1000;

/// HID++ 1.0: reading a short and a long register.
const GET_REGISTER: u8 = 0x81;
const GET_LONG_REGISTER: u8 = 0x83;

/// One top-level collection of an interface.
#[derive(Debug, Clone)]
pub struct Collection {
    pub path: CString,
    pub usage_page: u16,
    pub usage: u16,
}

impl Collection {
    /// The HID++ report size the collection accepts: usage `0x0001`/`0x0002`/`0x0004`
    /// on page `0xFF00`, or `0x0601`/`0x0602`/`0x0604` on page `0xFF43` (G series, per Solaar;
    /// the G815 over the cable has only long and very long, so a short report then goes out as a long one).
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

/// A USB interface of a Logitech device with all its collections.
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

/// All Logitech interfaces, grouped by device and interface number.
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

/// The grouping key for the collections of one interface. On Windows a path looks like
/// `\\?\hid#vid_046d&pid_c54d&mi_02&col01#a&df688c6&0&0000#{guid}`: for the collections of one
/// interface everything matches except `&colNN` and the instance number at the end of the third part.
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

/// What arrives in the transport's channel.
#[derive(Debug)]
pub enum Inbound {
    Report(Report),
    /// Wakes the thread waiting for events (there is a command for it).
    Wake,
    /// The reader thread has finished: the device is disconnected.
    Closed,
}

/// The result of waiting for device events.
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
    /// Asynchronous device messages that arrived while waiting for a reply.
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
            // We don't wait for the thread on close: it exits by itself on the `stop` flag within READ_TIMEOUT_MS.
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

    /// A non-zero sw_id that tells our replies apart from notifications (theirs is sw_id = 0).
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

    /// Requests inside `f` use a short timeout and no retry.
    pub fn quick<R>(&self, f: impl FnOnce() -> R) -> R {
        let prev = self.quick.replace(true);
        let result = f();
        self.quick.set(prev);
        result
    }

    /// Calls an HID++ 2.0 function. A long report if the collection exists; otherwise a short one with ≤ 3 parameters.
    pub fn call(
        &self,
        device: u8,
        feature_index: u8,
        function: u8,
        params: &[u8],
    ) -> Result<Report, Error> {
        self.call_as(false, device, feature_index, function, params)
    }

    /// The same, but as a short report if the parameters fit. Receivers (HID++ 1.0)
    /// don't answer a long report at index 0xFF and return a 1.0 error for a short one.
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
                // That's how HID++ 1.0 (a receiver) answers a 2.0 request.
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

    /// The request [`Transport::call_as`] would send, without sending it (sw_id = 0).
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

    /// Reads an HID++ 1.0 register (read-only: register writes aren't used).
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

    /// Sends a request and waits for the reply: timeout ~1 s, one retry (in quick mode, 300 ms without a retry).
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

    /// Moves the accumulated reports to the notifications so they aren't mistaken for the reply.
    fn drain(&self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Inbound::Report(r) => self.notifications.borrow_mut().push(r),
                Inbound::Wake => self.woken.set(true),
                Inbound::Closed => self.closed.set(true),
            }
        }
    }

    /// A sender another thread can use to wake [`Transport::next_event`].
    pub fn waker(&self) -> Sender<Inbound> {
        self.tx.clone()
    }

    /// Waits for the next device event (a notification, a wake-up or a disconnect).
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

/// The error code from an error reply to our request. Layout (both for 2.0 `0xFF` and for 1.0 `0x8F`):
/// `[report][device][marker][feature_index | sub_id][address][code]`.
fn error_code(r: &Report, marker: u8, first: u8, address: u8) -> Option<u8> {
    let p = r.params();
    (r.sub_id() == marker && r.address() == first && p[0] == address).then_some(p[1])
}

/// Writes a report, retrying when the device rejects it (see [`WRITE_ATTEMPTS`]).
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
        // A real mouse reply to setDpi with lift-off 0 (2026-10-19): INVALID_ARGUMENT.
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
        // G815 over the cable (user report 2026-10-20).
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

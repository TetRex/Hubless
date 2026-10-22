// SPDX-License-Identifier: GPL-3.0-or-later
//! Проверка обновлений по кнопке в настройках («Advanced»): один запрос к GitHub API за
//! последним релизом. Это единственное место, где Hubless ходит в сеть, и только по нажатию —
//! фоновых проверок нет. HTTP — системный WinHTTP, без лишних зависимостей.

use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::core::{PCWSTR, w};

/// Страница последнего релиза — открывается кнопкой «Скачать».
pub const RELEASES_URL: &str = "https://github.com/TetRex/Hubless/releases/latest";
const API_HOST: windows::core::PCWSTR = w!("api.github.com");
const API_PATH: windows::core::PCWSTR = w!("/repos/TetRex/Hubless/releases/latest");
/// Таймауты WinHTTP, мс: поиск имени, соединение, отправка, ответ.
const TIMEOUT_MS: i32 = 10_000;

/// Последний релиз на GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Версия без `v`: `0.1.5`.
    pub version: String,
    /// Страница релиза.
    pub url: String,
}

/// Последний релиз (блокирующий запрос — вызывать не на главном потоке).
pub fn latest() -> Result<Release, String> {
    let body = get_latest_release()?;
    parse_release(&body)
}

fn parse_release(body: &[u8]) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| format!("json: {e}"))?;
    let tag = v["tag_name"].as_str().ok_or("no tag_name in the reply")?;
    Ok(Release {
        version: tag.trim_start_matches('v').to_owned(),
        url: v["html_url"].as_str().unwrap_or(RELEASES_URL).to_owned(),
    })
}

/// Версия `latest` новее `current` (числа через точку; лишние части — нули).
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse =
        |s: &str| -> Vec<u64> { s.split(['.', '-']).map_while(|p| p.parse().ok()).collect() };
    let (a, b) = (parse(latest), parse(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// Хэндл WinHTTP, закрывается при drop.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: хэндл получен от WinHTTP и закрывается один раз.
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn handle(h: *mut core::ffi::c_void, what: &str) -> Result<Handle, String> {
    if h.is_null() {
        Err(format!("{what}: {}", windows::core::Error::from_thread()))
    } else {
        Ok(Handle(h))
    }
}

/// GET `https://api.github.com/repos/.../releases/latest`: тело ответа при коде 200.
fn get_latest_release() -> Result<Vec<u8>, String> {
    let agent = format!("Hubless/{}\0", env!("CARGO_PKG_VERSION"));
    let agent: Vec<u16> = agent.encode_utf16().collect();
    // SAFETY: все строки живут до конца функции, хэндлы закрываются в Drop в обратном порядке.
    unsafe {
        let session = handle(
            WinHttpOpen(
                PCWSTR(agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ),
            "WinHttpOpen",
        )?;
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS)
            .map_err(|e| format!("timeouts: {e}"))?;
        let connect = handle(
            WinHttpConnect(session.0, API_HOST, INTERNET_DEFAULT_HTTPS_PORT, 0),
            "WinHttpConnect",
        )?;
        let request = handle(
            WinHttpOpenRequest(
                connect.0,
                w!("GET"),
                API_PATH,
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            ),
            "WinHttpOpenRequest",
        )?;
        let headers: Vec<u16> = "Accept: application/vnd.github+json"
            .encode_utf16()
            .collect();
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0)
            .map_err(|e| format!("send: {e}"))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|e| format!("receive: {e}"))?;

        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(|e| format!("status: {e}"))?;
        if status != 200 {
            return Err(format!("HTTP {status}"));
        }

        let mut body = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
            )
            .map_err(|e| format!("read: {e}"))?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.1.10", "0.1.4"));
        assert!(is_newer("0.2", "0.1.9"));
        assert!(!is_newer("0.1.4", "0.1.4"));
        assert!(!is_newer("0.1.3", "0.1.4"));
        assert!(!is_newer("garbage", "0.1.4"));
    }

    #[test]
    fn release_reply() {
        let body = br#"{"tag_name":"v0.1.5","html_url":"https://github.com/TetRex/Hubless/releases/tag/v0.1.5"}"#;
        let r = parse_release(body).unwrap();
        assert_eq!(r.version, "0.1.5");
        assert!(r.url.ends_with("v0.1.5"));
        assert!(parse_release(b"{}").is_err());
    }
}

#[cfg(test)]
mod network_tests {
    /// Настоящий запрос к GitHub: `cargo test --release live_latest_release -- --ignored`.
    #[test]
    #[ignore]
    fn live_latest_release() {
        let r = super::latest().unwrap();
        println!("latest: {} {}", r.version, r.url);
        assert!(!r.version.is_empty());
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
//! Toast-уведомления Windows для неупакованного приложения.
//!
//! AUMID регистрируется в `HKCU\Software\Classes\AppUserModelId\<id>` (имя для центра уведомлений)
//! и задаётся процессу. WinRT загружается только в момент показа уведомления.

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::core::HSTRING;

use crate::win::hkcu_write_string;

const AUMID: &str = "TetRex.Hubless";

/// Экранирование для XML-содержимого toast.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn show(title: &str, text: &str) {
    if let Err(e) = try_show(title, text) {
        tracing::warn!("toast: {e}");
    }
}

fn try_show(title: &str, text: &str) -> windows::core::Result<()> {
    hkcu_write_string(
        &format!(r"Software\Classes\AppUserModelId\{AUMID}"),
        "DisplayName",
        "Hubless",
    )?;
    // SAFETY: строковый аргумент живёт до конца вызова.
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID))? };

    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(text)
    );
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_xml() {
        assert_eq!(
            super::xml_escape("a<b & \"c\">"),
            "a&lt;b &amp; &quot;c&quot;&gt;"
        );
    }
}

// Send a dropped file by email. Resend when a key is set; otherwise the default
// mail client through Simple MAPI, or a plain mailto: when MAPI is not there
// (new Outlook, web mail). Only ever called from the Send button.
// Ported in spirit from the upstream MailView (IslandViewContent.swift).

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};
use windows::core::{s, w, PWSTR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Mapi::{MapiFileDescW, MapiMessageW, MapiRecipDescW};

/// Resend takes about 40 MB per message after base64.
const MAX_BYTES: u64 = 28 * 1024 * 1024;

/// The dropped file, only if it is one of the copies in the inbox.
pub fn checked_file(path: &str) -> Result<PathBuf, String> {
    let file = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let inbox = std::fs::canonicalize(notch_core::files::inbox_dir()).map_err(|e| e.to_string())?;
    if !file.starts_with(inbox) || !file.is_file() {
        return Err("Only a file dropped on the island can be sent.".into());
    }
    Ok(file)
}

pub fn valid_address(a: &str) -> bool {
    let a = a.trim();
    let Some((user, host)) = a.split_once('@') else { return false };
    !user.is_empty()
        && host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains('@')
        && !a.chars().any(|c| c.is_whitespace() || matches!(c, ',' | ';' | '<' | '>' | '"'))
}

pub fn via_resend(key: &str, from: &str, to: &str, subject: &str, text: &str, file: Option<&PathBuf>) -> Result<Value, String> {
    let mut body = json!({ "from": from, "to": [to], "subject": subject, "text": text });
    if let Some(file) = file {
        let size = std::fs::metadata(file).map_err(|e| e.to_string())?.len();
        if size > MAX_BYTES {
            return Err("This file is too large to send by email (limit about 28 MB).".into());
        }
        let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
        let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        body["attachments"] = json!([{ "filename": name, "content": base64::engine::general_purpose::STANDARD.encode(bytes) }]);
    }
    let url = reqwest::Url::parse("https://api.resend.com/emails").map_err(|e| e.to_string())?;
    let client = notch_core::net::keyed_client(&url, Duration::from_secs(90))?;
    tauri::async_runtime::block_on(async {
        let res = client.post(url).bearer_auth(key).json(&body).send().await.map_err(|e| e.to_string())?;
        let status = res.status();
        let bytes = res.bytes().await.map_err(|e| e.to_string())?;
        if status.is_success() {
            Ok(json!({ "via": "resend" }))
        } else {
            Err(format!("Resend: {}", notch_core::net::error_detail(&bytes)))
        }
    })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Opens a compose window with the file attached. `Ok(true)` once the client took it,
/// `Ok(false)` when the user closed it, `Err` when MAPI is not usable.
fn mapi(to: &str, subject: &str, text: &str, file: &PathBuf) -> Result<bool, String> {
    type Send = unsafe extern "system" fn(usize, usize, *mut MapiMessageW, u32, u32) -> u32;
    unsafe {
        let lib = LoadLibraryW(w!("mapi32.dll")).map_err(|e| e.to_string())?;
        let f = GetProcAddress(lib, s!("MAPISendMailW")).ok_or("MAPI is not available")?;
        let f: Send = std::mem::transmute(f);
        let mut path = wide(&file.to_string_lossy());
        let mut name = wide(&file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        let mut addr = wide(&format!("SMTP:{to}"));
        let mut who = wide(to);
        let mut subject = wide(subject);
        let mut text = wide(text);
        let mut attachment = MapiFileDescW {
            lpszPathName: PWSTR(path.as_mut_ptr()),
            lpszFileName: PWSTR(name.as_mut_ptr()),
            nPosition: u32::MAX,
            ..Default::default()
        };
        let mut recipient = MapiRecipDescW {
            ulRecipClass: 1,
            lpszName: PWSTR(who.as_mut_ptr()),
            lpszAddress: PWSTR(addr.as_mut_ptr()),
            ..Default::default()
        };
        let mut msg = MapiMessageW {
            lpszSubject: PWSTR(subject.as_mut_ptr()),
            lpszNoteText: PWSTR(text.as_mut_ptr()),
            nRecipCount: 1,
            lpRecips: &mut recipient,
            nFileCount: 1,
            lpFiles: &mut attachment,
            ..Default::default()
        };
        // MAPI_LOGON_UI | MAPI_DIALOG
        match f(0, 0, &mut msg, 0x1 | 0x8, 0) {
            0 => Ok(true),
            1 => Ok(false), // MAPI_E_USER_ABORT
            code => Err(format!("MAPI error {code}")),
        }
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

pub fn via_client(to: &str, subject: &str, text: &str, file: Option<&PathBuf>) -> Value {
    match file.map(|file| mapi(to, subject, text, file)) {
        Some(Ok(true)) => json!({ "via": "mapi" }),
        Some(Ok(false)) => json!({ "via": "cancelled" }),
        // No file (a mail written by voice), or no MAPI mail client: the mail program's own compose window.
        failed => {
            notch_core::win::open_url(&format!("mailto:{}?subject={}&body={}", encode(to), encode(subject), encode(text)));
            // mailto: cannot carry an attachment, so show the file instead.
            if let (Some(_), Some(file)) = (failed, file) {
                notch_core::win::reveal_folder(&file.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default());
            }
            json!({ "via": "mailto" })
        }
    }
}

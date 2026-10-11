// The clipboard, opened the careful way: a few tries (another program may hold
// it), read or written at once, closed on drop. Nothing here keeps it open
// across a wait. Used by `paste_text` in this crate and by the clipboard
// feature, so that one place knows which writes were the app's own.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use windows::core::HSTRING;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::Graphics::Gdi::{GetEnhMetaFileBits, SetEnhMetaFileBits, HENHMETAFILE};
use windows::Win32::System::DataExchange::{
    CloseClipboard, CountClipboardFormats, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardFormatNameW, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};

pub const CF_TEXT: u32 = 1;
pub const CF_BITMAP: u32 = 2;
pub const CF_METAFILEPICT: u32 = 3;
pub const CF_OEMTEXT: u32 = 7;
pub const CF_DIB: u32 = 8;
pub const CF_PALETTE: u32 = 9;
pub const CF_UNICODETEXT: u32 = 13;
pub const CF_ENHMETAFILE: u32 = 14;
pub const CF_HDROP: u32 = 15;
pub const CF_LOCALE: u32 = 16;
pub const CF_DIBV5: u32 = 17;

/// Any data in this format keeps the content out of clipboard histories and the cloud (Microsoft's "Clipboard Formats").
pub const EXCLUDE: &str = "ExcludeClipboardContentFromMonitorProcessing";
/// A DWORD: 0 keeps the content out of clipboard histories.
pub const NO_HISTORY: &str = "CanIncludeInClipboardHistory";
/// A DWORD: 0 keeps the content off other devices.
pub const NO_CLOUD: &str = "CanUploadToCloudClipboard";

/// The number of a named format (`RegisterClipboardFormatW`; the same name always gives the same number).
pub fn format(name: &str) -> u32 {
    unsafe { RegisterClipboardFormatW(&HSTRING::from(name)) }
}

/// A format's name, for logs and for telling formats apart: `CF_UNICODETEXT`, `HTML Format`, `#49161`.
pub fn format_name(id: u32) -> String {
    let standard = match id {
        CF_TEXT => "CF_TEXT",
        CF_BITMAP => "CF_BITMAP",
        CF_METAFILEPICT => "CF_METAFILEPICT",
        CF_OEMTEXT => "CF_OEMTEXT",
        CF_DIB => "CF_DIB",
        CF_PALETTE => "CF_PALETTE",
        CF_UNICODETEXT => "CF_UNICODETEXT",
        CF_ENHMETAFILE => "CF_ENHMETAFILE",
        CF_HDROP => "CF_HDROP",
        CF_LOCALE => "CF_LOCALE",
        CF_DIBV5 => "CF_DIBV5",
        _ => "",
    };
    if !standard.is_empty() {
        return standard.into();
    }
    let mut name = [0u16; 128];
    let len = unsafe { GetClipboardFormatNameW(id, &mut name) };
    if len > 0 { String::from_utf16_lossy(&name[..len as usize]) } else { format!("#{id}") }
}

/// Changes with every write to the clipboard, whoever made it.
pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// Nothing is on the clipboard.
pub fn is_empty() -> bool {
    unsafe { CountClipboardFormats() == 0 }
}

pub fn has(format: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(format).is_ok() }
}

// ── The app's own writes ──────────────────────────────────────────────────────

static OWN: AtomicU32 = AtomicU32::new(0);

/// Call right after a write of the app's own (and after closing the clipboard):
/// what is on the clipboard now is not a copy the user made.
pub fn mark_own() {
    OWN.store(sequence(), Ordering::SeqCst);
}

/// What is on the clipboard was put there by the app itself (an entry copied
/// back, a transient paste, its restore, a clear): a history must not record it.
pub fn is_own() -> bool {
    let seq = sequence();
    seq != 0 && seq == OWN.load(Ordering::SeqCst)
}

// ── Open ──────────────────────────────────────────────────────────────────────

/// The clipboard, open. Closed on drop: keep it for a few calls, never across a wait.
pub struct Open(());

/// Opens the clipboard, trying again for a moment when another program holds it.
pub fn open() -> Result<Open, String> {
    for wait in [0u64, 10, 20, 30, 40, 60] {
        std::thread::sleep(Duration::from_millis(wait));
        if unsafe { OpenClipboard(None) }.is_ok() {
            return Ok(Open(()));
        }
    }
    Err("The clipboard is in use by another program. Try again.".into())
}

impl Drop for Open {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Formats that are handles to drawing objects, not memory: they cannot be
/// copied as bytes. Windows makes the bitmap and the palette again from a DIB
/// and the old metafile from the enhanced one, so nothing is lost by leaving them.
fn is_object(format: u32) -> bool {
    matches!(format, CF_BITMAP | CF_PALETTE | CF_METAFILEPICT | 0x80..=0x8E | 0x200..=0x3FF)
}

impl Open {
    /// Every format on the clipboard, in the order the copier gave them.
    pub fn formats(&self) -> Vec<u32> {
        let mut all = Vec::new();
        let mut next = unsafe { EnumClipboardFormats(0) };
        while next != 0 {
            all.push(next);
            next = unsafe { EnumClipboardFormats(next) };
        }
        all
    }

    /// The bytes of a format held as memory; `None` when it is absent, empty or larger than `cap`.
    pub fn bytes(&self, format: u32, cap: usize) -> Option<Vec<u8>> {
        if is_object(format) {
            return None;
        }
        unsafe {
            let handle = GetClipboardData(format).ok()?;
            if format == CF_ENHMETAFILE {
                let picture = HENHMETAFILE(handle.0);
                let size = GetEnhMetaFileBits(picture, None) as usize;
                if size == 0 || size > cap {
                    return None;
                }
                let mut out = vec![0u8; size];
                GetEnhMetaFileBits(picture, Some(&mut out));
                return Some(out);
            }
            let memory = HGLOBAL(handle.0);
            let size = GlobalSize(memory);
            if size == 0 || size > cap {
                return None;
            }
            let at = GlobalLock(memory) as *const u8;
            if at.is_null() {
                return None;
            }
            let out = std::slice::from_raw_parts(at, size).to_vec();
            let _ = GlobalUnlock(memory);
            Some(out)
        }
    }

    /// The text on the clipboard (`CF_UNICODETEXT`), up to `cap` bytes of UTF-16.
    pub fn text(&self, cap: usize) -> Option<String> {
        let raw = self.bytes(CF_UNICODETEXT, cap)?;
        let units: Vec<u16> =
            raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        Some(String::from_utf16_lossy(&units))
    }

    /// The files of a copy made in Explorer (`CF_HDROP`).
    pub fn files(&self) -> Vec<String> {
        // DROPFILES: the offset of the names, a point, two flags; then names, each ended by a zero.
        let Some(raw) = self.bytes(CF_HDROP, 1 << 22) else { return Vec::new() };
        if raw.len() < 20 {
            return Vec::new();
        }
        let start = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        let wide = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]) != 0;
        let names = raw.get(start..).unwrap_or_default();
        let text = if wide {
            String::from_utf16_lossy(&names.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
        } else {
            String::from_utf8_lossy(names).into_owned()
        };
        text.split('\0').take_while(|name| !name.is_empty()).map(str::to_string).collect()
    }

    pub fn empty(&self) {
        unsafe {
            let _ = EmptyClipboard();
        }
    }

    /// Empties the clipboard and fills it again in one go.
    pub fn empty_then(&self, fill: impl FnOnce(&Open) -> Result<(), String>) -> Result<(), String> {
        self.empty();
        fill(self)
    }

    /// Puts `bytes` on the clipboard as `format`. Call `empty` first for a new copy.
    pub fn set(&self, format: u32, bytes: &[u8]) -> Result<(), String> {
        unsafe {
            if format == CF_ENHMETAFILE {
                let picture = SetEnhMetaFileBits(bytes);
                return SetClipboardData(format, Some(HANDLE(picture.0))).map(|_| ()).map_err(|e| e.message());
            }
            let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).map_err(|e| e.message())?;
            let at = GlobalLock(memory) as *mut u8;
            if at.is_null() {
                let _ = GlobalFree(Some(memory));
                return Err("Out of memory.".into());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), at, bytes.len());
            let _ = GlobalUnlock(memory);
            // On success the clipboard owns the memory.
            if let Err(err) = SetClipboardData(format, Some(HANDLE(memory.0))) {
                let _ = GlobalFree(Some(memory));
                return Err(err.message());
            }
            Ok(())
        }
    }

    pub fn set_text(&self, text: &str) -> Result<(), String> {
        let wide: Vec<u8> = text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        self.set(CF_UNICODETEXT, &wide)
    }

    /// Files, as Explorer's Copy puts them.
    pub fn set_files(&self, paths: &[String]) -> Result<(), String> {
        let mut raw = vec![0u8; 20];
        raw[0] = 20; // the names start after the header
        raw[16] = 1; // wide characters
        for path in paths {
            raw.extend(path.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        }
        raw.extend([0, 0]);
        self.set(CF_HDROP, &raw)?;
        self.set(format("Preferred DropEffect"), &1u32.to_le_bytes()) // copy
    }

    /// Marks what was just set as not to be recorded by any clipboard history, Windows' own included.
    pub fn set_excluded(&self) -> Result<(), String> {
        for name in [EXCLUDE, NO_HISTORY, NO_CLOUD] {
            self.set(format(name), &0u32.to_le_bytes())?;
        }
        Ok(())
    }

    /// Everything on the clipboard that is held as memory, to put back with `restore`.
    pub fn snapshot(&self) -> Snapshot {
        let formats = self.formats();
        let mut kept = Vec::new();
        let mut left = Vec::new();
        for format in formats {
            // OLE's own bookkeeping for a data object that will be gone: the plain formats beside it are the content.
            let name = format_name(format);
            if name == "DataObject" || name == "Ole Private Data" {
                continue;
            }
            match self.bytes(format, SNAPSHOT_CAP) {
                Some(bytes) => kept.push((format, bytes)),
                None if is_object(format) => {}
                None => left.push(format),
            }
        }
        Snapshot { formats: kept, left }
    }

    /// Empties the clipboard and puts a snapshot back.
    pub fn restore(&self, snapshot: &Snapshot) {
        self.empty();
        for (format, bytes) in &snapshot.formats {
            let _ = self.set(*format, bytes);
        }
    }
}

// ponytail: one format over 256 MiB is not saved (listed in `left`); nothing the clipboard usually holds is that large.
const SNAPSHOT_CAP: usize = 256 << 20;

/// What the clipboard held, format by format.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub formats: Vec<(u32, Vec<u8>)>,
    /// Formats that were there and could not be read (too large, or their owner did not answer).
    pub left: Vec<u32>,
}

impl Snapshot {
    pub fn bytes(&self) -> usize {
        self.formats.iter().map(|(_, b)| b.len()).sum()
    }
}

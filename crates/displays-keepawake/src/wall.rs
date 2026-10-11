// Wallpapers: the pictures Windows ships and the ones in a folder of the
// user's. Setting one is IDesktopWallpaper (all displays, Fill), with the
// classic call as the fallback.

use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper, DWPOS_FILL};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SPI_SETDESKWALLPAPER,
};

const EXT: [&str; 5] = ["jpg", "jpeg", "png", "bmp", "webp"];
/// FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: a OneDrive file that is not on disk yet.
const CLOUD: u32 = 0x0040_0000;

pub fn system_dir() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("Web").join("Wallpaper")
}

/// The user's folder: the setting, or Pictures.
pub fn own_dir(setting: &str) -> PathBuf {
    if !setting.is_empty() {
        return PathBuf::from(setting);
    }
    notch_core::win::picture_dirs().into_iter().next().unwrap_or_default()
}

fn is_picture(p: &Path) -> bool {
    p.extension().and_then(|x| x.to_str()).is_some_and(|x| EXT.contains(&x.to_ascii_lowercase().as_str()))
}

fn pictures(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                pictures(&p, depth - 1, out);
            }
        } else if is_picture(&p) {
            out.push(p);
        }
        if out.len() >= 400 {
            return;
        }
    }
}

/// (system pictures, own pictures), at most 400 of each.
pub fn list(own: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut sys, mut mine) = (Vec::new(), Vec::new());
    pictures(&system_dir(), 2, &mut sys);
    pictures(own, 0, &mut mine);
    sys.sort();
    mine.sort();
    (sys, mine)
}

/// True when `path` is a picture inside one of the two folders (nothing else is ever read or set).
pub fn allowed(path: &Path, own: &Path) -> bool {
    let Ok(p) = path.canonicalize() else { return false };
    let inside = |dir: &Path| dir.canonicalize().is_ok_and(|d| p.starts_with(d));
    is_picture(&p) && (inside(&system_dir()) || inside(own))
}

pub fn set(path: &Path) -> Result<(), String> {
    // A OneDrive placeholder is read once so that it is on disk before Windows looks at it.
    if std::fs::metadata(path).is_ok_and(|m| m.file_attributes() & CLOUD != 0) {
        std::fs::read(path).map_err(|e| e.to_string())?;
    }
    let wide = HSTRING::from(path.as_os_str());
    let result = unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance::<_, IDesktopWallpaper>(&DesktopWallpaper, None, CLSCTX_ALL).and_then(|d| {
            d.SetWallpaper(PCWSTR::null(), PCWSTR(wide.as_ptr()))?;
            d.SetPosition(DWPOS_FILL)
        })
    };
    if result.is_ok() {
        return Ok(());
    }
    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wide.as_ptr() as *mut _),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
    }
    .map_err(|e| e.message())
}

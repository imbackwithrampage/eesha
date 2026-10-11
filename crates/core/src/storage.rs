// A feature's own JSON file under `Ctx::data_dir`: loaded leniently, saved
// atomically. For preferences use `Ctx::settings` instead; this is for data
// (history, caches, journals).

use std::io::Write;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

/// The value in `path`, or `T::default()` when there is no file. A file that
/// does not parse is kept beside it as `<name>.corrupt-<stamp>` rather than
/// being written over by the next save, and the default is returned.
pub fn json_load<T: DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(bytes) = std::fs::read(path) else { return T::default() };
    // PowerShell 5 and Notepad write a UTF-8 BOM, and serde_json refuses it.
    let text = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    if text.iter().all(u8::is_ascii_whitespace) {
        return T::default();
    }
    serde_json::from_slice(text).unwrap_or_else(|err| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let aside = path.with_file_name(format!("{name}.corrupt-{}", crate::win::stamp()));
        let kept = std::fs::copy(path, &aside).is_ok();
        crate::log::line(format!(
            "{} is not usable ({err}){}",
            path.display(),
            if kept { format!(" — kept as {}", aside.display()) } else { String::new() }
        ));
        T::default()
    })
}

/// Writes beside the target and renames over it: a crash, a full disk or a
/// power cut leaves the previous file intact rather than half a file.
pub fn json_save_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!("{name}.{}-{}", notch_brand::SLUG, std::process::id()));
    let written = std::fs::File::create(&temp)
        .and_then(|mut file| file.write_all(&json).and_then(|()| file.sync_all()))
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

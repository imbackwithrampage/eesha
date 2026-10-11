// Pointer acceleration off: Windows' own "Enhance pointer precision", switched
// through SystemParametersInfo. A system setting the app changes is written
// down first and put back on quit and at the next launch (ARCHITECTURE §12.11).

use std::path::Path;

use notch_core::storage::{json_load, json_save_atomic};
use serde::{Deserialize, Serialize};
use windows::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPIF_SENDCHANGE, SPI_GETMOUSE, SPI_SETMOUSE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
};

/// `[threshold 1, threshold 2, acceleration]` as Windows has it now; acceleration 0 is off. Reads only.
pub fn read() -> Option<[i32; 3]> {
    let mut mouse = [0i32; 3];
    unsafe { SystemParametersInfoW(SPI_GETMOUSE, 0, Some(mouse.as_mut_ptr().cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) }
        .ok()
        .map(|_| mouse)
}

fn write(mut mouse: [i32; 3]) -> bool {
    // Told to every program, but not saved to the user's profile: signing out
    // brings their own value back even if the app never gets to.
    unsafe { SystemParametersInfoW(SPI_SETMOUSE, 0, Some(mouse.as_mut_ptr().cast()), SPIF_SENDCHANGE) }.is_ok()
}

#[derive(Serialize, Deserialize, Default)]
struct Journal {
    /// What Windows had before the app changed it.
    mouse: Option<[i32; 3]>,
}

/// `off`: acceleration goes off, the value it had written to `journal` first.
/// Otherwise the value in the journal, if there is one, is put back and the
/// journal removed: the switch being turned off, the app quitting, or the
/// launch after a crash.
pub fn apply(journal: &Path, off: bool) -> Result<(), String> {
    let kept: Journal = json_load(journal);
    if off {
        let now = read().ok_or("Windows did not tell its pointer setting.")?;
        if kept.mouse.is_none() {
            json_save_atomic(journal, &Journal { mouse: Some(now) }).map_err(|e| format!("The pointer setting could not be written down first: {e}"))?;
        }
        if now[2] != 0 && !write([now[0], now[1], 0]) {
            return Err("Windows refused the pointer setting.".into());
        }
    } else if let Some(was) = kept.mouse {
        if !write(was) {
            return Err("Windows refused the pointer setting.".into());
        }
        let _ = std::fs::remove_file(journal);
    }
    Ok(())
}

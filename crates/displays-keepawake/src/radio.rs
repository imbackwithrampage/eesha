// Bluetooth and Wi-Fi radios (Windows.Devices.Radios). Blocking: call from a
// thread of your own, never from a window-message thread.

use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Bluetooth,
    Wifi,
}

impl Kind {
    fn raw(self) -> RadioKind {
        match self {
            Kind::Bluetooth => RadioKind::Bluetooth,
            Kind::Wifi => RadioKind::WiFi,
        }
    }
}

fn radios(kind: Kind) -> Result<Vec<Radio>, String> {
    let e = |e: windows::core::Error| e.message();
    let access = Radio::RequestAccessAsync().map_err(e)?.get().map_err(e)?;
    if access != RadioAccessStatus::Allowed {
        return Err("Radio access is turned off in the Windows privacy settings.".into());
    }
    let all = Radio::GetRadiosAsync().map_err(e)?.get().map_err(e)?;
    Ok(all.into_iter().filter(|r| r.Kind().ok() == Some(kind.raw())).collect())
}

/// One entry per radio of the kind: is it on. Empty = no such radio.
pub fn states(kind: Kind) -> Result<Vec<bool>, String> {
    Ok(radios(kind)?.iter().map(|r| r.State().ok() == Some(RadioState::On)).collect())
}

pub fn set(kind: Kind, on: bool) -> Result<(), String> {
    let state = if on { RadioState::On } else { RadioState::Off };
    for r in radios(kind)? {
        r.SetStateAsync(state).and_then(|op| op.get()).map_err(|e| e.message())?;
    }
    Ok(())
}

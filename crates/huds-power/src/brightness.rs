// Brightness of the built-in panel, as WMI tells it. The brightness keys are
// handled by firmware and never reach an app, so the level is observed, not
// intercepted: `WmiMonitorBrightnessEvent` in root\WMI fires on every change
// (keys, the Windows slider, the power plan). The class does not exist on a PC
// without such a panel: `listen` then returns an error at once.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

use windows::core::{w, IUnknown, Interface, Result, BSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Wmi::{
    IWbemLocator, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_GENERIC_FLAG_TYPE,
};

/// Blocks, calling `on` with every new level (0–100), until `stop` is set.
pub fn listen(stop: &AtomicBool, on: impl Fn(u8)) -> Result<()> {
    unsafe {
        let entered = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = events(stop, &on);
        if entered {
            CoUninitialize();
        }
        result
    }
}

unsafe fn events(stop: &AtomicBool, on: &dyn Fn(u8)) -> Result<()> {
    unsafe {
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
        let none = BSTR::new();
        let wmi = locator.ConnectServer(&BSTR::from("ROOT\\WMI"), &none, &none, &none, 0, &none, None)?;
        impersonate(&wmi)?;
        let events = wmi.ExecNotificationQuery(
            &BSTR::from("WQL"),
            &BSTR::from("SELECT * FROM WmiMonitorBrightnessEvent"),
            WBEM_GENERIC_FLAG_TYPE(WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0),
            None,
        )?;
        impersonate(&events)?;
        while !stop.load(Relaxed) {
            let mut row = [None];
            let mut count = 0u32;
            // debt: wakes every two seconds to look at `stop`; an async sink
            // ended with CancelAsyncCall would sleep until the level moves.
            events.Next(2000, &mut row, &mut count).ok()?;
            let Some(event) = row[0].take() else { continue };
            let mut level = VARIANT::default();
            event.Get(w!("Brightness"), 0, &mut level, None, None)?;
            if let Ok(percent) = u32::try_from(&level) {
                on(percent.min(100) as u8);
            }
        }
        Ok(())
    }
}

/// WMI answers a caller it may impersonate; the process default is to be
/// identified only. The setting belongs to this interface's proxy, so the
/// pointer is passed as it is rather than asked for its IUnknown.
unsafe fn impersonate(proxy: &impl Interface) -> Result<()> {
    unsafe {
        let raw = proxy.as_raw();
        let Some(proxy) = IUnknown::from_raw_borrowed(&raw) else { return Ok(()) };
        CoSetProxyBlanket(
            proxy,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            None,
            RPC_C_AUTHN_LEVEL_CALL,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )
    }
}

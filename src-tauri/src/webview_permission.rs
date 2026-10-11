// The island's camera permission (WebView2 `PermissionRequested`).
//
// Without a handler WebView2 answers `getUserMedia` with a prompt of its own,
// which a transparent window that never activates shows badly, and asks again
// on every visit. The Mirror section of the island-extras feature is the one
// thing that uses the camera, so its request is answered here: allowed for the
// app's own pages, and only while that feature runs. Every other request, the
// microphone's above all (dictation records in Rust), is left exactly as
// WebView2 treats it without this file.

use tauri::{AppHandle, Url};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2PermissionRequestedEventArgs3, COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
    COREWEBVIEW2_PERMISSION_STATE_ALLOW,
};
use webview2_com::{take_pwstr, PermissionRequestedEventHandler};
use wv2_core::{Interface, PWSTR};

use crate::{island, log, registry};

/// The feature whose section shows the camera.
const CAMERA_FEATURE: &str = "island-extras";

/// Where the app's own pages come from: Tauri's local host, and the dev server in a dev build.
fn own_origins(app: &AppHandle) -> Vec<String> {
    let mut own = vec!["http://tauri.localhost".to_string(), "https://tauri.localhost".to_string()];
    if tauri::is_dev() {
        if let Some(dev) = &app.config().build.dev_url {
            own.push(dev.origin().ascii_serialization());
        }
    }
    own
}

fn is_own(uri: &str, own: &[String]) -> bool {
    Url::parse(uri).is_ok_and(|u| own.contains(&u.origin().ascii_serialization()))
}

/// Puts the handler on the island's webview, once, at launch.
pub fn install(app: &AppHandle) {
    let Some(win) = island::window(app) else { return };
    let handle = app.clone();
    let own = own_origins(app);
    let result = win.with_webview(move |webview| unsafe {
        let core = match webview.controller().CoreWebView2() {
            Ok(core) => core,
            Err(err) => {
                log::line(format!("camera permission: no webview: {err}"));
                return;
            }
        };
        let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            args.PermissionKind(&mut kind)?;
            if kind != COREWEBVIEW2_PERMISSION_KIND_CAMERA {
                return Ok(());
            }
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            if !is_own(&take_pwstr(uri), &own) || !registry::is_running(&handle, CAMERA_FEATURE) {
                return Ok(());
            }
            // Not remembered by the browser profile: every request comes back here,
            // so the camera goes with the feature when it is switched off.
            if let Ok(newer) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                newer.SetSavesInProfile(false)?;
            }
            args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
            log::line("camera allowed for the island's Mirror section");
            Ok(())
        }));
        let mut token = 0i64;
        if let Err(err) = core.add_PermissionRequested(&handler, &mut token) {
            log::line(format!("camera permission: cannot listen: {err}"));
        }
    });
    if let Err(err) = result {
        log::line(format!("camera permission: {err}"));
    }
}

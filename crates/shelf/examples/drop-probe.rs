// Proves what the shelf's drop of a mail attachment rests on, with windows of
// the probe's own and nothing of the app's:
//
//   cargo run -p notch-feat-shelf --example shelf-drop-probe -- <scratch folder>
//
// A mail client does not drag a file out: it offers names (`FileGroupDescriptorW`)
// and bytes on request (`FileContents`). The island's page is a WebView2, which
// takes drops itself (src-tauri/src/webview_drop.rs), so the shelf never sees
// that offer: it sees what WebView2 makes of it. This probe makes the same
// drop happen for real. One window plays the mail client and starts an OLE
// drag of two offered files; another holds a WebView2 whose page does what the
// shelf's page does with a drop (the File objects handed back to the host);
// the pointer is moved from the one to the other with the button held. It then
// prints what arrived, which is what `take_files` in src/lib.rs goes by: where
// WebView2 says the files are, and under what names.
//
// The WebView2's data goes under <scratch folder>; the pointer is put back and
// the window that was in front gets the keyboard back.

use std::num::NonZeroIsize;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use webview2_com::Microsoft::Web::WebView2::Win32::{ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs2};
use webview2_com::{take_pwstr, WebMessageReceivedEventHandler};
use windows::core::w;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Ole::{OleInitialize, DROPEFFECT_COPY};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MOVE, MOUSEINPUT, MOUSE_EVENT_FLAGS,
};
use windows::Win32::UI::Shell::SHDoDragDrop;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetAncestor, GetCursorPos, GetForegroundWindow, GetSystemMetrics, SetForegroundWindow, ShowWindow,
    WindowFromPoint, GA_ROOT, SM_CXSCREEN, SM_CYSCREEN, SW_SHOWNOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use wry::raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle};
use wry::{WebContext, WebViewBuilder, WebViewExtWindows};
use wv2_core::{Interface, PWSTR};

#[allow(dead_code)]
#[path = "../../svc-shell/examples/probe/offer.rs"]
mod offer;
#[allow(dead_code)]
#[path = "../../svc-shell/examples/probe/stage.rs"]
mod stage;
use stage::{describe, ours, pump, rect_of, window};

/// What the shelf's page does with a drop (src/features/shelf/index.ts, `onDrop`).
const PAGE: &str = r#"<!doctype html><meta charset="utf-8">
<body style="margin:0;height:100vh;display:grid;place-items:center;background:#141518;color:#c9cdd4;font:15px system-ui">
drop here
<script>
addEventListener("dragenter", (e) => e.preventDefault());
addEventListener("dragover", (e) => { e.preventDefault(); e.dataTransfer.dropEffect = "copy"; });
addEventListener("drop", (e) => {
  e.preventDefault();
  const files = e.dataTransfer.files;
  window.ipc.postMessage(JSON.stringify({ names: Array.from(files, (f) => f.name), sizes: Array.from(files, (f) => f.size), types: Array.from(e.dataTransfer.types) }));
  chrome.webview.postMessageWithAdditionalObjects("probe-drop", files);
});
window.ipc.postMessage("ready");
</script>"#;

struct Host(isize);

impl HasWindowHandle for Host {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?));
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, at: Option<POINT>) {
    // Where on the main screen, in the 0…65535 a mouse event counts in.
    let (dx, dy) = at.map_or((0, 0), |p| unsafe { (p.x * 65536 / GetSystemMetrics(SM_CXSCREEN) + 1, p.y * 65536 / GetSystemMetrics(SM_CYSCREEN) + 1) });
    let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, dwFlags: flags, ..Default::default() } } };
    unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
}

fn middle(window: HWND) -> POINT {
    let r = rect_of(window);
    POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 }
}

fn under(at: POINT) -> HWND {
    unsafe { GetAncestor(WindowFromPoint(at), GA_ROOT) }
}

fn main() {
    let Some(scratch) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: shelf-drop-probe <scratch folder>");
        std::process::exit(2);
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = OleInitialize(None);
    }
    let found = unsafe { GetForegroundWindow() };
    println!("in front at the start: {}", describe(found));

    // The island's stand-in: a window with a WebView2 in it that takes drops itself.
    let host = window(w!("shelf-drop-probe: the island"), WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP, (900, 320), (520, 300));
    let said: Arc<Mutex<Vec<String>>> = Arc::default();
    let paths: Arc<Mutex<Option<Vec<String>>>> = Arc::default();
    let mut context = WebContext::new(Some(scratch.join("drop-probe-webview")));
    let heard = said.clone();
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_html(PAGE)
        .with_ipc_handler(move |message| heard.lock().unwrap().push(message.body().clone()))
        .build(&Host(host.0 as isize))
        .expect("a WebView2");
    // As src-tauri/src/webview_drop.rs does: the File objects of the page's message, as paths.
    let got = paths.clone();
    let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let Ok(with_objects) = args.cast::<ICoreWebView2WebMessageReceivedEventArgs2>() else { return Ok(()) };
        let Ok(objects) = (unsafe { with_objects.AdditionalObjects() }) else { return Ok(()) };
        let mut count = 0u32;
        unsafe { objects.Count(&mut count)? };
        if count == 0 {
            return Ok(());
        }
        let mut list = Vec::new();
        for i in 0..count {
            let Ok(file) = unsafe { objects.GetValueAtIndex(i) }?.cast::<ICoreWebView2File>() else { continue };
            let mut path = PWSTR::null();
            unsafe { file.Path(&mut path)? };
            list.push(take_pwstr(path));
        }
        *got.lock().unwrap() = Some(list);
        Ok(())
    }));
    unsafe {
        let mut token = 0i64;
        webview.controller().CoreWebView2().expect("core").add_WebMessageReceived(&handler, &mut token).expect("listen");
        let _ = ShowWindow(host, SW_SHOWNOACTIVATE);
    }
    let until = Instant::now() + Duration::from_secs(20);
    while !said.lock().unwrap().iter().any(|m| m == "ready") && Instant::now() < until {
        pump(100);
    }
    if !said.lock().unwrap().iter().any(|m| m == "ready") {
        println!("the page never came up: stopping");
        return;
    }
    pump(500);

    // The mail client's stand-in, on a thread of its own: a drag runs on the thread that starts it.
    let report: Vec<u8> = (0..150_000u32).flat_map(|n| n.to_le_bytes()).collect();
    let photo = b"\xFF\xD8\xFF\xE0 not really a photo, but the bytes that were offered".to_vec();
    let (made, ready) = mpsc::channel::<isize>();
    let (go, start) = mpsc::channel::<()>();
    let (sent_report, sent_photo) = (report.clone(), photo.clone());
    let mail = std::thread::spawn(move || unsafe {
        let _ = OleInitialize(None);
        let source = window(w!("shelf-drop-probe: the mail"), WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP, (300, 360), (320, 220));
        let _ = ShowWindow(source, SW_SHOWNOACTIVATE);
        let _ = made.send(source.0 as isize);
        while start.try_recv().is_err() {
            pump(20);
        }
        let offered = offer::offer(&[("Quarterly report.pdf", &sent_report), ("photo of a cat.jpg", &sent_photo)]);
        // Returns when the button is released.
        let effect = SHDoDragDrop(Some(source), &offered, None, DROPEFFECT_COPY);
        println!("the drag ended: {}", effect.map_or_else(|e| format!("FAILED ({})", e.message()), |e| if e.contains(DROPEFFECT_COPY) { "taken (copy)".to_string() } else { "not taken".to_string() }));
        let _ = DestroyWindow(source);
    });
    let source = HWND(ready.recv().expect("the mail window") as *mut _);
    pump(300);

    // The hand: press on the mail, carry to the island, let go. On a thread of its own, so that both windows go on answering.
    let (from, to) = (middle(source), middle(host));
    let (source_raw, host_raw) = (source.0 as isize, host.0 as isize);
    let hand = std::thread::spawn(move || {
        let mut old = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut old);
        }
        let step = |at: POINT| {
            mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, Some(at));
            std::thread::sleep(Duration::from_millis(25));
        };
        step(from);
        if under(from).0 as isize != source_raw {
            println!("another window is over the mail window: nothing pressed");
            let _ = go.send(());
            step(old);
            return;
        }
        mouse(MOUSEEVENTF_LEFTDOWN, None);
        std::thread::sleep(Duration::from_millis(150));
        let _ = go.send(());
        std::thread::sleep(Duration::from_millis(300));
        for i in 1..=30 {
            step(POINT { x: from.x + (to.x - from.x) * i / 30, y: from.y + (to.y - from.y) * i / 30 });
        }
        // A few moves on the spot: the page hears dragover and says it takes a copy.
        for i in 0..12 {
            step(POINT { x: to.x + i % 2, y: to.y });
        }
        // Let go only over the probe's own window; anywhere else, back on the mail window, where nothing takes it.
        if under(to).0 as isize != host_raw {
            println!("another window is over the island's stand-in: carried back, nothing dropped");
            step(from);
        }
        mouse(MOUSEEVENTF_LEFTUP, None);
        std::thread::sleep(Duration::from_millis(100));
        step(old);
    });

    let until = Instant::now() + Duration::from_secs(12);
    while paths.lock().unwrap().is_none() && Instant::now() < until {
        pump(100);
    }
    pump(300);
    let _ = hand.join();
    let _ = mail.join();

    let page: Vec<String> = said.lock().unwrap().iter().filter(|m| m.starts_with('{')).cloned().collect();
    println!("the page saw: {}", page.first().map_or("NO DROP", |m| m.as_str()));
    let names: Vec<String> = page.first().map_or_else(Vec::new, |m| {
        // The names, without a JSON parser: between `"names":[` and `]`.
        let list = m.split("\"names\":[").nth(1).and_then(|rest| rest.split(']').next()).unwrap_or_default();
        list.split("\",\"").map(|name| name.trim_matches('"').to_string()).filter(|name| !name.is_empty()).collect()
    });
    let temp = std::fs::canonicalize(std::env::temp_dir()).map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
    match paths.lock().unwrap().clone() {
        None => println!("WebView2 handed over: NOTHING"),
        Some(list) => {
            println!("WebView2 handed over {} path(s) for {} name(s)", list.len(), names.len());
            for (i, path) in list.iter().enumerate() {
                let path = Path::new(path);
                let own = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let name = names.get(i).cloned().unwrap_or_default();
                let bytes = std::fs::read(path).unwrap_or_default();
                // Right in the temp folder, not in a folder below it: what `take_files` asks.
                let in_temp = path.parent().and_then(|folder| std::fs::canonicalize(folder).ok()).is_some_and(|folder| folder.to_string_lossy().to_lowercase() == temp);
                // What was offered, or that with the rest of the block it came in (the engine does not cut it to the size the offer names).
                let as_offered = bytes == report || bytes.starts_with(&photo);
                let shown = std::fs::canonicalize(path).map(|p| p.to_string_lossy().to_lowercase().replacen(&temp, "%TEMP%", 1)).unwrap_or_default();
                println!("  \"{name}\" arrives as {shown}");
                println!("    under its own name: {}, right in the temp folder: {in_temp}, {} bytes, the bytes that were offered: {as_offered}", own.to_lowercase() == name.to_lowercase(), bytes.len());
                println!("    so the shelf: {}", if in_temp { "keeps a copy of its own (take_files)" } else { "takes the path as it is" });
            }
        }
    }

    unsafe {
        let front = GetForegroundWindow();
        if front != found && (front.0.is_null() || ours(front)) {
            let _ = SetForegroundWindow(found);
            pump(200);
        }
        println!("in front at the end: {} (as it was found: {})", describe(GetForegroundWindow()), GetForegroundWindow() == found);
        drop(webview);
        let _ = DestroyWindow(host);
    }
}

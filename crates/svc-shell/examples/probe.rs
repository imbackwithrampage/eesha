// Proves svc-shell against the real Windows, one part at a time:
//
//   cargo run -p svc-shell --example shell-probe -- <path>
//       pictures, zip, "Open with", the formats of a drag, the Share package
//   … -- fresh <picture>…      thumbnails of files that have only just landed
//   … -- share <file>          the Share sheet: the owners that get nothing, then a window of its own
//   … -- incoming <folder>     offered files (a mail's attachments), text, a link, a picture; the clipboard's kinds
//   … -- files <folder>        the rules for a name, rename, the Recycle Bin
//   … -- pick <file>           the file picker, opened and answered
//   … -- menus <folder>        the right-click and "Send to" entries, in a key and a folder of the probe's own
//
// The first mode writes <path>.thumb.png, <path>.icon.png and <path>.zip beside
// the file and shows nothing. `share` and `pick` put windows of the probe's own
// on the screen, click on them and close what they opened; the window that was
// in front gets the keyboard back. `files` recycles two things it made itself
// and takes them out of the bin again. `menus` writes under
// HKCU\Software\shell-probe-scratch-<pid> and in <folder>\SendTo, and removes
// both; the user's real menus are only looked at. Nothing reads what the
// clipboard holds beyond what kind of thing it is.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use svc_shell::{Item, MenuEntry, Places};
use windows::core::{factory, w, HSTRING, PCWSTR};
use windows::ApplicationModel::DataTransfer::{DataPackage, DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{IDataObject, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM};
use windows::Win32::System::DataExchange::{GetClipboardFormatNameW, RegisterClipboardFormatW};
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize, CF_UNICODETEXT};
use windows::Win32::System::Registry::{RegDeleteTreeW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Shell::{IDataTransferManagerInterop, SHCreateDataObject};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyWindow, GetDlgItem, GetForegroundWindow, PostMessageW, SendMessageW, SetForegroundWindow, ShowWindow,
    SW_SHOW, SW_SHOWNOACTIVATE, WM_CLOSE, WM_COMMAND, WM_SETTEXT, WM_USER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_POPUP,
};

#[path = "probe/offer.rs"]
mod offer;
#[path = "probe/stage.rs"]
mod stage;
use offer::{global, offer};
use stage::{class_of, click, describe, ours, pump, rect_of, visible, window};

fn beside(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

fn png(label: &str, made: Result<Vec<u8>, String>, to: &Path) {
    match made {
        Ok(bytes) if bytes.starts_with(b"\x89PNG") && bytes.len() > 24 => {
            let side = |at: usize| u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
            std::fs::write(to, &bytes).expect("write png");
            println!("{label}: {}x{} px, {} bytes -> {}", side(16), side(20), bytes.len(), to.display());
        }
        Ok(bytes) => println!("{label}: NOT A PNG ({} bytes)", bytes.len()),
        Err(err) => println!("{label}: none ({err})"),
    }
}

fn format_names(object: &IDataObject) -> Vec<String> {
    let mut names = Vec::new();
    unsafe {
        let Ok(list) = object.EnumFormatEtc(DATADIR_GET.0 as u32) else { return names };
        loop {
            let mut one = [FORMATETC::default()];
            let mut got = 0u32;
            if list.Next(&mut one, Some(&mut got)).is_err() || got == 0 {
                break;
            }
            let mut name = [0u16; 96];
            let len = GetClipboardFormatNameW(one[0].cfFormat as u32, &mut name);
            names.push(match one[0].cfFormat {
                13 => "CF_UNICODETEXT".to_string(),
                15 => "CF_HDROP".to_string(),
                other if len == 0 => format!("#{other}"),
                _ => String::from_utf16_lossy(&name[..len as usize]),
            });
        }
    }
    names
}

fn formats(label: &str, items: &[Item]) {
    match svc_shell::data_object(items) {
        Ok(object) => println!("drag of {label}: {}", format_names(&object).join(", ")),
        Err(err) => println!("drag of {label}: FAILED ({err})"),
    }
}

/// The window that had the keyboard when the probe began gets it back, if a window of the probe's has it now.
fn give_back(to: HWND) {
    unsafe {
        let front = GetForegroundWindow();
        if front != to && (front.0.is_null() || ours(front)) {
            let _ = SetForegroundWindow(to);
            pump(200);
        }
        println!("in front at the end: {} (as it was found: {})", describe(GetForegroundWindow()), GetForegroundWindow() == to);
    }
}

// ── The first mode: one file, nothing on screen ───────────────────────────────

fn one_file(path: &Path) {
    let path = svc_shell::shell_path(path);
    println!("file: {}", path.display());

    png("thumbnail", svc_shell::thumbnail(&path, 192), &beside(&path, ".thumb.png"));
    png("icon", svc_shell::icon(&path.to_string_lossy(), 64), &beside(&path, ".icon.png"));
    let again = std::time::Instant::now();
    let _ = svc_shell::icon(&path.to_string_lossy(), 64);
    println!("icon again (cached): {} us", again.elapsed().as_micros());

    let zip = beside(&path, ".zip");
    match svc_shell::zip(std::slice::from_ref(&path), &zip) {
        Ok(()) => println!("zip: {} bytes -> {}", std::fs::metadata(&zip).map_or(0, |m| m.len()), zip.display()),
        Err(err) => println!("zip: FAILED ({err})"),
    }

    let handlers = svc_shell::open_with_menu(&path);
    println!("open with: {} handler(s)", handlers.len());
    for handler in &handlers {
        println!("  {}  [{}]", handler.name, handler.exe);
    }

    unsafe {
        let _ = OleInitialize(None);
    }
    formats("the file", &[Item::File(path.clone())]);
    formats("a text", &[Item::Text("hello".into())]);
    formats("a link", &[Item::Link("https://example.com/".into())]);
    unsafe { OleUninitialize() };

    match svc_shell::share_package(&[Item::File(path.clone())]) {
        Ok(_) => println!("share package: ok"),
        Err(err) => println!("share package: FAILED ({err})"),
    }
}

// ── fresh: pictures that have only just landed ────────────────────────────────

fn said(made: Result<Vec<u8>, String>) -> String {
    match made {
        Ok(png) => format!("picture, {} bytes", png.len()),
        Err(err) => format!("NO PICTURE ({err})"),
    }
}

fn fresh(sources: &[PathBuf]) {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let copy = |kind: &str, n: usize, from: &Path| {
        let ext = from.extension().unwrap_or_default().to_string_lossy().into_owned();
        let to = svc_shell::shell_path(&from.with_file_name(format!("{kind}-{stamp}-{n}.{ext}")));
        std::fs::copy(from, &to).expect("copy");
        to
    };

    // Each asked for twice at once, as the page does when two draws overlap.
    let copies: Vec<PathBuf> = sources.iter().enumerate().map(|(n, from)| copy("fresh", n, from)).collect();
    let began = std::time::Instant::now();
    std::thread::scope(|scope| {
        for path in &copies {
            for call in 0..2 {
                scope.spawn(move || {
                    let made = said(svc_shell::thumbnail(path, 192));
                    println!("fresh {} call {call}: {made} after {} ms", path.file_name().unwrap_or_default().to_string_lossy(), began.elapsed().as_millis());
                });
            }
        }
    });
    for path in &copies {
        let _ = std::fs::remove_file(path);
    }

    // The same file named the way a script names it.
    for (n, from) in sources.iter().enumerate() {
        let to = copy("slashes", n, from);
        let slashed = to.to_string_lossy().replace('\\', "/");
        println!("written with / ({slashed}): {}; icon: {}", said(svc_shell::thumbnail(Path::new(&slashed), 192)), said(svc_shell::icon(&slashed, 64)));
        let _ = std::fs::remove_file(&to);
    }

    // A file its writer still holds for itself (a download, a copy, the virus scanner).
    use std::os::windows::fs::OpenOptionsExt;
    for (n, from) in sources.iter().enumerate() {
        let to = copy("held", n, from);
        let held = std::fs::OpenOptions::new().write(true).share_mode(0).open(&to).expect("hold");
        let (path, t) = (to.clone(), std::time::Instant::now());
        // Let go of after 250 ms: inside the time `thumbnail` waits for it.
        let asking = std::thread::spawn(move || (said(svc_shell::thumbnail(&path, 192)), t.elapsed().as_millis()));
        std::thread::sleep(std::time::Duration::from_millis(250));
        drop(held);
        let (made, after) = asking.join().expect("thumbnail");
        println!("held for 250 ms by its writer {n}: {made} after {after} ms");

        let to2 = copy("held-long", n, from);
        let held = std::fs::OpenOptions::new().write(true).share_mode(0).open(&to2).expect("hold");
        let t = std::time::Instant::now();
        println!("held throughout {n}: {} after {} ms; its icon: {}", said(svc_shell::thumbnail(&to2, 192)), t.elapsed().as_millis(), said(svc_shell::icon(&to2.to_string_lossy(), 64)));
        drop(held);
        println!("asked again once it is let go {n}: {}", said(svc_shell::thumbnail(&to2, 192)));
        let _ = std::fs::remove_file(&to);
        let _ = std::fs::remove_file(&to2);
    }
}

// ── share ─────────────────────────────────────────────────────────────────────

/// What came on screen since `before` that is not the probe's: the sheet, if anything.
fn sheet_since(before: &[isize]) -> Option<HWND> {
    visible().into_iter().filter(|h| !before.contains(h)).map(|h| HWND(h as *mut c_void)).find(|h| {
        let r = rect_of(*h);
        !ours(*h) && r.right - r.left > 50 && !class_of(*h).contains("IME")
    })
}

/// The old way: the sheet asked for with `owner` as its window.
fn old_way(label: &str, owner: HWND, package: &DataPackage) {
    let before = visible();
    let answer = (|| unsafe {
        let interop: IDataTransferManagerInterop = factory::<DataTransferManager, IDataTransferManagerInterop>()?;
        let manager: DataTransferManager = interop.GetForWindow(owner)?;
        let data = package.clone();
        let token = manager.DataRequested(&TypedEventHandler::<DataTransferManager, DataRequestedEventArgs>::new(move |_, args| {
            args.ok()?.Request()?.SetData(&data)
        }))?;
        let shown = interop.ShowShareUIForWindow(owner);
        pump(1800);
        let _ = manager.RemoveDataRequested(token);
        shown
    })();
    let sheet = sheet_since(&before);
    println!(
        "share, {label}: Windows answers {}; sheet: {}",
        match answer {
            Ok(()) => "\"done\"".to_string(),
            Err(err) => format!("{:#010x}", err.code().0),
        },
        sheet.map_or("NONE".to_string(), describe)
    );
    if let Some(sheet) = sheet {
        unsafe {
            let _ = PostMessageW(Some(sheet), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        pump(1500);
    }
}

fn share(file: &Path) {
    unsafe {
        let _ = OleInitialize(None);
        let found = GetForegroundWindow();
        println!("in front at the start: {}", describe(found));
        let package = svc_shell::share_package(&[Item::File(file.to_path_buf())]).expect("package");

        // 1. The settings window as it is at launch: never shown.
        let hidden = window(w!("shell-probe: hidden owner"), WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, (200, 200), (400, 300));
        old_way("owner never shown (the settings window)", hidden, &package);

        // 2. The island under the pointer: on screen, on top, never the window in front.
        let island = window(w!("shell-probe: island"), WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP, (700, 0), (640, 192));
        let _ = ShowWindow(island, SW_SHOWNOACTIVATE);
        pump(200);
        old_way("owner that does not take the keyboard (the island)", island, &package);

        // The fix, where the old way got nothing: another program in front, the island never clicked.
        let r = rect_of(island);
        let before = visible();
        let asked = svc_shell::share_sheet(&package, ((r.left + r.right) / 2, r.bottom + 12 * GetDpiForWindow(island) as i32 / 96));
        pump(2500);
        let sheet = sheet_since(&before);
        println!(
            "share, a window of its own, another program in front: {}; sheet: {}",
            asked.map_or_else(|err| format!("FAILED ({err})"), |()| "ok".to_string()),
            sheet.map_or("NONE".to_string(), describe)
        );
        if let Some(sheet) = sheet {
            let _ = PostMessageW(Some(sheet), WM_CLOSE, WPARAM(0), LPARAM(0));
            pump(2500);
            println!("share, the keyboard afterwards: {} (back where it was: {})", describe(GetForegroundWindow()), GetForegroundWindow() == found);
        }
        let _ = DestroyWindow(island);

        // 3. The island with the keyboard (a click gave it): the hidden window is still asked first.
        let island = window(w!("shell-probe: island"), WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP, (700, 0), (640, 192));
        let _ = ShowWindow(island, SW_SHOW);
        pump(200);
        if !click(island) || GetForegroundWindow() != island {
            println!("share: the probe's window did not get the keyboard: stopping here");
            let _ = DestroyWindow(island);
            let _ = DestroyWindow(hidden);
            return give_back(found);
        }
        old_way("owner never shown, the island in front", hidden, &package);
        // The call other packages make, `share(items, window)`, with that same hidden window.
        let before = visible();
        let asked = svc_shell::share(&[Item::File(file.to_path_buf())], hidden.0 as isize);
        pump(2500);
        let sheet = sheet_since(&before);
        println!(
            "share(items, a window never shown): {}; sheet: {}",
            asked.map_or_else(|err| format!("FAILED ({err})"), |()| "ok".to_string()),
            sheet.map_or("NONE".to_string(), describe)
        );
        if let Some(sheet) = sheet {
            let _ = PostMessageW(Some(sheet), WM_CLOSE, WPARAM(0), LPARAM(0));
            pump(2500);
        }
        let _ = DestroyWindow(hidden);

        // 4. The fix: a window of the sheet's own, under the island.
        click(island);
        // Where the shelf asks for it (crates/shelf, `sheet_point`): hanging under the middle of the island.
        let r = rect_of(island);
        let at = ((r.left + r.right) / 2, r.bottom + 12 * GetDpiForWindow(island) as i32 / 96);
        let before = visible();
        let asked = svc_shell::share_sheet(&package, at);
        pump(300);
        // As the island does once another window is in front: it stops taking the keyboard.
        let _ = DestroyWindow(island);
        let island = window(w!("shell-probe: island"), WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP, (700, 0), (640, 192));
        let _ = ShowWindow(island, SW_SHOWNOACTIVATE);
        pump(2700);
        let sheet = sheet_since(&before);
        println!(
            "share, a window of its own, under {},{}: {}; sheet: {}; in front: {}",
            at.0,
            at.1,
            asked.map_or_else(|err| format!("FAILED ({err})"), |()| "ok".to_string()),
            sheet.map_or("NONE".to_string(), describe),
            describe(GetForegroundWindow())
        );
        if let Some(sheet) = sheet {
            let s = rect_of(sheet);
            println!("share, the sheet under the island: {} px below it, its middle {} px from the island's", s.top - r.bottom, (s.left + s.right) / 2 - (r.left + r.right) / 2);
            let _ = PostMessageW(Some(sheet), WM_CLOSE, WPARAM(0), LPARAM(0));
            pump(2500);
            println!("share, the sheet closed again: {}", !visible().contains(&(sheet.0 as isize)));
        }
        let left: Vec<String> = visible().into_iter().map(|h| HWND(h as *mut c_void)).filter(|h| ours(*h) && *h != island).map(describe).collect();
        println!("share, windows of the share left behind: {}", if left.is_empty() { "none".to_string() } else { left.join(" ") });
        println!("share, the keyboard afterwards: {} (back where it was: {})", describe(GetForegroundWindow()), GetForegroundWindow() == found);
        give_back(found);
        let _ = DestroyWindow(island);
    }
}

// ── incoming ──────────────────────────────────────────────────────────────────

unsafe fn put(object: &IDataObject, format: u16, medium: STGMEDIUM) {
    let what = FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: medium.tymed };
    // `true`: the object owns the medium from here.
    unsafe { object.SetData(&what, &medium, true).expect("SetData") };
}

fn kind(item: &Item) -> &'static str {
    match item {
        Item::File(_) => "file",
        Item::Text(_) => "text",
        Item::Link(_) => "link",
    }
}

fn incoming(dir: &Path) {
    let dir = svc_shell::shell_path(dir).join("incoming");
    let _ = std::fs::remove_dir_all(&dir);
    unsafe {
        let _ = OleInitialize(None);
        let report: Vec<u8> = (0..200_000u32).flat_map(|n| n.to_le_bytes()).collect();
        let photo = b"\xFF\xD8\xFF\xE0 not really a photo".to_vec();
        let offered = offer(&[("Quarterly report.pdf", &report), (r"..\evil?.jpg", &photo)]);
        println!("offered: {}", format_names(&offered).join(", "));
        for round in 1..=2 {
            for item in svc_shell::incoming(&offered, &dir, "Pasted picture") {
                let Item::File(path) = &item else {
                    println!("offered files, round {round}: UNEXPECTED {}", kind(&item));
                    continue;
                };
                let got = std::fs::read(path).unwrap_or_default();
                let same = got == report || got == photo;
                println!("offered files, round {round}: {} ({} bytes, as sent: {same})", path.strip_prefix(&dir).unwrap_or(path).display(), got.len());
            }
        }

        let show = |label: &str, object: &IDataObject| {
            let items = svc_shell::incoming(object, &dir, "Pasted picture");
            let said: Vec<String> = items
                .iter()
                .map(|item| match item {
                    Item::File(path) => format!("file {}", path.file_name().unwrap_or_default().to_string_lossy()),
                    Item::Text(text) => format!("text {text:?}"),
                    Item::Link(url) => format!("link {url}"),
                })
                .collect();
            println!("{label}: {}", if said.is_empty() { "nothing".to_string() } else { said.join(", ") });
        };
        show("a file that exists", &svc_shell::data_object(&[Item::File(std::env::current_exe().expect("exe"))]).expect("object"));
        show("a link", &svc_shell::data_object(&[Item::Link("https://example.com/a".into())]).expect("object"));
        show("a text", &svc_shell::data_object(&[Item::Text("two\r\nlines".into())]).expect("object"));
        let picture: IDataObject = SHCreateDataObject(None, None, None).expect("data object");
        put(&picture, RegisterClipboardFormatW(w!("PNG")) as u16, global(b"\x89PNG\r\n\x1a\n stand-in"));
        show("a picture that is nobody's file", &picture);
        // Excel's way: the text, and a picture of it.
        let both: IDataObject = SHCreateDataObject(None, None, None).expect("data object");
        put(&both, RegisterClipboardFormatW(w!("PNG")) as u16, global(b"\x89PNG\r\n\x1a\n stand-in"));
        let text: Vec<u8> = "a cell".encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        put(&both, CF_UNICODETEXT.0, global(&text));
        show("text with a picture of it", &both);
        OleUninitialize();
    }
    // The real clipboard, read only, and only what kind of thing it holds.
    let now = svc_shell::clipboard(&dir, "Pasted picture");
    let kinds: Vec<&str> = now.iter().map(kind).collect();
    println!("the clipboard now: {} item(s) [{}] (what they are is not shown)", now.len(), kinds.join(", "));
    let _ = std::fs::remove_dir_all(&dir);
}

// ── files: a name, the Recycle Bin ────────────────────────────────────────────

fn files(dir: &Path) {
    let dir = svc_shell::shell_path(dir).join("files");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("folder");

    for name in ["report.pdf", "  spaced out.txt  ", "", "   ", "a/b.txt", "what?.txt", "tab\there", "CON", "nul.txt", "COM7", "LPT10", "ends.", "..", ".gitignore"] {
        println!("name {name:?}: {}", svc_shell::name_problem(name).map_or("fine".to_string(), |p| format!("refused ({})", p.code())));
    }

    let first = dir.join("probe one.txt");
    std::fs::write(&first, "one").expect("write");
    std::fs::write(dir.join("taken.txt"), "other").expect("write");
    let said = |what: &str, done: Result<PathBuf, svc_shell::NameProblem>| match done {
        Ok(path) => {
            let listed = std::fs::read_dir(&dir).into_iter().flatten().flatten().any(|e| e.file_name() == path.file_name().unwrap_or_default());
            println!("rename {what}: now {} (on disk under exactly that name: {listed})", path.file_name().unwrap_or_default().to_string_lossy());
            path
        }
        Err(problem) => {
            println!("rename {what}: refused ({})", problem.code());
            PathBuf::new()
        }
    };
    let renamed = said("to another name", svc_shell::rename(&first, "Probe Renamed.txt"));
    let renamed = said("capitals only", svc_shell::rename(&renamed, "probe renamed.TXT"));
    said("to a name that is taken", svc_shell::rename(&renamed, "taken.txt"));
    said("to a bad name", svc_shell::rename(&renamed, "a:b"));
    said("of a file that is gone", svc_shell::rename(&first, "anything.txt"));
    println!("rename: the first name is free again: {}, content kept: {}", !first.exists(), std::fs::read_to_string(&renamed).is_ok_and(|s| s == "one"));

    // Two things of the probe's own go to the bin, and are taken out of it again.
    let file = dir.join("shell-probe recycle me.txt");
    let folder = dir.join("shell-probe recycle folder");
    std::fs::write(&file, "bin").expect("write");
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(folder.join("inner.txt"), "bin").expect("write");
    match svc_shell::recycle(&[file.clone(), folder.clone(), dir.join("never was.txt")]) {
        Ok(recycled) => {
            println!("recycle: {} of 3 went (one never existed)", recycled.len());
            for one in &recycled {
                let in_bin = one.bin.to_string_lossy().to_uppercase().contains("$RECYCLE.BIN");
                println!(
                    "recycle {}: gone from its folder: {}, lies in the Recycle Bin: {} ({})",
                    one.from.file_name().unwrap_or_default().to_string_lossy(),
                    !one.from.exists(),
                    in_bin && one.bin.exists(),
                    one.bin.file_name().unwrap_or_default().to_string_lossy()
                );
                // Out of the bin again: the item ($R…) and its record ($I…). Only what was just put there.
                if in_bin {
                    let record = one.bin.with_file_name(one.bin.file_name().unwrap_or_default().to_string_lossy().replacen("$R", "$I", 1));
                    let gone = if one.bin.is_dir() { std::fs::remove_dir_all(&one.bin) } else { std::fs::remove_file(&one.bin) };
                    let _ = std::fs::remove_file(&record);
                    println!("  taken out of the bin again: {}", gone.is_ok() && !one.bin.exists() && !record.exists());
                }
            }
        }
        Err(err) => println!("recycle: FAILED ({err})"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ── pick ──────────────────────────────────────────────────────────────────────

fn pick(file: &Path) {
    let file = svc_shell::shell_path(file);
    unsafe {
        let found = GetForegroundWindow();
        println!("in front at the start: {}", describe(found));
        // A click in a window of ours, as the click on the empty shelf is: Windows then lets the dialog come to the front.
        let island = window(w!("shell-probe: island"), WS_EX_TOOLWINDOW | WS_EX_TOPMOST, WS_POPUP, (700, 0), (640, 192));
        let _ = ShowWindow(island, SW_SHOW);
        pump(200);
        click(island);
        let before = visible();
        let picking = std::thread::spawn(|| svc_shell::pick_files("shell-probe: add to the shelf"));
        let mut dialog = None;
        for _ in 0..50 {
            pump(100);
            dialog = visible().into_iter().filter(|h| !before.contains(h)).map(|h| HWND(h as *mut c_void)).find(|h| ours(*h) && class_of(*h) == "#32770");
            if dialog.is_some() {
                break;
            }
        }
        let Some(dialog) = dialog else {
            println!("pick: NO DIALOG came up");
            let _ = DestroyWindow(island);
            return give_back(found);
        };
        pump(600);
        println!("pick: the dialog is up: {}; in front: {}", describe(dialog), GetForegroundWindow() == dialog);
        // The file name goes into the dialog's own field (a message to our own window, no keys), then its Open button.
        let name = HSTRING::from(file.as_os_str());
        let named = GetDlgItem(Some(dialog), 0x47C).ok().map(|combo| {
            let edit = SendMessageW(combo, WM_USER + 7, None, None).0;
            let field = if edit != 0 { HWND(edit as *mut c_void) } else { combo };
            SendMessageW(field, WM_SETTEXT, None, Some(LPARAM(name.as_ptr() as isize))).0 != 0
        });
        match named {
            Some(true) => {
                let _ = PostMessageW(Some(dialog), WM_COMMAND, WPARAM(1), LPARAM(0));
            }
            _ => {
                println!("pick: its file name field was not found: cancelling");
                let _ = PostMessageW(Some(dialog), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        for _ in 0..50 {
            if picking.is_finished() {
                break;
            }
            pump(100);
        }
        if !picking.is_finished() {
            let _ = PostMessageW(Some(dialog), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        let chosen = picking.join().expect("picker");
        println!("pick: chosen {} file(s): {}", chosen.len(), chosen.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
        println!("pick: the file asked for: {}", chosen.len() == 1 && chosen[0] == file);

        // And cancelled: nothing comes back.
        click(island);
        let before = visible();
        let picking = std::thread::spawn(|| svc_shell::pick_files("shell-probe: add to the shelf"));
        for _ in 0..50 {
            pump(100);
            if let Some(dialog) = visible().into_iter().filter(|h| !before.contains(h)).map(|h| HWND(h as *mut c_void)).find(|h| ours(*h) && class_of(*h) == "#32770") {
                pump(400);
                let _ = PostMessageW(Some(dialog), WM_CLOSE, WPARAM(0), LPARAM(0));
                break;
            }
        }
        for _ in 0..50 {
            if picking.is_finished() {
                break;
            }
            pump(100);
        }
        println!("pick: cancelled: {} file(s)", picking.join().map_or(99, |chosen| chosen.len()));
        // While a window of ours still has the keyboard: once it is gone, Windows picks for itself.
        give_back(found);
        let _ = DestroyWindow(island);
    }
}

// ── menus ─────────────────────────────────────────────────────────────────────

fn key_exists(key: &str) -> bool {
    unsafe {
        let mut open = HKEY::default();
        let there = RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(key), None, KEY_READ, &mut open).is_ok();
        if there {
            let _ = windows::Win32::System::Registry::RegCloseKey(open);
        }
        there
    }
}

fn menus(dir: &Path) {
    let scratch = format!(r"Software\shell-probe-scratch-{}", std::process::id());
    let places = Places { classes: format!(r"{scratch}\Classes"), send_to: svc_shell::shell_path(dir).join("SendTo") };
    let _ = std::fs::remove_dir_all(&places.send_to);
    std::fs::create_dir_all(&places.send_to).expect("folder");
    let exe = std::env::current_exe().expect("exe");
    let id = "probe.AddToShelf";
    let listed = |places: &Places| -> Vec<String> {
        std::fs::read_dir(&places.send_to).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    };
    println!("menus: writing under HKCU\\{scratch} and {}", places.send_to.display());

    let entry = MenuEntry { id, label: "Add to Shelf", exe: &exe, arg: "--shelf-add" };
    println!("add: {:?}", svc_shell::add_to_menus(&entry, &places));
    println!("  right-click, files: {:?}", svc_shell::menu_command(id, &places));
    println!("  right-click, folders: key there: {}", key_exists(&format!(r"{}\Directory\shell\{id}\command", places.classes)));
    println!("  Send to: {:?}", listed(&places));
    for link in listed(&places) {
        match svc_shell::shortcut_target(&places.send_to.join(&link)) {
            Ok((target, args)) => println!("  {link} starts: {} {args} (this probe: {})", target.display(), target == exe),
            Err(err) => println!("  {link}: UNREADABLE ({err})"),
        }
    }

    // Another language: the same entry under another label.
    let entry = MenuEntry { label: "Ajouter à l'étagère", ..entry };
    println!("add again, another label: {:?}; Send to: {:?}", svc_shell::add_to_menus(&entry, &places), listed(&places));

    println!("remove: {:?}", svc_shell::remove_from_menus(id, &places));
    println!("  right-click: {:?}; folders key there: {}; Send to: {:?}", svc_shell::menu_command(id, &places), key_exists(&format!(r"{}\Directory\shell\{id}", places.classes)), listed(&places));
    println!("remove again (nothing there): {:?}", svc_shell::remove_from_menus(id, &places));
    println!("an empty id is refused: {}", svc_shell::remove_from_menus("", &places).is_err());

    // The probe's own key and folder go; the real menus were never written to.
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(scratch.as_str()));
        let _ = windows::Win32::System::Registry::RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(HSTRING::from(scratch.as_str()).as_ptr()));
    }
    let _ = std::fs::remove_dir_all(&places.send_to);
    println!("scratch key gone: {}; scratch folder gone: {}", !key_exists(&scratch), !places.send_to.exists());
    match Places::user() {
        Some(user) => println!(
            "the real menus (looked at, not written): a probe entry there: {}; a probe shortcut in {}: {}",
            svc_shell::menu_command(id, &user).is_some(),
            user.send_to.display(),
            user.send_to.join("Add to Shelf.lnk").exists()
        ),
        None => println!("the real SendTo folder was not found"),
    }
}

fn main() {
    unsafe {
        // Real pixels, as the app has them.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let mode = args.first().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
    match (mode.as_str(), args.get(1)) {
        ("fresh", Some(_)) => fresh(&args[1..]),
        ("share", Some(file)) => share(file),
        ("incoming", Some(dir)) => incoming(dir),
        ("files", Some(dir)) => files(dir),
        ("pick", Some(file)) => pick(file),
        ("menus", Some(dir)) => menus(dir),
        (_, None) if !mode.is_empty() && Path::new(&mode).exists() => one_file(Path::new(&mode)),
        _ => {
            eprintln!("usage: shell-probe <path> | fresh <picture>… | share <file> | incoming <folder> | files <folder> | pick <file> | menus <folder>");
            std::process::exit(2);
        }
    }
}

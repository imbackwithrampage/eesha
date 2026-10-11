// Shelf — files, text and links parked in the island until they are wanted
// somewhere else. A file on the shelf is a path to where the file lives: the
// shelf never copies a user's file, and it never deletes one (the one thing
// that removes a file is "Move to Recycle Bin", which the page asks about every
// time). What is on it is kept in items.json under the feature's data folder,
// so it survives a restart.
//
// What has no file of its own gets one in the shelf's own temp folder: a zip
// the shelf made, an attachment dragged out of a mail, a picture pasted from
// the clipboard. Those go when their entry goes.
//
// The contract is docs/ARCHITECTURE.md §2: nothing runs before `start`,
// everything is gone after `stop`, every command arrives through `call`, on a
// thread that is not the page's. The work on files is svc-shell's; the drag
// watcher is watch.rs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notch_core::{i18n, log, storage, Ctx, Feature, Group, Manifest, Monitor, Use, WindowSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use svc_shell::{DropEffect, Item, MenuEntry, Places};
use tauri::Manager;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

mod watch;

pub(crate) const ID: &str = "shelf";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Shelf",
    group: Group::Files,
    default_on: true,
    // The drag watcher: mouse buttons and position, only while "dragDetect" is on.
    // The clipboard: read once for each Ctrl+V on the shelf, never watched.
    // Files: "Move to Recycle Bin", after the page has asked.
    uses: &[Use::InputHook, Use::Clipboard, Use::FileDelete],
    shortcuts: &[],
    tray: &[],
    defaults: r#"{ "dragDetect": true, "explorerMenu": false }"#,
};

/// The preview window (Space).
const PREVIEW: &str = "shelf-preview";

/// "Add to Shelf" in Explorer's menus: the verb's name in the registry, and
/// what the app is started with before the paths.
const MENU_VERB: &str = "AddToShelf";
const MENU_ARG: &str = "--shelf-add";

/// The height of the shelf's section (`height` in src/features/shelf/index.ts), logical pixels.
const SECTION_H: i32 = 192;

// ── What is on the shelf ──────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    File,
    Text,
    Link,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Entry {
    id: String,
    kind: Kind,
    /// The path, the text or the address.
    value: String,
    /// Made by the shelf itself in its own folder: deleted with the entry.
    #[serde(default)]
    temp: bool,
}

/// One writer of items.json at a time.
static WRITING: Mutex<()> = Mutex::new(());

fn store(cx: &Ctx) -> PathBuf {
    cx.data_dir(ID).join("items.json")
}

/// Where the shelf keeps what it made itself.
fn temp_dir(cx: &Ctx) -> PathBuf {
    cx.data_dir(ID).join("temp")
}

fn load(cx: &Ctx) -> Vec<Entry> {
    storage::json_load(&store(cx))
}

/// Loads, changes, saves; the page hears `shelf:changed` when something did change.
fn edit<T>(cx: &Ctx, change: impl FnOnce(&mut Vec<Entry>) -> T) -> T {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut items = load(cx);
    let before = items.clone();
    let out = change(&mut items);
    if items != before {
        if let Err(err) = storage::json_save_atomic(&store(cx), &items) {
            log::line(format!("shelf: cannot save: {err}"));
        }
        cx.emit(ID, "changed", ());
    }
    out
}

fn new_id() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    format!("{now:x}-{:x}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Puts things on the shelf; what is already there (same file, text or address) is not added twice.
fn add(cx: &Ctx, new: Vec<(Kind, String, bool)>) {
    edit(cx, |items| {
        for (kind, value, temp) in new {
            let there = items.iter().any(|e| {
                e.kind == kind && if kind == Kind::File { e.value.to_lowercase() == value.to_lowercase() } else { e.value == value }
            });
            if !there {
                items.push(Entry { id: new_id(), kind, value, temp });
            }
        }
    });
}

fn is_web(text: &str) -> bool {
    (text.starts_with("https://") || text.starts_with("http://")) && !text.contains(char::is_whitespace)
}

/// A text as the shelf keeps it; an address becomes a link.
fn worded(text: &str) -> Option<(Kind, String, bool)> {
    // ponytail: cut at 200 000 characters; anything longer belongs in a file.
    let text: String = text.trim().chars().take(200_000).collect();
    (!text.is_empty()).then(|| (if is_web(&text) { Kind::Link } else { Kind::Text }, text, false))
}

/// Files named by their paths, as the shelf keeps them: written the way
/// Windows writes a path, so that the same file is the same entry however it
/// was named, and the shell takes it (svc_shell::shell_path).
fn real_files<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<PathBuf> {
    paths.map(Path::new).filter(|p| p.is_absolute() && p.exists()).map(svc_shell::shell_path).collect()
}

fn filed(path: PathBuf, temp: bool) -> (Kind, String, bool) {
    (Kind::File, path.to_string_lossy().into_owned(), temp)
}

/// An entry as the page draws it.
fn view(entry: &Entry) -> Value {
    match entry.kind {
        Kind::File => {
            let path = Path::new(&entry.value);
            let meta = std::fs::metadata(path).ok();
            let dir = meta.as_ref().is_some_and(|m| m.is_dir());
            let changed = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            json!({
                "id": entry.id,
                "kind": entry.kind,
                "name": path.file_name().unwrap_or(path.as_os_str()).to_string_lossy(),
                "path": entry.value,
                "dir": dir,
                // Moved, deleted, or on a drive that is not there now: it keeps its place.
                "missing": meta.is_none(),
                "size": if dir { 0 } else { meta.map_or(0, |m| m.len()) },
                "mtime": changed,
            })
        }
        Kind::Text => {
            let line: String = entry.value.lines().next().unwrap_or_default().trim().chars().take(80).collect();
            json!({ "id": entry.id, "kind": entry.kind, "name": line, "text": entry.value })
        }
        Kind::Link => {
            let bare = entry.value.split_once("://").map_or(entry.value.as_str(), |(_, rest)| rest);
            json!({ "id": entry.id, "kind": entry.kind, "name": bare.trim_end_matches('/'), "text": entry.value })
        }
    }
}

/// The entries a command names with `ids` (or one `id`), in shelf order.
fn picked(cx: &Ctx, args: &Value) -> Vec<Entry> {
    let ids: Vec<&str> = match (&args["ids"], &args["id"]) {
        (Value::Array(ids), _) => ids.iter().filter_map(Value::as_str).collect(),
        (_, Value::String(id)) => vec![id],
        _ => Vec::new(),
    };
    load(cx).into_iter().filter(|e| ids.contains(&e.id.as_str())).collect()
}

fn one(cx: &Ctx, args: &Value) -> Result<Entry, String> {
    picked(cx, args).into_iter().next().ok_or_else(|| "That item is not on the shelf any more.".to_string())
}

fn file_of(entry: &Entry) -> Result<&Path, String> {
    if entry.kind == Kind::File { Ok(Path::new(&entry.value)) } else { Err("That is not a file.".into()) }
}

/// The entries as svc-shell takes them, for a drag, a share or the clipboard.
/// A file that is not there is left out: there is nothing of it to hand over.
fn outgoing(entries: &[Entry]) -> Vec<Item> {
    entries
        .iter()
        .filter(|e| e.kind != Kind::File || Path::new(&e.value).exists())
        .map(|e| match e.kind {
            Kind::File => Item::File(PathBuf::from(&e.value)),
            Kind::Text => Item::Text(e.value.clone()),
            Kind::Link => Item::Link(e.value.clone()),
        })
        .collect()
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ABC[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

fn png_url(png: &[u8]) -> String {
    format!("data:image/png;base64,{}", base64(png))
}

/// "name.zip" in `dir`, or "name (2).zip"… when that is taken.
fn free_zip(dir: &Path, name: &str) -> PathBuf {
    (1u32..)
        .map(|n| dir.join(if n == 1 { format!("{name}.zip") } else { format!("{name} ({n}).zip") }))
        .find(|path| !path.exists())
        .expect("a free name")
}

/// The file was made or changed within the last minute: whoever wrote it may still hold it.
fn just_landed(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    [meta.created(), meta.modified()]
        .into_iter()
        .flatten()
        .any(|at| at.elapsed().is_ok_and(|age| age < Duration::from_secs(60)))
}

/// The start of a file that reads as text, for the preview. None for anything else.
fn text_of(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut start = vec![0u8; 16 * 1024];
    let got = std::fs::File::open(path).ok()?.read(&mut start).ok()?;
    start.truncate(got);
    if start.contains(&0) {
        return None;
    }
    // A character cut in two by the 16 KB is not a reason to call it not text.
    let text = match std::str::from_utf8(&start) {
        Ok(text) => text,
        Err(err) if start.len() - err.valid_up_to() < 4 => std::str::from_utf8(&start[..err.valid_up_to()]).ok()?,
        Err(_) => return None,
    };
    (!text.trim().is_empty()).then(|| text.to_string())
}

/// Where the Share sheet hangs from, in screen pixels: the middle of the
/// island, a little under its lower edge.
fn sheet_point(cx: &Ctx) -> Result<(i32, i32), String> {
    // Through the raw handle: Tauri's HWND is another version of the `windows` crate's.
    let island = cx.app().get_webview_window("island").and_then(|w| w.hwnd().ok()).ok_or("The island is not there.")?;
    let island = HWND(island.0 as *mut _);
    let mut at = RECT::default();
    unsafe { GetWindowRect(island, &mut at) }.map_err(|e| e.message())?;
    let scale = |px: i32| px * unsafe { GetDpiForWindow(island) }.max(96) as i32 / 96;
    Ok(((at.left + at.right) / 2, at.top + scale(SECTION_H + 12)))
}

/// The entry in Explorer's menus follows the "explorerMenu" setting: there
/// while it is on (and written again if the app has moved), gone when it is off.
// debt: the entry stays while the feature is switched off with the setting on,
// and after an uninstall: the uninstaller should remove it (see the report).
fn explorer_menu(settings: &Value) {
    let (Some(places), Ok(exe)) = (Places::user(), std::env::current_exe()) else { return };
    let verb = format!("{}.{MENU_VERB}", notch_brand::SLUG);
    let label = i18n::t("Add to Shelf");
    let now = svc_shell::menu_command(&verb, &places);
    let done = if settings["explorerMenu"].as_bool().unwrap_or(false) {
        let wanted = format!("\"{}\" {MENU_ARG} \"%1\"", exe.display());
        if now.as_deref() == Some(wanted.as_str()) {
            return;
        }
        svc_shell::add_to_menus(&MenuEntry { id: &verb, label: &label, exe: &exe, arg: MENU_ARG }, &places)
    } else if now.is_some() {
        svc_shell::remove_from_menus(&verb, &places)
    } else {
        return;
    };
    if let Err(err) = done {
        log::line(format!("shelf: Explorer's menus: {err}"));
    }
}

// ── The feature ───────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct Shelf {
    watch: Mutex<Option<watch::Watch>>,
    /// What the preview window shows.
    preview: Mutex<Option<String>>,
}

/// The file picker is up.
static PICKING: AtomicBool = AtomicBool::new(false);

impl Shelf {
    /// The drag watcher follows the "dragDetect" setting.
    fn watch_drags(&self, cx: &Ctx, settings: &Value) {
        let mut watch = self.watch.lock().unwrap_or_else(|e| e.into_inner());
        if settings["dragDetect"].as_bool().unwrap_or(true) {
            if watch.is_none() {
                *watch = watch::start(cx.clone());
            }
        } else {
            watch.take();
        }
    }
}

/// Files handed over by their paths (a drop, Explorer's menus) join the shelf.
///
/// A file that lies right in Windows' temp folder is somebody's working copy
/// and will not last: the shelf keeps a copy of its own, under the same name.
/// That is how an attachment dragged out of a mail arrives. The mail client
/// only offers it (`FileGroupDescriptorW`, `FileContents`); the island's
/// WebView2, which takes the drop, writes it to %TEMP%\<its name>, hands that
/// path over and deletes the file when the app closes (seen with the example
/// shelf-drop-probe). A picture dragged out of a web page comes the same way.
// ponytail: copied whole, whatever its size; a limit and a word to the user if a huge one ever comes this way.
// ponytail: only the temp folder itself, where WebView2 writes. A file in a folder below it is taken as a
// path like any other (an archive program's extract is gone when that program closes: it then shows as missing).
fn take_files(cx: &Ctx, paths: Vec<PathBuf>) -> usize {
    // Long names, one case: the two paths come from different programs.
    let plain = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().to_lowercase();
    let (own, temp) = (temp_dir(cx), plain(&std::env::temp_dir()));
    let files: Vec<_> = paths
        .into_iter()
        .map(|path| {
            if !path.is_file() || path.parent().is_none_or(|folder| plain(folder) != temp) {
                return filed(path, false);
            }
            let _ = std::fs::create_dir_all(&own);
            let to = svc_shell::free_name(&own, &path.file_name().unwrap_or_default().to_string_lossy());
            match std::fs::copy(&path, &to) {
                Ok(_) => filed(to, true),
                Err(err) => {
                    log::line(format!("shelf: a file from the temp folder could not be kept: {err}"));
                    filed(path, false)
                }
            }
        })
        .collect();
    let count = files.len();
    add(cx, files);
    count
}

impl Feature for Shelf {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn start(&self, cx: &Ctx) -> Result<(), String> {
        // What the shelf made whose entry is gone (removed while the app was closing) goes too.
        let kept = load(cx);
        for file in std::fs::read_dir(temp_dir(cx)).into_iter().flatten().flatten() {
            if !kept.iter().any(|e| Path::new(&e.value) == file.path()) {
                let _ = std::fs::remove_file(file.path());
            }
        }
        let settings = cx.settings(ID);
        self.watch_drags(cx, &settings);
        explorer_menu(&settings);
        Ok(())
    }

    fn stop(&self, _cx: &Ctx) {
        self.watch.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.preview.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    fn settings_changed(&self, cx: &Ctx, settings: &Value) {
        self.watch_drags(cx, settings);
        explorer_menu(settings);
    }

    fn call(&self, cx: &Ctx, method: &str, args: Value) -> Result<Value, String> {
        match method {
            // Looking at the files happens here, off the page's thread (a disk
            // that sleeps, a share that is slow). A file that is not there keeps
            // its entry and its id: it may be back (a drive plugged in again), and
            // the page marks it. Only what the shelf made itself goes with its file.
            "list" => {
                let items = edit(cx, |items| {
                    items.retain(|e| !(e.temp && e.kind == Kind::File && !Path::new(&e.value).exists()));
                    items.clone()
                });
                Ok(Value::Array(items.iter().map(view).collect()))
            }
            // Files dropped on the shelf, by their paths: the app calls this itself
            // (src-tauri/src/webview_drop.rs) when the page tags its drop with this feature.
            "dropped" => {
                let paths = args["paths"].as_array().map(|a| real_files(a.iter().filter_map(Value::as_str))).unwrap_or_default();
                take_files(cx, paths);
                Ok(Value::Null)
            }
            // Files named from outside the island ("Add to Shelf" in Explorer's
            // menus): the same, and the page is told so that it shows them landing.
            "added" => {
                let paths = args["paths"].as_array().map(|a| real_files(a.iter().filter_map(Value::as_str))).unwrap_or_default();
                let count = take_files(cx, paths);
                cx.emit(ID, "picked", count);
                Ok(json!(count))
            }
            // A dropped text; an address becomes a link.
            "add" => {
                let Some(text) = worded(args["text"].as_str().unwrap_or_default()) else {
                    return Err("There was no text to keep.".into());
                };
                add(cx, vec![text]);
                Ok(Value::Null)
            }
            // Ctrl+V: what the clipboard holds joins the shelf. Files as their
            // paths; a mail's attachment or a picture that is nobody's file gets
            // one in the shelf's own folder. Answers how many things came.
            "paste" => {
                let own = temp_dir(cx);
                let new: Vec<_> = svc_shell::clipboard(&own, &i18n::t("Pasted picture"))
                    .into_iter()
                    .filter_map(|item| match item {
                        Item::File(path) => {
                            let ours = path.starts_with(&own);
                            Some(filed(path, ours))
                        }
                        Item::Text(text) | Item::Link(text) => worded(&text),
                    })
                    .collect();
                let count = new.len();
                add(cx, new);
                Ok(json!(count))
            }
            // A click on the empty shelf: Windows' own file picker. It is up for as
            // long as the user takes; `picked` tells the page how many files came.
            "pick" => {
                if PICKING.swap(true, Ordering::SeqCst) {
                    return Ok(json!(false));
                }
                let cx = cx.clone();
                std::thread::spawn(move || {
                    let files = svc_shell::pick_files(&i18n::t("Add to the shelf"));
                    PICKING.store(false, Ordering::SeqCst);
                    let count = files.len();
                    add(&cx, files.into_iter().map(|path| filed(path, false)).collect());
                    cx.emit(ID, "picked", count);
                });
                Ok(json!(true))
            }
            "remove" => {
                let gone: Vec<String> = picked(cx, &args).into_iter().map(|e| e.id).collect();
                let own = temp_dir(cx);
                edit(cx, |items| {
                    items.retain(|e| {
                        let go = gone.contains(&e.id);
                        // Only what the shelf made itself is deleted; a user's file is only forgotten.
                        if go && e.temp && Path::new(&e.value).starts_with(&own) {
                            let _ = std::fs::remove_file(&e.value);
                        }
                        !go
                    });
                });
                Ok(Value::Null)
            }
            // A new name for the file itself, where it lives. The entry keeps its
            // id. A name that will not do is an answer, not an error: `why` is one
            // of svc_shell::NameProblem's words, and the page has the sentence.
            "rename" => {
                let entry = one(cx, &args)?;
                match svc_shell::rename(file_of(&entry)?, args["name"].as_str().unwrap_or_default()) {
                    Ok(to) => {
                        edit(cx, |items| {
                            if let Some(e) = items.iter_mut().find(|e| e.id == entry.id) {
                                e.value = to.to_string_lossy().into_owned();
                            }
                        });
                        Ok(json!({ "ok": true }))
                    }
                    Err(problem) => Ok(json!({ "ok": false, "why": problem.code() })),
                }
            }
            // The user's own files go to the Recycle Bin (never deleted for good:
            // svc_shell::recycle). The page has asked. Only what did go leaves the shelf.
            "recycle" => {
                let asked: Vec<Entry> = picked(cx, &args).into_iter().filter(|e| e.kind == Kind::File).collect();
                let paths: Vec<PathBuf> = asked.iter().map(|e| PathBuf::from(&e.value)).collect();
                let went: Vec<String> = svc_shell::recycle(&paths)?.into_iter().map(|r| r.from.to_string_lossy().to_lowercase()).collect();
                let gone: Vec<&str> = asked
                    .iter()
                    .filter(|e| went.contains(&svc_shell::shell_path(Path::new(&e.value)).to_string_lossy().to_lowercase()))
                    .map(|e| e.id.as_str())
                    .collect();
                edit(cx, |items| items.retain(|e| !gone.contains(&e.id.as_str())));
                Ok(json!({ "moved": gone.len(), "asked": asked.len() }))
            }
            // The file's own picture when it has one, its icon otherwise. `again`:
            // it got the icon but has only just landed, so its writer may still
            // hold it; the page asks once more in a moment.
            "thumb" => {
                let entry = one(cx, &args)?;
                let path = file_of(&entry)?;
                let (png, icon) = match svc_shell::thumbnail(path, 192) {
                    Ok(png) => (png, false),
                    Err(_) => (svc_shell::icon(&entry.value, 64)?, true),
                };
                Ok(json!({ "url": png_url(&png), "icon": icon, "again": icon && just_landed(path) }))
            }
            // Space: the preview window, on the item named; without one it closes.
            // The window asks `previewing` when it opens and hears `preview` after.
            "preview" => {
                let shown = match picked(cx, &args).into_iter().next() {
                    Some(entry) => {
                        *self.preview.lock().unwrap_or_else(|e| e.into_inner()) = Some(entry.id.clone());
                        cx.window().open(WindowSpec {
                            label: PREVIEW.into(),
                            feature: ID.into(),
                            page: "preview".into(),
                            w: 520.0,
                            h: 420.0,
                            frameless: true,
                            topmost: true,
                            // The keyboard stays where it is: the island goes on reading Space and Esc.
                            no_activate: true,
                            monitor: Monitor::Island,
                            ..Default::default()
                        })?;
                        view(&entry)
                    }
                    None => {
                        self.preview.lock().unwrap_or_else(|e| e.into_inner()).take();
                        cx.window().close(PREVIEW);
                        Value::Null
                    }
                };
                cx.emit(ID, "preview", shown.clone());
                Ok(shown)
            }
            "previewing" => {
                let id = self.preview.lock().unwrap_or_else(|e| e.into_inner()).clone();
                Ok(id.and_then(|id| load(cx).into_iter().find(|e| e.id == id)).map_or(Value::Null, |e| view(&e)))
            }
            // What the preview shows of a file: its picture, large; failing that
            // the start of its text; failing that its icon.
            "picture" => {
                let entry = one(cx, &args)?;
                let path = file_of(&entry)?;
                if let Ok(png) = svc_shell::thumbnail(path, 800) {
                    return Ok(json!({ "url": png_url(&png) }));
                }
                if let Some(text) = text_of(path) {
                    return Ok(json!({ "text": text }));
                }
                Ok(json!({ "url": png_url(&svc_shell::icon(&entry.value, 256)?), "icon": true }))
            }
            // A double click or Enter: files and links open, a text goes to the clipboard.
            "open" => {
                let entry = one(cx, &args)?;
                match entry.kind {
                    Kind::Text => svc_shell::copy_to_clipboard(&[Item::Text(entry.value)])?,
                    Kind::Link if !is_web(&entry.value) => return Err("That is not a web address.".into()),
                    Kind::File if !Path::new(&entry.value).exists() => return Err("That file is not there any more.".into()),
                    _ => svc_shell::open(&entry.value)?,
                }
                Ok(Value::Null)
            }
            "handlers" => {
                let names: Vec<String> = svc_shell::open_with_menu(file_of(&one(cx, &args)?)?).into_iter().map(|h| h.name).collect();
                Ok(json!(names))
            }
            // With one of the names `handlers` gave; without a name, Windows' own chooser.
            "open_with" => {
                let entry = one(cx, &args)?;
                let path = file_of(&entry)?;
                match args["name"].as_str().filter(|name| !name.is_empty()) {
                    Some(name) => svc_shell::open_with(path, name)?,
                    None => svc_shell::open_with_dialog(path)?,
                }
                Ok(Value::Null)
            }
            "reveal" => {
                svc_shell::reveal(file_of(&one(cx, &args)?)?)?;
                Ok(Value::Null)
            }
            // Files as Explorer's Copy puts them there, a text as text, a link as
            // an address and as text. Answers how many things went.
            "copy" => {
                let items = outgoing(&picked(cx, &args));
                if !items.is_empty() {
                    svc_shell::copy_to_clipboard(&items)?;
                }
                Ok(json!(items.len()))
            }
            // One file: name.ext.zip; one folder: Folder.zip; several: Archive.zip. The zip joins the shelf.
            "zip" => {
                let files: Vec<PathBuf> = outgoing(&picked(cx, &args))
                    .into_iter()
                    .filter_map(|item| match item {
                        Item::File(path) => Some(path),
                        _ => None,
                    })
                    .collect();
                let name = match files.as_slice() {
                    [] => return Err("Only files can be zipped.".into()),
                    [one] => one.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                    _ => "Archive".to_string(),
                };
                let dir = temp_dir(cx);
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let out = free_zip(&dir, &name);
                let cx = cx.clone();
                std::thread::spawn(move || match svc_shell::zip(&files, &out) {
                    Ok(()) => add(&cx, vec![filed(out, true)]),
                    Err(err) => {
                        log::line(format!("shelf: zip failed: {err}"));
                        cx.emit(ID, "failed", "zip");
                    }
                });
                // Started: the page hears `changed` or `failed` when it ends.
                Ok(json!(true))
            }
            // The Windows Share sheet. It is given a window of its own under the
            // island (svc_shell::share_sheet says why: for the island, or for the
            // settings window while it is hidden, Windows shows nothing). True when it is up.
            "share" => {
                let items = outgoing(&picked(cx, &args));
                if items.is_empty() {
                    return Err("There is nothing to share.".into());
                }
                // Looking the files up waits on Windows: here, not on the UI thread.
                let package = svc_shell::share_package(&items).map_err(|e| e.message())?;
                let at = sheet_point(cx)?;
                let (done, shown) = mpsc::channel();
                cx.app()
                    .run_on_main_thread(move || {
                        let _ = done.send(svc_shell::share_sheet(&package, at));
                    })
                    .map_err(|e| e.to_string())?;
                shown.recv().map_err(|e| e.to_string())??;
                Ok(json!(true))
            }
            // An OLE drag out of the island, on the UI thread, until the button is released.
            "drag" => {
                let items = outgoing(&picked(cx, &args));
                if items.is_empty() {
                    return Ok(json!("none"));
                }
                // The picture that follows the pointer is read from the file here, not on the UI thread.
                let picture = svc_shell::drag_picture(&items);
                let (done, effect) = mpsc::channel();
                cx.app()
                    .run_on_main_thread(move || {
                        let _ = done.send(svc_shell::drag_out_with(&items, picture));
                    })
                    .map_err(|e| e.to_string())?;
                Ok(json!(match effect.recv().unwrap_or(DropEffect::None) {
                    DropEffect::None => "none",
                    DropEffect::Copy => "copy",
                    DropEffect::Move => "move",
                    DropEffect::Link => "link",
                }))
            }
            _ => Err("unknown method".into()),
        }
    }

    fn strings(&self) -> &'static str {
        include_str!("../../../src/features/shelf/strings.json")
    }
}

pub fn feature() -> std::sync::Arc<dyn Feature> {
    std::sync::Arc::new(Shelf::default())
}

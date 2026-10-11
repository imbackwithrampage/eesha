// What the cleaner looks for: a fixed table of places, each a `svc_fs::Rule`
// written here and nowhere else. Nothing is searched for by guessing, and
// nothing in this file removes anything: `scan` reads names, dates and sizes.
//
// What starts ticked follows the upstream file
// Sources/Vorssaint/Services/Cleaner/CleanerPolicy.swift (ticked only when the
// evidence is strong and rebuilding is cheap); the age rule of the downloads
// follows Sources/Vorssaint/Services/ManagedDownloads/WhatsAppDownloadSupport.swift
// (`isOldEnough`, `sanitizedRetentionDays`, the extension sets).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use svc_fs::{Base, Found, Rule};

const fn local(root: &'static str, pattern: &'static str) -> Rule {
    Rule { base: Base::Local, root, pattern }
}
const fn home(root: &'static str, pattern: &'static str) -> Rule {
    Rule { base: Base::Profile, root, pattern }
}

/// One line of the review list: where to look, and how the find is offered.
pub struct Target {
    /// "caches" | "dev" | "downloads" | "screenshots" | "backups": the group it is listed in.
    pub kind: &'static str,
    /// English; the page shows it through its strings.
    pub title: &'static str,
    pub rules: &'static [Rule],
    /// Ticked when found.
    pub checked: bool,
    /// Left alone when younger than this many hours (0: any age).
    pub hours: u64,
    /// One row a find (the user's own files) instead of one row for all of them.
    pub each: bool,
    /// The program that keeps these open: while it runs, the row waits unticked.
    pub holder: &'static str,
}

const fn cache(title: &'static str, rules: &'static [Rule], holder: &'static str) -> Target {
    Target { kind: "caches", title, rules, checked: true, hours: 0, each: false, holder }
}
const fn dev(title: &'static str, rules: &'static [Rule], checked: bool, holder: &'static str) -> Target {
    Target { kind: "dev", title, rules, checked, hours: 0, each: false, holder }
}

/// The caches a Chromium browser rebuilds, in each of its profiles. Never its history, logins or cookies.
macro_rules! chromium {
    ($root:literal) => {
        &[local($root, "*/Cache/Cache_Data"), local($root, "*/Code Cache"), local($root, "*/GPUCache")]
    };
}

pub static TARGETS: &[Target] = &[
    // A day old at least: what a running program wrote this morning is its own.
    Target { kind: "caches", title: "Temporary files", rules: &[local("Temp", "*")], checked: true, hours: 24, each: false, holder: "" },
    cache("Crash dumps", &[local("CrashDumps", "*")], ""),
    cache("Windows error reports", &[local("Microsoft/Windows/WER/ReportArchive", "*"), local("Microsoft/Windows/WER/ReportQueue", "*")], ""),
    cache("Graphics shader caches", &[local("D3DSCache", "*"), local("NVIDIA/DXCache", "*"), local("NVIDIA/GLCache", "*"), local("AMD/DxCache", "*")], ""),
    cache("Chrome cache", chromium!("Google/Chrome/User Data"), "chrome.exe"),
    cache("Edge cache", chromium!("Microsoft/Edge/User Data"), "msedge.exe"),
    cache("Brave cache", chromium!("BraveSoftware/Brave-Browser/User Data"), "brave.exe"),
    cache("Firefox cache", &[local("Mozilla/Firefox/Profiles", "*/cache2")], "firefox.exe"),
    cache(
        "Package manager caches",
        &[local("npm-cache", "_cacache"), local("Yarn/Cache", "*"), local("pip/Cache", "*"), local("NuGet/v3-cache", "*"), local("go-build", "*")],
        "",
    ),
    dev("Visual Studio caches", &[local("Microsoft/VisualStudio", "*/ComponentModelCache"), local("Microsoft/VisualStudio", "*/Designer/ShadowCache")], true, "devenv.exe"),
    dev("JetBrains caches and logs", &[local("JetBrains", "*/caches"), local("JetBrains", "*/log")], true, ""),
    dev("Android SDK cache", &[home(".android", "cache")], true, ""),
    // Large, and downloaded again in full: listed for the willing.
    dev("Cargo registry cache", &[home(".cargo/registry", "cache")], false, "cargo.exe"),
    dev("Gradle caches", &[home(".gradle", "caches")], false, ""),
    dev("NuGet packages", &[home(".nuget", "packages")], false, ""),
    // Only files of the kinds and the age the user set, and only when the switch is on.
    Target { kind: "downloads", title: "Telegram downloads", rules: &[Rule { base: Base::Downloads, root: "Telegram Desktop", pattern: "*" }], checked: true, hours: 0, each: true, holder: "" },
    // The user's own pictures: offered, never ticked.
    Target {
        kind: "screenshots",
        title: "Old screenshots",
        rules: &[Rule { base: Base::Screenshots, root: "", pattern: "Screenshot*.png" }, Rule { base: Base::Screenshots, root: "", pattern: "Screenshot*.jpg" }],
        checked: false,
        hours: 0,
        each: true,
        holder: "",
    },
    // A safety net, never a machine's call: offered, never ticked.
    Target {
        kind: "backups",
        title: "Device backups",
        rules: &[Rule { base: Base::Roaming, root: "Apple Computer/MobileSync/Backup", pattern: "*" }, home("Apple/MobileSync/Backup", "*")],
        checked: false,
        hours: 0,
        each: true,
        holder: "",
    },
];

/// What a scan is told by the settings and the moment.
pub struct Opts {
    pub now: SystemTime,
    /// 0: screenshots are not looked for.
    pub screenshot_days: u64,
    /// The age in days and the kinds of file, when old downloads are to be listed.
    pub downloads: Option<(u64, Vec<String>)>,
    /// The programs that run, by file name, lowercase.
    pub running: Vec<String>,
}

/// One row of the list, with the files behind it: what a clean of the row moves, and nothing else.
pub struct Row {
    /// Given by the feature when it lists the row.
    pub id: u32,
    pub kind: &'static str,
    pub title: &'static str,
    /// A row of one file or folder is called by its name.
    pub name: Option<String>,
    /// The folder to show for it.
    pub place: PathBuf,
    pub checked: bool,
    /// "running" (its program holds the files), "left" (a clean could not move all of it), or nothing.
    pub note: &'static str,
    pub modified: Option<u64>,
    pub items: Vec<(Found, u64)>,
}

impl Row {
    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|(_, bytes)| bytes).sum()
    }
}

/// 1, 2, 7, 14 or 30 days; anything else is a week.
pub fn retention(days: u64) -> u64 {
    if [1, 2, 7, 14, 30].contains(&days) { days } else { 7 }
}

/// Old only when it both arrived and was last changed before the cut: a file
/// edited yesterday is not old, whenever it was downloaded.
pub fn old(created: Option<SystemTime>, modified: Option<SystemTime>, now: SystemTime, hours: u64) -> bool {
    if hours == 0 {
        return true;
    }
    let Some(cut) = now.checked_sub(Duration::from_secs(hours * 3600)) else { return false };
    [created, modified].iter().all(|at| at.is_some_and(|at| at <= cut))
}

/// The kind of a downloaded file by its extension; "" for one still being downloaded.
pub fn category(name: &str) -> &'static str {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "download" | "part" | "partial" | "crdownload" | "tmp" => "",
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "bmp" | "tif" | "tiff" => "image",
        "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "3gp" => "video",
        "mp3" | "m4a" | "ogg" | "oga" | "opus" | "wav" | "flac" | "aac" => "audio",
        "7z" | "bz" | "bz2" | "cab" | "gz" | "iso" | "rar" | "tar" | "tgz" | "xz" | "zip" => "archive",
        "csv" | "doc" | "docm" | "docx" | "epub" | "md" | "odp" | "ods" | "odt" | "pdf" | "ppt" | "pptm" | "pptx" | "rtf" | "tex" | "txt" | "xls" | "xlsm" | "xlsx" => "document",
        _ => "other",
    }
}

/// A name Windows itself gives a screenshot: "Screenshot (12).png" (Win+PrtScn)
/// or "Screenshot 2026-10-11 093015.png" (Snipping Tool). A file the user named is a decision and is not one.
pub fn is_screenshot_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let Some(rest) = lower.strip_suffix(".png").or_else(|| lower.strip_suffix(".jpg")).and_then(|stem| stem.strip_prefix("screenshot ")) else { return false };
    let shape: String = rest.chars().map(|c| if c.is_ascii_digit() { '0' } else { c }).collect();
    shape == "0000-00-00 000000" || (shape.len() > 2 && shape.starts_with('(') && shape.ends_with(')') && shape[1..shape.len() - 1].chars().all(|c| c == '0'))
}

/// What the target has to offer now. Reads only; a find of no size is not listed.
pub fn scan(target: &Target, opts: &Opts, cancel: &AtomicBool) -> Vec<Row> {
    let hours = match target.kind {
        "screenshots" if opts.screenshot_days == 0 => return Vec::new(),
        "screenshots" => opts.screenshot_days * 24,
        "downloads" => match &opts.downloads {
            Some((days, _)) => retention(*days) * 24,
            None => return Vec::new(),
        },
        _ => target.hours,
    };
    let keep = |path: &Path| {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match target.kind {
            "screenshots" => is_screenshot_name(&name),
            "downloads" => opts.downloads.as_ref().is_some_and(|(_, kinds)| kinds.iter().any(|k| k == category(&name))),
            _ => true,
        }
    };
    let held = !target.holder.is_empty() && opts.running.iter().any(|exe| exe == target.holder);
    let row = |name, place: &Path, modified, items| Row {
        id: 0,
        kind: target.kind,
        title: target.title,
        name,
        place: place.to_path_buf(),
        checked: target.checked && !held,
        note: if held { "running" } else { "" },
        modified,
        items,
    };
    let secs = |at: Option<SystemTime>| at.and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());

    let (mut rows, mut all, mut place) = (Vec::new(), Vec::new(), None);
    for rule in target.rules {
        for item in svc_fs::find(rule, keep) {
            if cancel.load(SeqCst) {
                return Vec::new();
            }
            // The user's own files are files: a folder among them is not this rule's.
            if !old(item.created(), item.modified(), opts.now, hours) || (target.each && target.kind != "backups" && item.is_dir()) {
                continue;
            }
            // A folder nobody touched for a month may hold a file of this morning: then it is not old.
            let (bytes, newest) = svc_fs::measure(&item, cancel);
            if bytes == 0 || !old(newest, newest, opts.now, hours) {
                continue;
            }
            if target.each {
                let place = item.root().to_path_buf();
                rows.push(row(Some(item.name()), &place, secs(item.modified()), vec![(item, bytes)]));
            } else {
                place.get_or_insert_with(|| item.root().to_path_buf());
                all.push((item, bytes));
            }
        }
    }
    if let Some(place) = place {
        rows.push(row(None, &place, None, all));
    }
    rows
}

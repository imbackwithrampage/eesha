// Proves the parts of the cleaner that touch Windows, without the app:
//
//   cargo run -p notch-feat-cleaner-uninstaller --example cleaner-probe
//
// Everything it writes, moves or sends to the Recycle Bin is a made-up tree it
// builds under target/cleaner-probe (and target/cleaner-probe-outside, where
// the junction points). Real folders are only read: asked whether they would
// be refused, and measured. No uninstaller is run but the probe itself, and
// no startup entry is changed.

use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};
use std::time::{Duration, Instant, SystemTime};

use notch_feat_cleaner_uninstaller::{clean, startup, uninstall};
use svc_apps::InstalledApp;
use svc_fs::{Base, Refusal, Rule};

/// The made-up tree, written from this crate's folder: text of the source, as every rule's root is.
macro_rules! tree {
    ($rel:literal) => {
        concat!(env!("CARGO_MANIFEST_DIR"), r"\..\..\target\cleaner-probe", $rel)
    };
}
const fn fixed(root: &'static str, pattern: &'static str) -> Rule {
    Rule { base: Base::Fixed, root, pattern }
}
const CACHE: Rule = fixed(tree!(r"\cache"), "*");
const THROUGH: Rule = fixed(tree!(r"\cache"), "escape/*");
/// A made-up Temp, cleaned as the real one is: what is a day old and holds nothing newer.
static TEMP: clean::Target = clean::Target { kind: "caches", title: "probe", rules: &[fixed(tree!(r"\temp"), "*")], checked: true, hours: 24, each: false, holder: "" };
static PLACES: &[Rule] = &[
    fixed(tree!(r"\appdata\Roaming"), "*"),
    fixed(tree!(r"\appdata\Roaming"), "*/*"),
    fixed(tree!(r"\appdata\Local"), "*"),
    fixed(tree!(r"\appdata\Local"), "*/*"),
    fixed(tree!(r"\appdata\Start"), "*"),
];

static FAILED: AtomicU32 = AtomicU32::new(0);

fn ok(label: &str, pass: bool) {
    if !pass {
        FAILED.fetch_add(1, SeqCst);
    }
    println!("  {} {label}", if pass { "ok  " } else { "FAIL" });
}

fn refused(label: &str, root: &Path, path: &Path, want: Refusal) {
    let got = svc_fs::refusal(root, path);
    ok(&format!("{label}: {} -> {}", path.display(), got.map_or("allowed", Refusal::code)), got == Some(want));
}

fn write(path: &Path, bytes: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, vec![b'x'; bytes]).unwrap();
}

/// Makes a made-up file or folder look `days` old.
fn age(path: &Path, days: u64) {
    use std::os::windows::fs::FileTimesExt;
    let at = SystemTime::now() - Duration::from_secs(days * 86_400);
    // 0x0200_0000: a folder opens only with the backup flag.
    let file = std::fs::OpenOptions::new().write(true).custom_flags(0x0200_0000).open(path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(at).set_accessed(at).set_created(at)).unwrap();
}

fn junction(link: &Path, to: &Path) {
    let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(link).arg(to).output().unwrap();
    assert!(made.status.success(), "mklink: {}", String::from_utf8_lossy(&made.stderr));
}

fn is_link(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|m| m.file_attributes() & 0x400 != 0)
}

fn app(name: &str, publisher: &str, key: &str) -> InstalledApp {
    InstalledApp { key: key.into(), name: name.into(), version: "1.2.3".into(), publisher: publisher.into(), location: String::new(), uninstall: String::new(), per_user: true, store: false }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--fake-uninstaller") {
        println!("fake uninstaller: {}", args[2..].join("|"));
        return;
    }
    let never = AtomicBool::new(false);
    let tree = std::path::absolute(tree!("")).unwrap();
    let outside = tree.with_file_name("cleaner-probe-outside");
    assert!(tree.ends_with(r"target\cleaner-probe") && tree.parent().unwrap().join("CACHEDIR.TAG").is_file(), "not the build folder: {}", tree.display());
    let cache = tree.join("cache");
    let home = Base::Profile.path().unwrap();
    let local = Base::Local.path().unwrap();

    // ── The made-up tree ──────────────────────────────────────────────────────
    println!("tree: {}", tree.display());
    write(&cache.join("a.tmp"), 1000);
    write(&cache.join("b.log"), 2000);
    write(&cache.join("dir1").join("inner.bin"), 3000);
    write(&cache.join("swap.txt"), 400);
    write(&cache.join("held.txt"), 500);
    write(&outside.join("keep.txt"), 77);
    if !is_link(&cache.join("escape")) {
        junction(&cache.join("escape"), &outside);
    }
    let dir2 = cache.join("dir2");
    if is_link(&dir2) {
        std::fs::remove_dir(&dir2).unwrap(); // the link of the last run, not what it points to
    }
    write(&dir2.join("inner.bin"), 600);
    for folder in [r"Roaming\Zzyzx Probe Tool", r"Roaming\Zzyzx Labs\Zzyzx Probe Tool", r"Roaming\Other Thing", r"Local\zzyzxprobetool-updater", r"Local\Zzyzx Probe Tool Pro"] {
        write(&tree.join("appdata").join(folder).join("settings.json"), 1234);
    }
    write(&tree.join(r"appdata\Start\Zzyzx Probe Tool.lnk"), 300);
    write(&tree.join(r"appdata\Start\zzyzx probe tool.txt"), 300);

    // ── Refusals: asked of real places, which are only read ───────────────────
    println!("\nrefusals");
    use Refusal::*;
    let users = home.parent().unwrap();
    refused("the profile", users, &home, Protected);
    let top: Vec<PathBuf> = std::fs::read_dir(&home).unwrap().flatten().map(|e| e.path()).collect();
    let open = top.iter().filter(|p| svc_fs::refusal(&home, p).is_none()).count();
    ok(&format!("the {} entries at the top of the profile: {} allowed", top.len(), open), open == 0 && top.len() > 5);
    for folder in ["Documents", "Desktop", "Downloads", "AppData"].map(|f| home.join(f)).iter().filter(|p| p.exists()) {
        refused("its own folder", &home, folder, Protected);
    }
    for folder in ["", "Temp", "Programs", "Microsoft", "Packages", r"Microsoft\Windows", r"Microsoft\WindowsApps"].map(|f| local.join(f)).iter().filter(|p| p.exists()) {
        refused("AppData's own", &home, folder, Protected);
    }
    for own in [notch_core::win::local_dir(), notch_core::win::config_dir()].iter().filter(|p| p.exists()) {
        refused("the app's own data", &home, own, Protected);
    }
    for letter in 'A'..='Z' {
        let drive = PathBuf::from(format!("{letter}:\\"));
        if drive.exists() {
            let got = svc_fs::refusal(&drive, &drive);
            ok(&format!("drive {} -> {}", drive.display(), got.map_or("allowed", Refusal::code)), got.is_some());
            let deep = svc_fs::refusal(&drive, &drive.join(r"no\such\made-up\file.tmp"));
            println!("       a path on it -> {} (missing on a fixed disk, drive on any other)", deep.map_or("allowed", Refusal::code));
        }
    }
    let c = Path::new(r"C:\");
    for path in [r"C:\Windows", r"C:\Windows\Temp", r"C:\Windows\System32\drivers\etc\hosts", r"C:\Program Files", r"C:\Program Files\Common Files", r"C:\Program Files (x86)", r"C:\ProgramData", r"C:\Users"] {
        refused("system", c, Path::new(path), Protected);
    }
    refused("network", Path::new(r"\\server\share\folder"), Path::new(r"\\server\share\folder\a\b.tmp"), Drive);
    refused("network", Path::new(r"\\?\UNC\server\share\folder"), Path::new(r"\\?\UNC\server\share\folder\a\b.tmp"), Drive);
    refused("outside the root", &cache, &tree.join(r"appdata\Roaming\Other Thing"), Outside);
    refused("climbing out", &cache, &cache.join(r"..\appdata\Roaming\Other Thing"), Outside);
    refused("the root itself", &cache, &cache, Outside);
    refused("a junction", &cache, &cache.join("escape"), Link);
    refused("through a junction", &cache, &cache.join(r"escape\keep.txt"), Outside);
    refused("a root that is a junction", &cache.join("escape"), &cache.join(r"escape\keep.txt"), Link);
    ok("a made-up file in the tree is allowed", svc_fs::refusal(&cache, &cache.join("a.tmp")).is_none());

    println!("\nrules that find nothing");
    let none = |label: &str, rule: Rule| {
        let found = svc_fs::find(&rule, |_| true).len();
        ok(&format!("{label}: {found} found"), found == 0);
    };
    none("the top of the profile", Rule { base: Base::Profile, root: "", pattern: "*" });
    none("the top of AppData\\Local, Microsoft", Rule { base: Base::Local, root: "", pattern: "Microsoft" });
    none("a drive", fixed(r"C:\", "*"));
    none("Windows", fixed(r"C:\Windows", "*"));
    none("Program Files", fixed(r"C:\Program Files", "*/*"));
    none("a rule that climbs", Rule { base: Base::Local, root: "..", pattern: "*" });
    none("a pattern that climbs", Rule { base: Base::Local, root: "Temp", pattern: "../*" });
    none("into a junction", THROUGH);

    // ── To the Recycle Bin, from the made-up tree only ────────────────────────
    println!("\nrecycle");
    let bin_before = svc_fs::recycle_bin();
    let items = svc_fs::find(&CACHE, |_| true);
    let mut names: Vec<String> = items.iter().map(|i| i.name()).collect();
    names.sort();
    ok(&format!("found {names:?}"), names == ["a.tmp", "b.log", "dir1", "dir2", "held.txt", "swap.txt"]);
    let sizes: u64 = items.iter().map(|i| svc_fs::size(i, &never)).sum();
    ok(&format!("sizes add up to {sizes} bytes"), sizes == 1000 + 2000 + 3000 + 600 + 500 + 400);

    // After the scan: one file is replaced by another of the same name, one is held open, one folder becomes a junction.
    std::fs::rename(cache.join("swap.txt"), tree.join("swapped-away.txt")).unwrap();
    write(&cache.join("swap.txt"), 400);
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(cache.join("held.txt")).unwrap();
    std::fs::rename(dir2.join("inner.bin"), tree.join("dir2-inner.bin")).unwrap();
    std::fs::remove_dir(&dir2).unwrap();
    junction(&dir2, &outside);

    for item in &items {
        let got = svc_fs::recycle(item);
        let there = item.path().symlink_metadata().is_ok();
        let said = match &got {
            Ok(bin) => format!("in the bin as {}", bin.display()),
            Err(why) => format!("left: {}", why.code()),
        };
        let pass = match item.name().as_str() {
            "a.tmp" | "b.log" | "dir1" => got.as_ref().is_ok_and(|bin| bin.exists()) && !there,
            "swap.txt" => got == Err(Changed) && there,
            "held.txt" => got == Err(Busy) && there,
            "dir2" => got == Err(Link) && there,
            _ => false,
        };
        ok(&format!("{} -> {said}", item.name()), pass);
    }
    drop(held);
    let bin_after = svc_fs::recycle_bin();
    println!("       the Recycle Bin held {bin_before:?} (bytes, items), now {bin_after:?}");
    ok("the bin grew by the three that went", bin_after.zip(bin_before).is_some_and(|(a, b)| a.1 >= b.1 + 3 && a.0 >= b.0 + 6000));
    ok("the junctions are still junctions", is_link(&cache.join("escape")) && is_link(&dir2));
    ok("what they point to is intact", std::fs::read(outside.join("keep.txt")).is_ok_and(|b| b == vec![b'x'; 77]));
    std::fs::remove_dir(&dir2).unwrap();

    // ── What an uninstalled program left: a made-up program, made-up places ───
    println!("\nleftovers");
    let gone = app("Zzyzx Probe Tool 1.2.3 (x64)", "Zzyzx Labs", "arp:ZzyzxProbe");
    ok(&format!("its name without the version is {:?}", uninstall::without_version(&gone.name)), uninstall::without_version(&gone.name) == "Zzyzx Probe Tool");
    let left = uninstall::leftovers(&gone, &[], PLACES);
    let mut found: Vec<String> = left.iter().map(|f| f.path().strip_prefix(tree.join("appdata")).unwrap().display().to_string()).collect();
    found.sort();
    for path in &found {
        println!("       {path}");
    }
    ok("exactly its four", found == [r"Local\zzyzxprobetool-updater", r"Roaming\Zzyzx Labs\Zzyzx Probe Tool", r"Roaming\Zzyzx Probe Tool", r"Start\Zzyzx Probe Tool.lnk"]);
    let pro = app("Zzyzx Probe Tool Pro", "Zzyzx Labs", "arp:ZzyzxPro");
    ok("none while a program with a longer name of the same beginning stays", uninstall::leftovers(&gone, &[pro], PLACES).is_empty());
    let vendor = app("Something Else", "Zzyzx Probe Tool", "arp:Else");
    ok("none while a program of a publisher by that name stays", uninstall::leftovers(&gone, &[vendor], PLACES).is_empty());
    ok("none for a name too common to own a folder", uninstall::leftovers(&app("Microsoft", "", "arp:M"), &[], uninstall::PLACES).is_empty());
    let real = uninstall::leftovers(&gone, &svc_apps::installed(), uninstall::PLACES);
    ok(&format!("in the real places (read only): {} found", real.len()), real.is_empty());
    let one = left.iter().find(|f| f.path().ends_with(r"Roaming\Zzyzx Probe Tool")).unwrap();
    ok("a ticked leftover goes to the bin", svc_fs::recycle(one).is_ok() && !one.path().exists());

    // ── The uninstaller's command ─────────────────────────────────────────────
    println!("\nuninstall commands");
    let me = std::env::current_exe().unwrap();
    let cut = |line: &str| uninstall::split(line).map(|(exe, args)| (exe.to_string_lossy().into_owned(), args));
    ok("quoted", cut(r#""C:\Program Files\App\unins000.exe" /SILENT "two words""#) == Some((r"C:\Program Files\App\unins000.exe".into(), vec!["/SILENT".into(), "two words".into()])));
    ok("unquoted with spaces", cut(&format!("{} --remove", me.display())) == Some((me.to_string_lossy().into_owned(), vec!["--remove".into()])));
    ok("msiexec", cut("MsiExec.exe /I{0A1B2C3D-0000-0000-0000-000000000000}") == Some(("MsiExec.exe".into(), vec!["/I{0A1B2C3D-0000-0000-0000-000000000000}".into()])));
    let with = |line: &str| InstalledApp { uninstall: line.into(), ..app("X", "", "arp:{0A1B2C3D-0000-0000-0000-000000000000}") };
    let msi = uninstall::own_command(&with("MsiExec.exe /I{0A1B2C3D-0000-0000-0000-000000000000}"));
    ok(&format!("msiexec runs as {:?}", msi.as_ref().map(|c| c.shown())), msi.is_ok_and(|c| c.exe.ends_with(r"System32\msiexec.exe") && c.args == ["/X{0A1B2C3D-0000-0000-0000-000000000000}"]));
    ok("a shell is never run", uninstall::own_command(&with(r#"cmd /c "del /q C:\x""#)) == Err(uninstall::Why::Shell));
    ok("nor rundll32", uninstall::own_command(&with(r"C:\Windows\System32\rundll32.exe some.dll,Entry")) == Err(uninstall::Why::Shell));
    ok("an uninstaller that is gone", uninstall::own_command(&with(r#""C:\no\such\unins000.exe""#)) == Err(uninstall::Why::Missing));
    ok("nothing registered", uninstall::own_command(&with("")) == Err(uninstall::Why::Unreadable));
    let winget = svc_apps::winget::exe();
    let through = uninstall::winget_command(&with("x"), Some(Path::new("winget.exe"))).map(|c| c.shown());
    ok(&format!("winget: {through:?}"), through.as_deref() == Some("winget.exe uninstall --product-code {0A1B2C3D-0000-0000-0000-000000000000} --exact --accept-source-agreements --disable-interactivity"));
    ok("a key that reads as an option never reaches winget", uninstall::winget_command(&app("X", "", "arp:--all"), Some(Path::new("winget.exe"))).is_none());

    let installed = svc_apps::installed();
    let (mut own, mut by_winget, mut neither) = (0, 0, 0);
    let mut why = std::collections::BTreeMap::new();
    for a in &installed {
        let (mine, theirs) = (uninstall::own_command(a), uninstall::winget_command(a, winget.as_deref()));
        match (&mine, &theirs) {
            (Ok(_), _) => own += 1,
            (Err(_), Some(_)) => by_winget += 1,
            _ => neither += 1,
        }
        if let Err(w) = mine {
            *why.entry(w.code()).or_insert(0) += 1;
        }
    }
    println!("       {} installed programs (read only): {own} by their own uninstaller, {by_winget} through winget only, {neither} by neither; no own command: {why:?}", installed.len());
    ok("most programs can be removed from here", installed.len() > 5 && neither * 10 <= installed.len());
    for a in installed.iter().filter(|a| !a.store).take(3) {
        println!("       {} -> {}", a.name, uninstall::own_command(a).map_or_else(|w| format!("({})", w.code()), |c| c.shown()));
    }

    // The probe is its own uninstaller: read as a registered line, started without a shell, waited for.
    let line = format!("\"{}\" --fake-uninstaller /S \"two words\"", me.display());
    let cmd = uninstall::own_command(&with(&line)).unwrap();
    let argv: Vec<&str> = cmd.args.iter().map(String::as_str).collect();
    let out = svc_apps::run(&cmd.exe, &argv, Duration::from_secs(20), &never, |_| {}).unwrap();
    ok(&format!("ran {:?}: code {:?}, said {:?}", cmd.shown(), out.code, out.text.trim()), out.code == Some(0) && out.text.contains("fake uninstaller: /S|two words"));

    // ── The real cleaning targets: sizes only ─────────────────────────────────
    println!("\nscan of the real targets (sizes only)");
    let opts = clean::Opts {
        now: SystemTime::now(),
        screenshot_days: 30,
        downloads: Some((7, ["image", "video", "audio", "document", "archive", "other"].map(String::from).to_vec())),
        running: notch_core::win::process_table().into_values().map(|p| p.exe.to_lowercase()).collect(),
    };
    let started = Instant::now();
    let (mut total, mut ticked) = (0, 0);
    for target in clean::TARGETS {
        let rows = clean::scan(target, &opts, &never);
        let (bytes, files): (u64, usize) = (rows.iter().map(clean::Row::bytes).sum(), rows.iter().map(|r| r.items.len()).sum());
        let state = match rows.first() {
            None => "nothing there",
            Some(r) if r.note == "running" => "unticked: its program runs",
            Some(r) if r.checked => "ticked",
            Some(_) => "unticked",
        };
        println!("  {:<28} {:>9.1} MB  {:>5} items in {:>3} rows  {state}", target.title, bytes as f64 / 1_048_576.0, files, rows.len());
        total += bytes;
        ticked += rows.iter().filter(|r| r.checked).map(clean::Row::bytes).sum::<u64>();
    }
    println!("  {:.1} MB found, {:.1} MB ticked, in {:.1} s; the Recycle Bin holds {:?} (bytes, items)", total as f64 / 1_048_576.0, ticked as f64 / 1_048_576.0, started.elapsed().as_secs_f32(), svc_fs::recycle_bin());
    ok("the scan found something and removed nothing", total > 0);

    println!("\nnames and ages");
    ok("screenshot names", clean::is_screenshot_name("Screenshot (12).png") && clean::is_screenshot_name("Screenshot 2026-10-11 093015.png") && !clean::is_screenshot_name("Screenshot of my passport.png") && !clean::is_screenshot_name("Screenshot (12) copy.png") && !clean::is_screenshot_name("Screenshot ().png"));
    let (now, day) = (SystemTime::now(), Duration::from_secs(86_400));
    ok("old only when both dates are", clean::old(Some(now - 9 * day), Some(now - 8 * day), now, 7 * 24) && !clean::old(Some(now - 9 * day), Some(now - day), now, 7 * 24) && !clean::old(None, Some(now - 9 * day), now, 7 * 24));
    let temp = tree.join("temp");
    for file in [r"stale\old.bin", r"busy\fresh.bin", "young.bin", "old.bin"] {
        write(&temp.join(file), 100);
    }
    for stale in [r"stale\old.bin", "stale", "busy", "old.bin"] {
        age(&temp.join(stale), 3);
    }
    let rows = clean::scan(&TEMP, &opts, &never);
    let mut listed: Vec<String> = rows.iter().flat_map(|r| r.items.iter().map(|(item, _)| item.name())).collect();
    listed.sort();
    ok(&format!("of a three days old folder, an old folder holding a file of today, a new file and an old one, a day's rule lists {listed:?}"), listed == ["old.bin", "stale"]);
    ok("kinds of download", clean::category("a.JPG") == "image" && clean::category("b.mp4") == "video" && clean::category("c.crdownload").is_empty() && clean::category("d") == "other" && clean::retention(5) == 7 && clean::retention(14) == 14);

    // ── What starts with Windows: read only ───────────────────────────────────
    println!("\nstartup (read only)");
    let entries = startup::list();
    for e in &entries {
        println!("  {:<8} {:<32} {}{}", e.place, e.name.chars().take(32).collect::<String>(), e.command.chars().take(70).collect::<String>(), if e.missing { "   <- its program is gone" } else { "" });
    }
    ok(&format!("{} entries, {} of a program that is gone", entries.len(), entries.iter().filter(|e| e.missing).count()), !entries.is_empty());

    // ── A start that did not finish ───────────────────────────────────────────
    println!("\nstartup safety");
    let config = tree.join("config");
    startup::settled(&config);
    let counts = [startup::begin(&config), startup::begin(&config), startup::begin(&config)];
    ok(&format!("three starts that never settled count {counts:?}: the third comes up bare"), counts == [0, 1, 2]);
    startup::settled(&config);
    ok("a start that settled clears the count", startup::begin(&config) == 0);
    startup::settled(&config);

    let failed = FAILED.load(SeqCst);
    println!("\n{}", if failed == 0 { "all checks passed".to_string() } else { format!("{failed} checks FAILED") });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}

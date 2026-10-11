// Proves the Windows-facing half of Apps on this PC. It only reads: the
// registry, the Store package list, `winget upgrade` (a listing) and one
// `winget search`. It installs, upgrades and removes nothing, opens no browser
// and changes no file. The checks of the pure logic are asserts: it stops at
// the first one that does not hold.
//
//   cargo run -p notch-feat-apps --example apps-probe

use std::cmp::Ordering::*;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notch_feat_apps::logic::*;
use notch_feat_apps::{job_args, search_args, self_check, LIST};
use serde_json::json;
use svc_apps::winget;

/// What winget prints for an install, written out by hand from its known shape
/// (nothing may be installed on this PC to record one): escapes, a bar redrawn
/// in place with sizes, then one with a percentage.
const TRANSCRIPT: &str = "\u{1b}[?25l   - \r   \\ \r   | \r\u{1b}[?25hFound 7-Zip [7zip.7zip] Version 24.08\r\n\
This application is licensed to you by its owner.\r\n\
Microsoft is not responsible for, nor does it grant any licenses to, third-party packages.\r\n\
Downloading https://www.7-zip.org/a/7z2408-x64.exe\r\n\
\r  \u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}  0.00 B / 1.55 MB\
\r  \u{2588}\u{2588}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}  320 KB / 1.55 MB\
\r  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2592}\u{2592}\u{2592}\u{2592}  1.00 MB / 1.55 MB\
\r  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}  1.55 MB / 1.55 MB\r\n\
Successfully verified installer hash\r\n\
Starting package install...\r\n\
\r  \u{2588}\u{2588}\u{2588}\u{2588}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}\u{2592}  45%\
\r  \u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}  100%\r\n\
\u{1b}[32mSuccessfully installed\u{1b}[0m\r\n";

/// The text `winget upgrade` prints, with a Chinese name, a version that says
/// nothing, the count under the table and the second table of pinned packages.
const LISTING: &str = "   - \r   \\ \r                                \rName               Id                 Version   Available Source\r\n\
-------------------------------------------------------------------\r\n\
Git                Git.Git            2.45.1    2.46.0    winget\r\n\
\u{817e}\u{8baf}QQ             Tencent.QQ         9.7.1     9.9.15    winget\r\n\
Old Thing          Old.Thing          Unknown   3.0       winget\r\n\
Same Thing         Same.Thing         1.2       1.2.0     winget\r\n\
Photos             9WZDNCRFJBH4       2024.1    2024.2    msstore\r\n\
5 upgrades available.\r\n\
\r\n\
The following packages have an upgrade available, but require explicit targeting for upgrade:\r\n\
Name    Id              Version Available Source\r\n\
------------------------------------------------\r\n\
Discord Discord.Discord 1.0.9   1.0.10    winget\r\n";

/// Seconds since 1970 as a UTC date and time.
fn date(secs: u64) -> String {
    let (days, rest) = ((secs / 86_400) as i64 + 719_468, secs % 86_400);
    let (era, doe) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rest / 3600, rest % 3600 / 60)
}

fn main() {
    // ── 1. The installed programs ────────────────────────────────────────────
    let t0 = Instant::now();
    let apps = svc_apps::installed();
    let store = apps.iter().filter(|a| a.store).count();
    println!("== installed: {} programs ({} from the registry, {} Store packages) in {} ms", apps.len(), apps.len() - store, store, t0.elapsed().as_millis());
    let step = (apps.len() / 5).max(1);
    for a in apps.iter().step_by(step).take(5) {
        println!("   {:<38.38} {:<18.18} {:<24.24} {}", a.name, a.version, a.publisher, a.key.split(':').next().unwrap_or(""));
    }
    assert!(!apps.is_empty(), "no installed program was found");

    // ── 2. winget: the listing of updates, read only ─────────────────────────
    let never = AtomicBool::new(false);
    match winget::exe() {
        None => println!("== winget: not on this PC; the listing and the search are skipped"),
        Some(exe) => {
            println!("== winget: {}", exe.display());
            let t0 = Instant::now();
            let out = svc_apps::run(&exe, &LIST, Duration::from_secs(120), &never, |_| {}).expect("winget upgrade");
            let table = winget::table(&out.text).unwrap_or_default();
            let cut = table.iter().filter(|r| r.iter().any(|c| c.contains('\u{2026}'))).count();
            let widest = out.text.lines().map(|l| l.chars().count()).max().unwrap_or(0);
            println!("== winget {}: exit {:?}, {} bytes, {} table rows ({cut} cut short by winget, widest line {widest}), {} ms", LIST.join(" "), out.code, out.text.len(), table.len(), t0.elapsed().as_millis());
            match parse_updates(out.code, &out.text) {
                Err(e) => println!("   could not be read: {e}"),
                Ok(updates) => {
                    println!("   {} updates after the checks (a valid id, both versions say something, available is newer)", updates.len());
                    for u in &updates {
                        println!("   {:<34.34} {:<34.34} {:>14.14} -> {:<14.14} {}", u.name, u.id, u.version, u.available, u.source);
                    }
                    if let Some(u) = updates.first() {
                        println!("   an upgrade of the first one would run (not run here): winget {}", job_args("upgrade", &u.id, &u.source).join(" "));
                    }
                }
            }

            // ── 3. winget: a search ──────────────────────────────────────────
            let query = clean_query("  --7zip ");
            let t0 = Instant::now();
            let out = svc_apps::run(&exe, &search_args(&query), Duration::from_secs(30), &never, |_| {}).expect("winget search");
            println!("== winget {}: exit {:?}, {} ms", search_args(&query).join(" "), out.code, t0.elapsed().as_millis());
            match parse_search(out.code, &out.text) {
                Err(e) => println!("   could not be read: {e}"),
                Ok(found) => {
                    println!("   {} packages found", found.len());
                    for f in found.iter().take(5) {
                        println!("   {:<40.40} {:<34.34} {}", f.name, f.id, f.version);
                    }
                }
            }
        }
    }

    // ── The runner stops what it started: on request, and after a silence ────
    let system = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap_or(r"C:\Windows".into())).join("System32");
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1200));
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let (t0, mut pieces) = (Instant::now(), 0);
    let out = svc_apps::run(&system.join("ping.exe"), &["-n", "30", "127.0.0.1"], Duration::from_secs(60), &stop, |_| pieces += 1).expect("ping");
    println!("== runner: `ping -n 30 127.0.0.1` asked to stop at 1.2 s, gone after {} ms ({pieces} pieces of output on the way)", t0.elapsed().as_millis());
    assert!(out.cancelled && out.code.is_none() && pieces > 0 && t0.elapsed() < Duration::from_secs(4));
    let t0 = Instant::now();
    let sleeper = system.join("WindowsPowerShell").join("v1.0").join("powershell.exe");
    let out = svc_apps::run(&sleeper, &["-NoProfile", "-NonInteractive", "-Command", "Start-Sleep 20"], Duration::from_secs(1), &never, |_| {}).expect("sleeper");
    println!("== runner: a program that sleeps 20 s in silence, closed after {} ms for saying nothing for 1 s", t0.elapsed().as_millis());
    assert!(out.timed_out && t0.elapsed() < Duration::from_secs(4));

    // ── 4. The listing parser, on a text with the hard cases ─────────────────
    let listed = parse_updates(Some(0), LISTING).unwrap();
    let ids: Vec<&str> = listed.iter().map(|u| u.id.as_str()).collect();
    println!("== listing parser: {ids:?}");
    // Kept: Git, the Chinese name (two cells a character) and the Store app, the Store last.
    // Left out: a version that says nothing, 1.2 against 1.2.0, the count line, the pinned table.
    assert_eq!(ids, ["Git.Git", "Tencent.QQ", "9WZDNCRFJBH4"]);
    assert_eq!(listed[1].name, "\u{817e}\u{8baf}QQ");
    assert_eq!((listed[1].version.as_str(), listed[1].available.as_str()), ("9.7.1", "9.9.15"));
    assert_eq!(listed[2].source, "msstore");
    // A list that failed is "could not check", never "no updates".
    assert!(parse_updates(Some(0x8A15_0005_u32 as i32), "Failed when opening source(s)").is_err());
    assert_eq!(parse_updates(Some(0), "No installed package found matching input criteria.").unwrap(), vec![]);

    // ── 5. What may stand on a command line ──────────────────────────────────
    for good in ["Git.Git", "7zip.7zip", "9NBLGGH4NNS1", "Microsoft.VCRedist.2015+.x64", "a_b-c"] {
        assert!(winget::valid_id(good), "{good}");
    }
    for bad in ["", "-e", "--all", "Git.Git --override x", "a..b", "a/b", "a\\b", "a\"b", "a;b", "Git.Gi\u{2026}", "\u{817e}\u{8baf}"] {
        assert!(!winget::valid_id(bad), "{bad}");
    }
    assert_eq!(clean_query("  --all /? vs\u{7}code "), "all /? vscode");
    assert_eq!(svc_apps::strip_vt("\u{1b}[32mok\u{1b}[0m \u{1b}]0;title\u{7}done"), "ok done");
    println!("== ids, queries and escapes: ok");

    // ── 6. The progress reader, fed the transcript nine characters at a time ─────
    println!("== progress:");
    let mut progress = Progress::default();
    let mut seen = Vec::new();
    let clean = svc_apps::strip_vt(TRANSCRIPT);
    let chars: Vec<char> = clean.chars().collect();
    for piece in chars.chunks(9) {
        progress.feed(&piece.iter().collect::<String>());
        let now = (progress.phase, progress.fraction.map(|f| (f * 100.0).round() as u32));
        if seen.last() != Some(&now) {
            println!("   {:<12} {:>4}  {}", format!("{:?}", now.0), now.1.map_or("-".into(), |p| format!("{p}%")), progress.line);
            seen.push(now);
        }
    }
    assert!(seen.contains(&(Phase::Downloading, Some(20))), "320 KB of 1.55 MB");
    assert!(seen.contains(&(Phase::Downloading, Some(100))));
    assert!(seen.contains(&(Phase::Verifying, None)));
    assert!(seen.contains(&(Phase::Installing, Some(45))));
    assert_eq!(seen.last(), Some(&(Phase::Done, None)));
    assert_eq!(progress.line, "Successfully installed");

    // ── 7. Skip and exclude ──────────────────────────────────────────────────
    let stored = json!([
        { "id": "Git.Git", "name": "Git", "version": "v2.46.0" },
        { "id": "Tencent.QQ", "name": "QQ" },
        { "id": "git.git", "name": "a second rule for Git" },
        { "id": "--all", "name": "not an id" },
        { "id": "Some.App", "name": "skips nothing", "version": "latest" }
    ]);
    let rules = rules(&stored);
    assert_eq!(rules.len(), 2, "one rule per app, and only for an id winget would write");
    let left: Vec<String> = visible(&listed, &rules).into_iter().map(|u| u.id).collect();
    assert_eq!(left, ["9WZDNCRFJBH4"], "Git 2.46.0 is skipped, QQ is excluded");
    let mut later = listed.clone();
    later[0].available = "2.47.0".into();
    let left: Vec<String> = visible(&later, &rules).into_iter().map(|u| u.id).collect();
    assert_eq!(left, ["Git.Git", "9WZDNCRFJBH4"], "the release after the skipped one is shown again");
    println!("== rules: skip hides one release, exclude hides the app: ok");

    // ── 8. The schedule ──────────────────────────────────────────────────────
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let (day, week) = (86_400, 7 * 86_400);
    assert_eq!(next_check(Some(now), "off", now), None);
    assert_eq!(next_check(None, "weekly", now), Some(now + CATCH_UP));
    assert_eq!(next_check(Some(now - 3 * day), "weekly", now), Some(now + 4 * day));
    assert_eq!(next_check(Some(now - 8 * day), "weekly", now), Some(now + CATCH_UP));
    assert_eq!(next_check(Some(now - week), "weekly", now), Some(now + CATCH_UP));
    assert_eq!(next_check(Some(now - 3600), "daily", now), Some(now + day - 3600));
    println!("== schedule (now {}):", date(now));
    println!("   weekly, checked 3 days ago  -> {}", date(next_check(Some(now - 3 * day), "weekly", now).unwrap()));
    println!("   weekly, checked 8 days ago  -> {} (three minutes after the app is up)", date(next_check(Some(now - 8 * day), "weekly", now).unwrap()));
    println!("   weekly, never checked       -> {}", date(next_check(None, "weekly", now).unwrap()));
    println!("   daily, checked an hour ago  -> {}", date(next_check(Some(now - 3600), "daily", now).unwrap()));

    // ── 9. Versions ──────────────────────────────────────────────────────────
    let cases = [
        ("130.0", "129.9", Greater), ("1.2", "1.2.0", Equal), ("1.2.1", "1.2", Greater), ("1.10", "1.9", Greater),
        ("10", "9a", Greater), ("5", "5beta", Greater), ("1.2.3", "1.2.3", Equal), ("2.45.1.windows.1", "2.45.1", Greater),
        ("007", "7", Equal), ("99999999999999999999999.1", "99999999999999999999998.9", Greater), ("1.0A", "1.0a", Equal),
    ];
    for (a, b, want) in cases {
        assert_eq!(compare(a, b), want, "{a} against {b}");
        assert_eq!(compare(b, a), want.reverse(), "{b} against {a}");
    }
    assert!(is_newer("v3.5.262,260717d", "3.5.261") && !is_newer("3.5.262,abc", "v3.5.262"));
    assert!(comparable("1.2") && !comparable("Unknown") && !comparable("< 3.2") && !comparable("latest") && !comparable("1.2\u{2026}") && !comparable(" "));
    // The app's own releases: stable only, a release beats its own pre-release.
    assert!(release_is_newer("v0.4.0", "0.3.0") && release_is_newer("0.3.1", "0.3.0") && release_is_newer("1.0.0", "1.0.0-beta.2"));
    assert!(!release_is_newer("0.3.0", "0.3.0") && !release_is_newer("0.2.9", "0.3.0") && !release_is_newer("0.4.0-beta.1", "0.3.0") && !release_is_newer("soon", "0.3.0"));
    println!("== versions: {} pairs both ways, releases, uncomparable ones: ok", cases.len());

    // ── 10. The app's own update check (one GET of the feed; nothing is downloaded or run) ──
    let running = env!("CARGO_PKG_VERSION");
    match self_check(running) {
        Ok(v) => println!("== own update: running {running}, published {}, newer {}; offered as {}", v["latest"], v["newer"], v["page"]),
        Err(e) => println!("== own update: running {running}; the feed did not answer ({e})"),
    }
}

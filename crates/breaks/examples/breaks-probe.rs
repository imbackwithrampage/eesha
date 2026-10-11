// Breaks probe: the Windows-facing parts on this PC, the break rule over a made-up timeline, and
// the feature's engine run as the app runs it. It reads and it draws; it plays no sound and sends
// no input. The fade is put on every screen twice, for about two seconds and for about one.
//
//   cargo run -p notch-feat-breaks --example breaks-probe
//   cargo run -p notch-feat-breaks --example breaks-probe -- --fade 15    (only the fade: try Esc, try a click)

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notch_feat_breaks::fade::{Fade, CLASS};
use notch_feat_breaks::rule::{Config, Ev, Kind, Plan, Rule, RECHECK};
use notch_feat_breaks::usage::{self, Usage};
use notch_feat_breaks::{sys, Engine, IDLE_AFTER};
use serde_json::{json, Value};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetSystemMetrics, SM_CMONITORS};

fn wait(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn front(fg: &svc_sysevents::Foreground) {
    match sys::app_of(fg.hwnd, fg.pid, &fg.exe) {
        Some((key, name)) => println!("  in front: {name} [{key}] category={} fullscreen={}", usage::category(&key), fg.fullscreen),
        None => println!("  in front: a window Windows will not name (exe {:?})", fg.exe),
    }
}

fn fade_on_screen() -> bool {
    unsafe { FindWindowW(CLASS, None) }.is_ok()
}

fn main() {
    // As in the app: a monitor's rectangle is in real pixels.
    unsafe { _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

    // `-- --fade 15`: only the fade, for that many seconds, to look at it and to try Esc and a click.
    let args: Vec<String> = std::env::args().collect();
    if let Some(secs) = args.iter().position(|a| a == "--fade").and_then(|i| args.get(i + 1)?.parse::<u64>().ok()) {
        let t0 = Instant::now();
        let fade = Fade::show(secs, "Look at something far away".into(), "Esc or click to skip".into(), || println!("skipped by Esc or a click"));
        while !fade.gone() {
            wait(50);
        }
        println!("gone after {} ms; a window left: {}", t0.elapsed().as_millis(), fade_on_screen());
        return;
    }

    println!("== idle clock (GetLastInputInfo)");
    let a = sys::idle_secs();
    wait(2000);
    let b = sys::idle_secs();
    let note = if b >= a + 2 { "no input meanwhile" } else { "something typed or moved the pointer meanwhile" };
    println!("  idle for {a} s, and two seconds later for {b} s: {note}");
    println!("  away after {IDLE_AFTER} s without input: {}", if b >= IDLE_AFTER { "away now" } else { "at the PC now" });

    println!("== microphone (ConsentStore: LastUsedTimeStop = 0 means open)");
    let mic = sys::mic_in_use();
    println!("  {} app(s) have it open: {mic:?} -> a break would {}", mic.len(), if mic.is_empty() { "start" } else { "wait" });

    println!("== window in front (the WinEvent hook of svc-sysevents), 4 s");
    front(&svc_sysevents::foreground());
    let changes = Arc::new(AtomicU32::new(0));
    let sub = svc_sysevents::on_foreground({
        let changes = changes.clone();
        move |fg| {
            changes.fetch_add(1, Relaxed);
            front(fg);
        }
    });
    wait(4000);
    drop(sub);
    println!("  {} change(s) in 4 s, told by the hook, never asked for", changes.load(Relaxed));

    println!("== the break rule over a made-up timeline (eye: every 60 s for 5 s; movement: every 200 s for 10 s)");
    rule_timeline();

    println!("== screen time accounting (names and seconds only)");
    screen_time();

    println!("== the engine, as the app runs it");
    engine_live();

    println!("== the fade");
    let monitors = unsafe { GetSystemMetrics(SM_CMONITORS) };
    let skipped = Arc::new(AtomicBool::new(false));
    let told = skipped.clone();
    let t0 = Instant::now();
    let fade = Fade::show(2, "Look at something far away".into(), "Esc or click to skip".into(), move || told.store(true, Relaxed));
    println!("  {} window(s) for {monitors} monitor(s), on screen: {}", fade.windows, fade_on_screen());
    assert_eq!(fade.windows as i32, monitors, "one window per monitor");
    assert!(fade_on_screen());
    while !fade.gone() && t0.elapsed() < Duration::from_secs(6) {
        wait(50);
    }
    println!("  gone by itself after {} ms (a 2 s countdown, then the fade-out); a window left: {}", t0.elapsed().as_millis(), fade_on_screen());
    assert!(fade.gone() && !fade_on_screen(), "it must end with its countdown");
    if skipped.load(Relaxed) {
        println!("  (someone pressed Esc or clicked meanwhile: that ended it early)");
    }

    // The real path: the feature fades the screens for a break, and is switched off a second in.
    let engine = Engine::new(Arc::new(|_| {}), std::path::PathBuf::from("unused"));
    engine.apply(&json!({ "eye": true, "eyeEvery": 20, "eyeSecs": 60, "fade": true }));
    engine.take(Kind::Eye);
    wait(1000);
    println!("  a 60 s break with the fade on: on screen: {}", fade_on_screen());
    assert!(fade_on_screen());
    let t0 = Instant::now();
    engine.stop();
    println!("  the feature switched off: stop returned after {} ms; a window left: {}", t0.elapsed().as_millis(), fade_on_screen());
    assert!(!fade_on_screen(), "stop must give the screens back before it returns");

    println!("probe ok");
}

/// Threads of this process, as Windows counts them.
fn threads() -> usize {
    let mut n = 0;
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return 0 };
        let mut entry = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut more = Thread32First(snapshot, &mut entry).is_ok();
        while more {
            n += (entry.th32OwnerProcessID == std::process::id()) as usize;
            more = Thread32Next(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    n
}

/// The real engine with the app's settings: off, on, a break by hand, snoozed, off again, stopped.
fn engine_live() {
    // Inside the repository's build folder, and removed at the end.
    let file = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/breaks-probe-screen-time.json");
    let _ = std::fs::remove_file(&file);
    let t0 = Instant::now();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let engine = Engine::new(
        Arc::new({
            let seen = seen.clone();
            move |v: Value| {
                if let Some(ev) = v.get("ev") {
                    println!("  [{:>5} ms] {} {} {}", t0.elapsed().as_millis(), ev["phase"], ev["kind"], ev["why"]);
                    seen.lock().unwrap().push(format!("{} {} {}", ev["phase"].as_str().unwrap_or(""), ev["kind"].as_str().unwrap_or(""), ev["why"].as_str().unwrap_or("")));
                }
            }
        }),
        file.clone(),
    );
    let settings = |eye: bool, screen_time: bool| {
        json!({ "eye": eye, "eyeEvery": 20, "eyeSecs": 5, "move": false, "moveEvery": 50, "moveMins": 2, "snooze": 5,
                "holdCall": true, "holdFullscreen": true, "fade": false, "sound": false, "screenTime": screen_time })
    };
    let before = threads();
    engine.apply(&settings(false, false));
    wait(300);
    println!("  everything off: {} thread(s) before, {} after -> the feature runs nothing", before, threads());
    assert!(threads() <= before, "with everything off nothing may start");

    engine.apply(&settings(true, true));
    wait(2500);
    println!("  eye breaks and screen time on: {} thread(s) (the loop, and the service that listens to Windows)", threads());
    println!("  status: {}", engine.status_now());
    let usage = engine.usage(false);
    println!("  screen time after 2.5 s: {} s counted, apps {} ({})", usage["total"], usage["apps"],
        if sys::idle_secs() >= IDLE_AFTER { "the person is away: the clock stands still" } else { "the person is here" });

    engine.take(Kind::Eye);
    wait(6500);
    assert_eq!(*seen.lock().unwrap(), ["start eye ", "end eye done"], "a 5 s break by hand starts and ends by itself");
    engine.take(Kind::Eye);
    wait(1200);
    engine.end("snooze");
    assert_eq!(seen.lock().unwrap().last().map(String::as_str), Some("end eye snooze"));
    println!("  status after the snooze: {}", engine.status_now());

    engine.apply(&settings(false, false));
    wait(1500);
    println!("  everything off again: {} thread(s)", threads());
    engine.stop();
    wait(300);
    println!("  stopped: {} thread(s) (what Windows' own thread pool keeps for a while is not the feature's)", threads());
    let _ = std::fs::remove_file(&file);
}

fn rule_timeline() {
    let mut rule = Rule::default();
    rule.cfg = Config {
        eye: Plan { on: true, every: 60, secs: 5 },
        mov: Plan { on: true, every: 200, secs: 10 },
        snooze: 20,
        warn: 10,
    };
    let mut sim = Sim { rule, t: 0, log: Vec::new() };
    let eye = Ev::Start(Kind::Eye, 5);

    sim.span("at the PC", 60, false, false);
    assert_eq!(sim.log, [(50, Ev::Soon(Kind::Eye)), (60, eye)], "20 minutes in small: announced, then started");
    sim.span("the break runs", 5, false, false);
    assert_eq!(sim.log.last(), Some(&(65, Ev::End(Kind::Eye, "done"))));

    sim.span("at the PC", 30, false, false);
    let n = sim.log.len();
    sim.span("idle (no input)", 20, true, false);
    assert_eq!(sim.log.len(), n, "nothing happens while idle");
    let back = sim.t;
    let at = sim.until("back at the PC", |ev| ev == eye);
    assert_eq!(at, back + 60, "the idle time was the break: a full interval again");
    sim.by_hand("Skip", |rule| rule.skip());
    assert_eq!(sim.log.last(), Some(&(at, Ev::End(Kind::Eye, "skip"))));

    sim.span("at the PC", 40, false, false);
    let n = sim.log.len();
    sim.span("locked", 30, true, false);
    assert_eq!(sim.log.len(), n, "nothing happens while locked");

    let unlocked = sim.t;
    sim.span("at the PC, on a call", 85, false, true);
    assert_eq!(sim.log.last(), Some(&(unlocked + 60, Ev::Held(Kind::Eye))), "due a full interval after the lock, and held");
    let hung_up = sim.t;
    let at = sim.until("the call is over", |ev| ev == eye);
    assert!(at <= hung_up + RECHECK, "it starts within {RECHECK} s of the call ending");
    sim.by_hand("Snooze", |rule| rule.stop("snooze"));
    let again = sim.until("at the PC", |ev| ev == eye);
    assert_eq!(again, at + 20, "snoozed: due again after 20 s at the PC");
    sim.span("the break runs", 5, false, false);

    sim.until("at the PC until the movement break", |ev| ev == Ev::Start(Kind::Move, 10));
    sim.span("the break runs", 10, false, false);
    assert_eq!(sim.log.last().map(|e| e.1), Some(Ev::End(Kind::Move, "done")));
    let status = sim.rule.status();
    println!("  status at the end: {status}");
    assert_eq!(status["eye"], 60, "standing up rested the eyes too");
    println!("  rule ok: {} events over {} made-up seconds", sim.log.len(), sim.t);
}

/// The rule fed one made-up second at a time.
struct Sim {
    rule: Rule,
    t: u64,
    log: Vec<(u64, Ev)>,
}

impl Sim {
    /// `away` is idle, locked or asleep; `busy` a call or a full-screen app.
    fn second(&mut self, away: bool, busy: bool) -> Vec<Ev> {
        self.t += 1;
        let evs = self.rule.tick(1, away, || busy);
        for ev in &evs {
            println!("  t={:>3}    -> {ev:?}", self.t);
            self.log.push((self.t, *ev));
        }
        evs
    }

    fn span(&mut self, what: &str, secs: u64, away: bool, busy: bool) {
        println!("  t={:>3}  {what}, {secs} s", self.t + 1);
        for _ in 0..secs {
            self.second(away, busy);
        }
    }

    /// At the PC and free until `wanted` happens: the second it did.
    fn until(&mut self, what: &str, wanted: impl Fn(Ev) -> bool) -> u64 {
        println!("  t={:>3}  {what}", self.t + 1);
        for _ in 0..1000 {
            if self.second(false, false).into_iter().any(&wanted) {
                return self.t;
            }
        }
        panic!("{what}: it never happened");
    }

    fn by_hand(&mut self, what: &str, act: impl FnOnce(&mut Rule) -> Option<Ev>) {
        let ev = act(&mut self.rule);
        println!("  t={:>3}  {what} -> {ev:?}", self.t);
        self.log.extend(ev.map(|e| (self.t, e)));
    }
}

fn screen_time() {
    let mut usage = Usage::load(None);
    let app = |key: &str, name: &str| Some((key.to_string(), name.to_string()));
    let day = "2026-10-11";
    usage.front(day, app("code", "Visual Studio Code"));
    usage.add(day, 1500);
    usage.front(day, app("chrome", "Google Chrome"));
    usage.add(day, 600);
    usage.front(day, None); // this app's own window: nobody's time
    usage.add(day, 30);
    usage.front(day, app("code", "Visual Studio Code"));
    usage.add(day, 300);
    let snap = usage.snapshot(day);
    println!("  today: {} s in {} switch(es): {}", snap["total"], snap["switches"], snap["apps"]);
    assert_eq!((snap["total"].as_u64(), snap["switches"].as_u64()), (Some(2400), Some(2)));
    assert_eq!(snap["apps"][0], serde_json::json!({ "name": "Visual Studio Code", "secs": 1800, "cat": "development" }));
    for d in 12..=20 {
        usage.add(&format!("2026-10-{d}"), 60);
    }
    let days = usage.snapshot(day)["days"].as_object().map_or(0, |d| d.len());
    println!("  days kept after ten days of use: {days}");
    assert_eq!(days, 7);
    for key in ["idea64", "gimp-2.10", "ms-teams", "explorer", "some-game"] {
        print!("  {key}={}", usage::category(key));
    }
    println!();
}

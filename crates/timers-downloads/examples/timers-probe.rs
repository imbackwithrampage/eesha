// Pomodoro probe: long break every 2 focus, 3 sessions, auto-start; a skipped focus; a simulated sleep.
// Rust only, so no sound.
use notch_feat_timers_downloads::clock::{Clock, Emit};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let t0 = Instant::now();
    let emit: Emit = Arc::new(move |ev: &str, v: Value| {
        if ev == "done" {
            println!("[{:>5}ms] done: phase={} next={} skipped={}", t0.elapsed().as_millis(), v["phase"], v["next"], v["skipped"]);
        }
    });
    let c = Clock::new(emit);
    let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
    let show = |what: &str, c: &Clock| {
        let s = c.snapshot();
        println!("{what}: status={} phase={} session={}/{}", s["status"], s["phase"], s["session"], s["sessions"]);
        s
    };
    // 0.02 min = 1.2 s per phase
    c.start(&json!({ "mode": "pomodoro", "focus": 0.02, "short": 0.02, "long": 0.02, "every": 2, "sessions": 3, "auto": true }));
    show("focus 1", &c);
    wait(1500);
    let s = show("auto short break", &c);
    assert!(s["phase"] == "short" && s["status"] == "running");
    wait(1200);
    let s = show("focus 2 (auto-started)", &c);
    assert!(s["phase"] == "focus" && s["session"] == 2);
    wait(1200);
    let s = show("long break after 2 focus", &c);
    assert!(s["phase"] == "long" && s["status"] == "running");
    wait(1200);
    let s = show("focus 3", &c);
    assert!(s["phase"] == "focus" && s["session"] == 3);
    c.skip();
    let s = show("focus 3 skipped, short break not long", &c);
    assert!(s["phase"] == "short" && s["session"] == 2, "a skipped focus must not count");
    // simulated sleep: the end date moves back 5 s while running; the overdue phase ends at once
    c.shift(5000);
    wait(300);
    let s = show("end date moved (slept through the break)", &c);
    assert!(s["phase"] == "focus" && s["status"] == "running", "overdue phase must end");
    // pause on sleep: the time left stays put
    c.pause();
    let before = c.snapshot()["remainingMs"].as_u64().unwrap();
    wait(500);
    assert_eq!(before, c.snapshot()["remainingMs"].as_u64().unwrap());
    println!("paused: remainingMs unchanged ({before})");
    c.resume();
    wait(1500);
    let s = show("end", &c);
    assert!(s["status"] == "finished", "3 focus sessions done");
    c.cancel();
    println!("probe ok");
}

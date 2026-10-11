// Proves the feature's own OS code without the app:
// `cargo run -p notch-feat-huds-power --example huds-probe`. Prints the battery
// report, then brightness changes for about 20 seconds. Observes only.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notch_feat_huds_power::{battery_json, brightness, mic, ChargeLimit, Switch, SWITCH_QUIET};

/// The rule of the level that follows an output switch, over a made-up run:
/// earbuds connect, the output goes to them, and they say their own volume.
fn switch_rule() {
    let ms = Duration::from_millis;
    let t0 = Instant::now();
    let mut rule = Switch::default();
    let level = |rule: &mut Switch, what: &str, at: Duration, key: bool, quiet: bool| {
        let got = rule.quiet(t0 + at, key);
        println!("{:>6} ms  {what:<58} -> {}", at.as_millis(), if got { "quiet (no notice)" } else { "a volume notice" });
        assert_eq!(got, quiet, "{what}");
    };
    println!("== a level right after an output switch (quiet for {} s) ==", SWITCH_QUIET.as_secs());
    level(&mut rule, "the slider moves, no switch before it", ms(0), false, false);
    rule.devices(t0 + ms(1000));
    println!("{:>6} ms  earbuds connect: the output is theirs now", 1000);
    level(&mut rule, "the earbuds say their own volume (96 %)", ms(1400), false, true);
    level(&mut rule, "…and once more, a step off", ms(2100), false, true);
    rule.devices(t0 + ms(2600));
    println!("{:>6} ms  their microphone arrives: the devices changed again", 2600);
    level(&mut rule, "a late report of the earbuds", ms(5500), false, true);
    level(&mut rule, "the slider, long after the switch", ms(5700), false, false);
    rule.devices(t0 + ms(10_000));
    println!("{:>6} ms  the output is switched by hand", 10_000);
    level(&mut rule, "a volume key right after it: the user's", ms(10_300), true, false);
    level(&mut rule, "the level that key press brings a moment later", ms(10_350), false, false);
}

fn main() {
    switch_rule();

    // Read-only: the apps that hold the microphone now (the registry is never written).
    println!("microphone in use by: {:?}", mic::in_use());
    // The charge-limit rule over a made-up run: (ac, percent) -> reminder?
    let mut rule = ChargeLimit::default();
    let run: [(bool, Option<u8>); 10] = [
        (true, Some(70)), (true, Some(80)), (true, Some(85)), (true, None), (true, Some(76)),
        (true, Some(75)), (true, Some(80)), (false, Some(80)), (true, Some(81)), (true, Some(82)),
    ];
    for (ac, p) in run {
        println!("limit 80: ac {ac} {p:?} -> {}", rule.feed(80, ac, p));
    }

    println!("battery: {}", battery_json());

    let start = Instant::now();
    let stop = Arc::new(AtomicBool::new(false));
    let listener = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            brightness::listen(&stop, |percent| println!("{:5.1}s brightness {percent}", start.elapsed().as_secs_f32()))
        })
    };
    println!("listening for 20 s: press the brightness keys…");
    std::thread::sleep(Duration::from_secs(20));
    stop.store(true, Relaxed);
    // Ok = the WMI subscription was live the whole time; an error = no such panel here.
    println!("{:5.1}s listener ended: {:?}", start.elapsed().as_secs_f32(), listener.join().unwrap());
}

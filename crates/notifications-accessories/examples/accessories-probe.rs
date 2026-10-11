// Proves the accessories half without the app:
// `cargo run -p notch-feat-notifications-accessories --example accessories-probe`.
// Reads only: the paired Bluetooth devices as Windows lists them, the rules
// over a made-up run of events, where the notification mirror stands, and the
// live watcher for a few seconds. Nothing is connected, registered or asked.

use std::time::{Duration, Instant};

use notch_feat_notifications_accessories::bt::{self, Battery, Connections, Kind};
use notch_feat_notifications_accessories::{device_json, log_line, mirror};

/// A device name is the owner's business: four letters are enough to tell rows apart.
fn short(name: &str) -> String {
    format!("{}…", name.chars().take(4).collect::<String>())
}

fn main() {
    println!("== paired accessories, as Windows lists them ==");
    let asked = Instant::now();
    match bt::devices() {
        Ok(devices) => {
            println!("listed, each with its battery, in {} ms", asked.elapsed().as_millis());
            for d in &devices {
                println!(
                    "{:<6} kind {:<8?} headphone battery {:<5} connected {:<5} battery {:?} tone {:?}",
                    short(&d.name),
                    d.kind,
                    d.kind.is_audio(),
                    d.connected,
                    d.battery,
                    d.battery.and_then(|b| b.tone()),
                );
            }
            println!("{} accessories", devices.len());
            if let Some(d) = devices.first() {
                let mut d = d.clone();
                d.name = short(&d.name);
                println!("as the page gets it: {}", device_json(&d));
                // A connect waits this long for its battery and no longer.
                let asked = Instant::now();
                let mut read = bt::battery_within(d.clone(), bt::BATTERY_WAIT);
                println!("battery within {} ms: {:?}, after {} ms", bt::BATTERY_WAIT.as_millis(), read.battery, asked.elapsed().as_millis());
                d.battery = None;
                let asked = Instant::now();
                let late = bt::battery_within(d, Duration::ZERO);
                println!("a battery that does not come in time: {:?}, told after {} ms all the same", late.battery, asked.elapsed().as_millis());
                assert_eq!(late.battery, None);
                read.connected = true;
                println!("the log on a connect:    {}", log_line(&read));
                read.connected = false;
                println!("the log on a disconnect: {}", log_line(&read));
            }
        }
        Err(err) => println!("could not list: {}", err.message()),
    }

    println!("\n== what a device is (class of device, or LE appearance) ==");
    let cases: [(&str, Option<u32>, u32, Option<u32>); 9] = [
        ("headset, major 4", Some(4), 0x01, None),
        ("mouse, major 5 pointing", Some(5), 0x20, None),
        ("keyboard, major 5", Some(5), 0x10, None),
        ("gamepad, major 5", Some(5), 0x02, None),
        ("phone, major 2", Some(2), 0x03, None),
        ("computer, major 1", Some(1), 0x03, None),
        ("LE earbuds, appearance 0x0941", None, 0, Some(0x0941)),
        ("LE mouse, appearance 0x03C2", None, 0, Some(0x03C2)),
        ("says nothing", None, 0, None),
    ];
    for (what, major, minor, appearance) in cases {
        match bt::kind(major, minor, appearance) {
            Some(kind) => println!("{what:<32} -> {kind:?}, headphone battery: {}", kind.is_audio()),
            None => println!("{what:<32} -> not an accessory, never announced"),
        }
    }
    assert_eq!(bt::kind(None, 0, None), Some(Kind::Other));
    assert_eq!(bt::kind(Some(2), 0, None), None);
    assert!(Kind::Audio.is_audio() && !Kind::Mouse.is_audio());

    println!("\n== who is announced, over a made-up run ==");
    let mut rule = Connections::default();
    let step = |rule: &mut Connections, what: &str, device: &str, link: &str, up: bool| {
        let pending = rule.feed(device, link, up);
        println!("{what:<46} -> {}", if pending { "news, settle" } else { "nothing" });
        pending
    };
    assert!(!step(&mut rule, "start: headset already connected", "HEADSET", "classic", true));
    assert!(!step(&mut rule, "start: mouse paired, away", "MOUSE", "le", false));
    rule.prime();
    println!("first listing over");
    assert_eq!(rule.settle("HEADSET"), None);
    println!("{:<46} -> {:?}", "headset, connected at start", rule.settle("HEADSET"));
    assert!(step(&mut rule, "mouse connects", "MOUSE", "le", true));
    println!("{:<46} -> {:?}", "  after the settle time", rule.settle("MOUSE"));
    assert!(!step(&mut rule, "headset: its LE link comes up as well", "HEADSET", "le", true));
    assert!(!step(&mut rule, "headset: classic link drops, LE still up", "HEADSET", "classic", false));
    assert!(step(&mut rule, "headset: LE link drops too", "HEADSET", "le", false));
    let told = rule.settle("HEADSET");
    assert_eq!(told, Some(false));
    println!("{:<46} -> {told:?} (disconnected)", "  after the settle time");
    assert!(step(&mut rule, "headset comes back", "HEADSET", "classic", true));
    let told = rule.settle("HEADSET");
    assert_eq!(told, Some(true));
    println!("{:<46} -> {told:?} (connected)", "  after the settle time");
    assert!(step(&mut rule, "mouse drops…", "MOUSE", "le", false));
    assert!(!step(&mut rule, "…and is back before the settle time", "MOUSE", "le", true));
    assert_eq!(rule.settle("MOUSE"), None);
    println!("{:<46} -> {:?} (a flap tells nothing)", "  after the settle time", rule.settle("MOUSE"));

    println!("\n== switched off and on: a new watcher starts from what is connected then ==");
    let mut rule = Connections::default();
    assert!(!step(&mut rule, "start: earbuds paired, in their case", "BUDS", "classic", false));
    assert!(!step(&mut rule, "start: keyboard connected", "KEYS", "le", true));
    assert!(!step(&mut rule, "a link that comes up during the first listing", "EARLY", "classic", true));
    rule.prime();
    println!("first listing over");
    assert_eq!((rule.settle("KEYS"), rule.settle("EARLY")), (None, None));
    assert!(step(&mut rule, "earbuds connect, right after the listing", "BUDS", "classic", true));
    assert_eq!(rule.settle("BUDS"), Some(true));
    println!("{:<46} -> Some(true) (connected)", "  after the settle time");
    assert!(step(&mut rule, "earbuds go back in their case", "BUDS", "classic", false));
    assert_eq!(rule.settle("BUDS"), Some(false));
    println!("{:<46} -> Some(false) (disconnected)", "  after the settle time");
    assert!(step(&mut rule, "earbuds connect again", "BUDS", "classic", true));
    assert_eq!(rule.settle("BUDS"), Some(true));
    println!("{:<46} -> Some(true) (connected, every time)", "  after the settle time");
    assert!(step(&mut rule, "keyboard, connected at the start, goes away", "KEYS", "le", false));
    assert_eq!(rule.settle("KEYS"), Some(false));
    assert!(step(&mut rule, "keyboard comes back", "KEYS", "le", true));
    assert_eq!(rule.settle("KEYS"), Some(true));
    println!("{:<46} -> Some(true) (connected)", "  after the settle time");

    println!("\n== paired after the start, then connected ==");
    assert!(!step(&mut rule, "a headset is paired, its link not up yet", "NEW", "classic", false));
    assert_eq!(rule.settle("NEW"), None);
    assert!(step(&mut rule, "its link comes up", "NEW", "classic", true));
    assert_eq!(rule.settle("NEW"), Some(true));
    println!("{:<46} -> Some(true) (connected)", "  after the settle time");
    assert!(step(&mut rule, "a mouse is paired and connected in one go", "ONE", "le", true));
    assert_eq!(rule.settle("ONE"), Some(true));
    println!("{:<46} -> Some(true) (connected)", "  after the settle time");

    println!("\n== a connect while a volume notice shows ==");
    println!("this side tells of the connect whatever the strip shows. The level that follows");
    println!("an output switch: `--example huds-probe`. The strip: tests/accessory-banner.test.mjs.");

    println!("\n== the colour of a level ==");
    for percent in [100u8, 51, 50, 21, 20, 5] {
        println!("{percent:>3}% -> {}", bt::tone(percent));
    }
    let buds = Battery { percent: None, left: Some(80), right: Some(15) };
    println!("left 80 right 15 -> {:?} (the emptier side decides)", buds.tone());
    assert_eq!((bt::tone(20), bt::tone(21), bt::tone(50), bt::tone(51)), ("bad", "warn", "warn", "ok"));
    assert_eq!(buds.tone(), Some("bad"));
    assert_eq!(Battery::default().tone(), None);

    println!("\n== the notification mirror (read only: nothing is registered or asked) ==");
    println!("identity {} · package registered {} · missing {:?}", mirror::has_identity(), mirror::registered(), mirror::missing());

    println!("\n== the live watcher, 6 s: what is connected now must not be told ==");
    let started = Instant::now();
    let watch = bt::watch(|d| println!("TOLD: {} connected {} battery {:?}", short(&d.name), d.connected, d.battery));
    match &watch {
        Ok(_) => println!("watcher started"),
        Err(err) => println!("no watcher: {}", err.message()),
    }
    if let Ok(watch) = &watch {
        // What connects before the first listing is over counts as connected at the start: this is how long that is.
        while watch.listed().is_none() && started.elapsed() < Duration::from_secs(6) {
            std::thread::sleep(Duration::from_millis(10));
        }
        println!("first listing over after {} ms, links listed: {:?}", started.elapsed().as_millis(), watch.listed());
    }
    std::thread::sleep(Duration::from_secs(6).saturating_sub(started.elapsed()));
    drop(watch);
    println!("watcher stopped");
}

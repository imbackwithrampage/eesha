// Proves the service without the app: `cargo run -p svc-sysevents --example sysevents-probe`.
// Prints the power status, then every event for about 20 seconds. Observes only.

use std::time::{Duration, Instant};

fn main() {
    let start = Instant::now();
    let stamp = move || format!("{:5.1}s", start.elapsed().as_secs_f32());

    println!("power: {:?}", svc_sysevents::power_status());
    println!("foreground: {:?}", svc_sysevents::foreground());

    let subs = [
        svc_sysevents::on_power(move |p| println!("{} power {p:?}", stamp())),
        svc_sysevents::on_session(move |s| println!("{} session {s:?}", stamp())),
        svc_sysevents::on_display_change(move || println!("{} display change", stamp())),
        svc_sysevents::on_device_change(move || println!("{} device change", stamp())),
        svc_sysevents::on_time_change(move || println!("{} time change", stamp())),
        svc_sysevents::on_foreground(move |f| println!("{} foreground {f:?}", stamp())),
    ];
    println!("listening for 20 s: switch windows, press F11 in a browser, pull the charger…");
    std::thread::sleep(Duration::from_secs(20));

    drop(subs);
    println!("{} unsubscribed: the window and its thread are gone", stamp());
}

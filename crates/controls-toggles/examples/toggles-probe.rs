// Read only: prints the current state of each toggle. Flips nothing.
use notch_feat_controls_toggles::sys;

fn main() {
    let (app_dark, sys_dark) = sys::theme();
    println!("theme: apps={} system={}", if app_dark { "dark" } else { "light" }, if sys_dark { "dark" } else { "light" });
    println!("hidden files shown: {}", sys::hidden_shown());
    println!("desktop icons shown: {:?}", sys::icons_shown());
    match sys::bin() {
        Some((n, b)) => println!("recycle bin: {n} items, {b} bytes"),
        None => println!("recycle bin: unreadable"),
    }
    match svc_audio::endpoint_volume(svc_audio::Flow::Render) {
        Ok(v) => println!("output volume: {:.0}% muted={}", v.scalar * 100.0, v.mute),
        Err(e) => println!("output volume: {e}"),
    }
    println!("brightness: {:?}", sys::brightness());
}

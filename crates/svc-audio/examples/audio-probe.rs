// Proves the service without the app: `cargo run -p svc-audio --example audio-probe`.
// Prints the default output and input with their level, the device lists, then
// every change for about 20 seconds. Observes only: it never sets anything,
// unless run with `--write`: then it first proves the output level writes (0 %
// mutes, above opens, newest of a burst wins, an outside change is told) on the
// default output with its level and mute written down first and put back,
// the level while muted. Refuses to run while anything plays.

use std::time::{Duration, Instant};

use svc_audio::Flow;

fn write_proof() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::sync::Arc;
    use svc_audio::{Change, Role};

    let outs = svc_audio::devices(Flow::Render).unwrap();
    let id = outs.iter().find(|d| d.default_for.contains(&Role::Console)).unwrap().id.clone();
    if svc_audio::sessions(Some(&id)).unwrap_or_default().iter().any(|s| s.active) {
        return println!("write proof skipped: something is playing");
    }
    let orig = svc_audio::device_volume(&id).unwrap();
    println!("write proof: {} outputs; recorded {orig:?}", outs.len());
    let (own, other) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let sub = {
        let (own, other) = (own.clone(), other.clone());
        svc_audio::on_endpoint_change(move |c| {
            if let Change::Volume { flow: Flow::Render, own: o, .. } = c {
                (if *o { &own } else { &other }).fetch_add(1, SeqCst);
            }
        })
    };
    let now = || svc_audio::endpoint_volume(Flow::Render).unwrap();
    let check = |what: &str, ok: bool| println!("  {} {what}", if ok { "ok  " } else { "FAIL" });
    svc_audio::set_device_mute(&id, true).unwrap();
    svc_audio::set_volume(Flow::Render, 0.0).unwrap();
    check("level 0 -> muted, level 0", now().mute && now().scalar == 0.0);
    svc_audio::set_volume(Flow::Render, 0.006).unwrap(); // -50 dB or so, and nothing plays
    check("0.6 % after 0 -> not muted", !now().mute && (now().scalar - 0.006).abs() < 0.002);
    svc_audio::set_volume(Flow::Render, 0.004).unwrap();
    check("0.4 % (reads 0 %) -> muted", now().mute);
    svc_audio::set_device_mute(&id, false).unwrap();
    // An outside change: no event context of ours.
    let seen = other.load(SeqCst);
    unsafe {
        use windows::Win32::Media::Audio::{Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator};
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let e: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let ep: IAudioEndpointVolume = e.GetDevice(&windows::core::HSTRING::from(&id)).unwrap().Activate(CLSCTX_ALL, None).unwrap();
        ep.SetMute(true, std::ptr::null()).unwrap();
    }
    std::thread::sleep(Duration::from_millis(300));
    check("a change from outside reaches the listener", other.load(SeqCst) > seen);
    // A burst of 6 threads x 20 writes, all below 0.5 %: the output stays silent.
    let before = own.load(SeqCst);
    let threads: Vec<_> = (0..6)
        .map(|t| std::thread::spawn(move || (0..20).for_each(|i| svc_audio::set_volume(Flow::Render, (t * 20 + i) as f32 * 0.00003).unwrap())))
        .collect();
    threads.into_iter().for_each(|t| t.join().unwrap());
    std::thread::sleep(Duration::from_millis(300));
    let applied = own.load(SeqCst) - before;
    println!("  120 writes asked, {applied} level events heard (fewer = coalesced), final {:?}", now());
    check("burst ended silent and in range", now().mute && now().scalar < 0.004);
    // Back: level while muted, then the mute.
    svc_audio::set_device_mute(&id, true).unwrap();
    svc_audio::set_device_volume(&id, orig.scalar).unwrap();
    svc_audio::set_device_mute(&id, orig.mute).unwrap();
    drop(sub);
    let back = svc_audio::device_volume(&id).unwrap();
    check(&format!("restored to {back:?}"), back.mute == orig.mute && (back.scalar - orig.scalar).abs() < 0.01);
}

fn main() {
    if std::env::args().any(|a| a == "--write") {
        return write_proof();
    }
    for flow in [Flow::Render, Flow::Capture] {
        println!("{flow:?}: default {:?}", svc_audio::endpoint_volume(flow));
        match svc_audio::devices(flow) {
            Ok(list) => list.iter().for_each(|d| println!("  {d:?}")),
            Err(err) => println!("  devices: {err}"),
        }
    }
    println!("can set the default device: {}", svc_audio::can_set_default());

    let start = Instant::now();
    let sub = svc_audio::on_endpoint_change(move |change| {
        println!("{:5.1}s {change:?}", start.elapsed().as_secs_f32());
    });
    println!("listening for 20 s: press the volume keys, mute the microphone, plug in headphones…");
    std::thread::sleep(Duration::from_secs(20));

    drop(sub);
    println!("{:5.1}s unsubscribed: the audio thread is gone", start.elapsed().as_secs_f32());
}

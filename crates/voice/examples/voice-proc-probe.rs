// Proves crates/voice/src/proc.rs without the app, without a window and without sound:
//
//   CARGO_BUILD_JOBS=4 cargo run -p notch-feat-voice --example voice-proc-probe
//
// The probe starts itself three times. The first copy only watches. It starts the second, "parent",
// which starts harmless children (copies that hold a loopback port and sleep) through `proc::spawn`,
// checks the environment, the console, the listener's process and `kill_all`, and then exits without
// ending its last child. The watcher then sees that child die with its parent: the job's kill-on-close.

use notch_feat_voice::{proc, speak_proto};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn me() -> Command {
    Command::new(std::env::current_exe().expect("own path"))
}

fn running(pid: u32) -> bool {
    notch_core::win::process_table().contains_key(&pid)
}

/// How long until `pid` is gone, if it goes within `limit`.
fn gone_within(pid: u32, limit: Duration) -> Option<Duration> {
    let from = Instant::now();
    while from.elapsed() < limit {
        if !running(pid) {
            return Some(from.elapsed());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn fail(what: &str) -> ! {
    println!("FAIL {what}");
    std::process::exit(1)
}

/// A harmless child: says what it was given, holds the port, sleeps.
fn child(port: u16) {
    let env = |name: &str| std::env::var(name).unwrap_or_else(|_| "unset".into());
    let console = unsafe { windows::Win32::System::Console::GetConsoleWindow() };
    let _listener = (port != 0).then(|| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).expect("bind"));
    println!(
        "NEEDLE_TELEMETRY={} DO_NOT_TRACK={} console window: {}",
        env("NEEDLE_TELEMETRY"),
        env("DO_NOT_TRACK"),
        if console.0.is_null() { "none" } else { "one" }
    );
    let _ = std::io::stdout().flush();
    std::thread::sleep(Duration::from_secs(120));
}

fn parent() {
    // 1. A child on a free port: its environment, its console, and that it is the one listening.
    let port = proc::free_port().unwrap_or_else(|e| fail(&e.to_string()));
    let mut a = proc::spawn(me().args(["child", &port.to_string()]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()))
        .unwrap_or_else(|e| fail(&e.to_string()));
    let mut said = String::new();
    let _ = BufReader::new(a.0.stdout.take().expect("pipe")).read_line(&mut said);
    println!("child A (pid {}): {}", a.pid(), said.trim());
    if !said.contains("NEEDLE_TELEMETRY=0 DO_NOT_TRACK=1 console window: none") {
        fail("the child's environment or console");
    }
    match proc::wait_listening(&mut a, port, Duration::from_secs(5)) {
        Ok(()) => println!("port {port}: listener is child A on 127.0.0.1 ({:?})", proc::listeners(port).unwrap_or_default()),
        Err(e) => fail(&e.to_string()),
    }

    // 2. A port somebody else listens on is refused.
    let mine = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
    let taken = mine.local_addr().expect("address").port();
    match proc::wait_listening(&mut a, taken, Duration::from_millis(300)) {
        Err(e) => println!("port {taken} (held by this probe): refused, \"{e}\""),
        Ok(()) => fail("a foreign listener was accepted"),
    }

    // 3. Voice off: every child ends at once.
    let pid = a.pid();
    proc::kill_all();
    match gone_within(pid, Duration::from_secs(2)) {
        Some(t) => println!("kill_all: child A gone after {} ms, alive() = {}", t.as_millis(), a.alive()),
        None => fail("kill_all left child A running"),
    }

    // 4. A second child (the job still works), left running: no destructor runs in `exit`.
    let b = proc::spawn(me().args(["child", "0"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()))
        .unwrap_or_else(|e| fail(&e.to_string()));
    println!("B {}", b.pid());
    let _ = std::io::stdout().flush();
    std::thread::sleep(Duration::from_millis(400));
    std::process::exit(0);
}

fn watcher() {
    // The wire of the speech server, there and back.
    let header = speak_proto::Header { id: 7, rate: speak_proto::RATE, samples: 0, last: true, error: None };
    let pcm = [0.0f32, 0.5, -1.0];
    let mut wire = Vec::new();
    speak_proto::write_frame(&mut wire, &header, &pcm).expect("write");
    let (back, heard) = speak_proto::read_frame(&mut wire.as_slice()).expect("read");
    if back.id != 7 || back.samples != 3 || heard != pcm {
        fail("speak_proto round trip");
    }
    println!("speak_proto: a frame of {} bytes read back whole", wire.len());

    let mut parent = me().arg("parent").stdin(Stdio::null()).stdout(Stdio::piped()).spawn().expect("start the parent");
    let mut b = 0u32;
    for line in BufReader::new(parent.stdout.take().expect("pipe")).lines().map_while(Result::ok) {
        if let Some(pid) = line.strip_prefix("B ") {
            b = pid.trim().parse().unwrap_or(0);
            println!("child B (pid {b}) running while its parent lives: {}", running(b));
            if !running(b) {
                fail("child B did not start");
            }
            // Not to the end of the pipe: B inherited a copy of it, and the wait must not depend on B.
            break;
        } else {
            println!("{line}");
            if line.starts_with("FAIL") {
                std::process::exit(1);
            }
        }
    }
    let status = parent.wait().expect("wait");
    if b == 0 || !status.success() {
        fail("the parent did not finish its checks");
    }
    match gone_within(b, Duration::from_secs(3)) {
        Some(t) => println!("parent ended without ending child B: B gone {} ms later (job, kill on close)", t.as_millis()),
        None => fail("child B outlived its parent"),
    }
    println!("ok");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("child") => child(args.get(1).and_then(|p| p.parse().ok()).unwrap_or(0)),
        Some("parent") => parent(),
        _ => watcher(),
    }
}

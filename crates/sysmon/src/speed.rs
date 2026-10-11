// The internet speed test. It runs only on the user's click, after the page
// has named the service it talks to, and it sends nothing of the user's: the
// upload is zeros.
//
// Behaviour after the upstream file
// Sources/Vorssaint/Services/Metrics/SpeedTest.swift: the best of five round
// trips, then five seconds of downloading, then five of uploading.

use std::time::{Duration, Instant};

use serde::Serialize;

/// The one service this contacts.
pub const HOST: &str = "speed.cloudflare.com";
/// Each direction is measured for this long.
const BOX: Duration = Duration::from_secs(5);
/// What one download request asks for (the service answers nothing at 100 MB).
const CHUNK: u64 = 90_000_000;
/// The largest single upload; zeros, so it costs the program file nothing.
static ZEROS: [u8; 16 << 20] = [0; 16 << 20];
/// Who asks: the service turns away a request that does not say.
const AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Default, Debug, Serialize)]
pub struct Progress {
    /// "latency" | "download" | "upload" | "done" | "failed"
    pub phase: &'static str,
    /// Best round trip, milliseconds.
    pub ms: Option<f64>,
    /// Megabits per second (decimal), so far or final.
    pub down: Option<f64>,
    pub up: Option<f64>,
}

fn mbps(bytes: u64, over: Duration) -> f64 {
    bytes as f64 * 8.0 / over.as_secs_f64().max(0.001) / 1e6
}

/// Runs the test once, reporting as it goes; the last report is "done" or
/// "failed". Dropping the future (aborting its task) cancels it.
pub async fn run(report: impl Fn(&Progress)) {
    let mut p = Progress { phase: "latency", ..Default::default() };
    report(&p);
    p.phase = match measure(&mut p, &report).await {
        Ok(()) => "done",
        Err(why) => {
            notch_core::log::line(format!("sysmon: speed test: {why}"));
            "failed"
        }
    };
    report(&p);
}

async fn measure(p: &mut Progress, report: &impl Fn(&Progress)) -> Result<(), String> {
    let base = notch_core::net::fetchable(&format!("https://{HOST}/"))?;
    let client = notch_core::net::client(&base, Duration::from_secs(20))?;
    let url = |tail: &str| format!("https://{HOST}/{tail}");
    let refused = |status| format!("{HOST} answered {status}");

    let mut best = f64::MAX;
    for _ in 0..5 {
        let began = Instant::now();
        let answer = client.get(url("__down?bytes=0")).header("User-Agent", AGENT).send().await.map_err(|e| e.to_string())?;
        if !answer.status().is_success() {
            return Err(refused(answer.status()));
        }
        let _ = answer.bytes().await;
        best = best.min(began.elapsed().as_secs_f64() * 1000.0);
    }
    p.ms = Some(best);

    p.phase = "download";
    report(p);
    let began = Instant::now();
    let (mut bytes, mut told) = (0u64, Instant::now());
    'download: while began.elapsed() < BOX {
        let mut answer = client.get(url(&format!("__down?bytes={CHUNK}"))).header("User-Agent", AGENT).send().await.map_err(|e| e.to_string())?;
        if !answer.status().is_success() {
            return Err(refused(answer.status()));
        }
        loop {
            match tokio::time::timeout(BOX.saturating_sub(began.elapsed()), answer.chunk()).await {
                Ok(Ok(Some(chunk))) => bytes += chunk.len() as u64,
                Ok(Ok(None)) => break,
                Ok(Err(e)) => return Err(e.to_string()),
                Err(_) => break 'download,
            }
            if told.elapsed() >= Duration::from_millis(250) {
                told = Instant::now();
                p.down = Some(mbps(bytes, began.elapsed()));
                report(p);
            }
        }
    }
    p.down = Some(mbps(bytes, began.elapsed()));

    // One post after another, each sized to take about half a second; a post
    // the time box cuts short is not counted.
    // ponytail: whole posts only, so a slow line is measured in few, coarse steps; a streamed body would count every byte.
    p.phase = "upload";
    report(p);
    let began = Instant::now();
    let (mut sent, mut until, mut size) = (0u64, Duration::ZERO, 256usize << 10);
    while began.elapsed() < BOX {
        let post = client.post(url("__up")).header("User-Agent", AGENT).body(&ZEROS[..size]).send();
        let posted = Instant::now();
        let Ok(answer) = tokio::time::timeout(BOX.saturating_sub(began.elapsed()), post).await else { break };
        let answer = answer.map_err(|e| e.to_string())?;
        if !answer.status().is_success() {
            return Err(refused(answer.status()));
        }
        sent += size as u64;
        until = began.elapsed();
        let rate = size as f64 / posted.elapsed().as_secs_f64().max(0.001);
        size = ((rate * 0.5) as usize).clamp(256 << 10, ZEROS.len());
        p.up = Some(mbps(sent, until));
        report(p);
    }
    if sent == 0 {
        return Err("no upload finished in time".into());
    }
    p.up = Some(mbps(sent, until));
    Ok(())
}

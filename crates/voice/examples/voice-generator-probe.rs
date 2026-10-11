// Proves crates/voice/src/generator.rs without the app, from the files the spike left (no download of the
// model), plus the download path against a loopback server of its own:
//
//   CARGO_BUILD_JOBS=4 cargo run -p notch-feat-voice --example voice-generator-probe
//
// Loads the 4B model three times and runs two short completions: nothing else of the machine is used.

use notch_feat_voice::{generator, proc};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ENGINE_DIR: &str = "target/brain-spike/llama";
const ZIP: &str = "target/brain-spike/dl/llama-b11552-bin-win-cpu-x64.zip";
const MODEL: &str = "target/brain-spike/models/Qwen3.5-4B-Q4_K_M.gguf";
const ZIP_SHA: &str = "c00741179811ff903b515eae28bdeef1f47af6e81c49920c4e68b62954e056d6";

fn dev_dir() -> PathBuf {
    PathBuf::from("target/voice-dev/voice-generator")
}

fn setup() {
    std::fs::create_dir_all(dev_dir()).unwrap();
    generator::init(dev_dir(), Arc::new(|e, p| println!("  event {e} {p}")));
    generator::use_files(PathBuf::from(ENGINE_DIR), PathBuf::from(MODEL));
    generator::mark_engine(&dev_dir()).unwrap();
}

fn gone(pid: u32) -> bool {
    !notch_core::win::process_table().contains_key(&pid)
}

fn wait_gone(pid: u32, limit: Duration) -> Option<Duration> {
    let from = Instant::now();
    while from.elapsed() < limit {
        if gone(pid) {
            return Some(from.elapsed());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// Streams one completion: time to the first token and tokens per second after it.
fn complete(url: &str, question: &str) {
    let body = serde_json::json!({
        "messages": [{"role":"system","content":"Answer in one short sentence."},{"role":"user","content":question}],
        "stream": true, "max_tokens": 32, "temperature": 0,
    });
    let client = reqwest::blocking::Client::builder().no_proxy().timeout(Duration::from_secs(300)).build().unwrap();
    let from = Instant::now();
    let resp = client.post(format!("{url}/v1/chat/completions")).json(&body).send().unwrap();
    let (mut first, mut n, mut chars) = (None, 0u32, 0usize);
    for line in BufReader::new(resp).lines() {
        let line = line.unwrap();
        let Some(data) = line.strip_prefix("data: ") else { continue };
        if data == "[DONE]" {
            break;
        }
        let v: serde_json::Value = serde_json::from_str(data).unwrap();
        if let Some(t) = v["choices"][0]["delta"]["content"].as_str().filter(|t| !t.is_empty()) {
            first.get_or_insert(from.elapsed());
            n += 1;
            chars += t.len();
        }
    }
    let total = from.elapsed();
    let first = first.expect("no token");
    let rate = n.saturating_sub(1) as f64 / (total - first).as_secs_f64().max(0.001);
    println!(
        "  completion: first token {:.2} s, {n} chunks ({chars} chars), {rate:.1} tokens/s after the first, total {:.2} s",
        first.as_secs_f64(),
        total.as_secs_f64()
    );
}

fn memory(pid: u32) {
    let script = format!("$p=Get-Process -Id {pid}; '{{0}} {{1}}' -f $p.WorkingSet64,$p.PrivateMemorySize64");
    let out = Command::new("powershell").args(["-NoProfile", "-Command", &script]).output().unwrap();
    let t = String::from_utf8_lossy(&out.stdout);
    let mut it = t.split_whitespace().map(|n| n.parse::<f64>().unwrap_or(0.0) / 1048576.0);
    println!("  memory: working set {:.0} MB, private {:.0} MB", it.next().unwrap_or(0.0), it.next().unwrap_or(0.0));
}

fn parent_mode() {
    setup();
    let url = generator::endpoint().expect("endpoint");
    let d = generator::diagnostics().expect("diag");
    println!("PID {} {url}", d.pid);
    std::io::stdout().flush().unwrap();
    // Leaves without unloading: the job must end the server.
    std::process::exit(0);
}

/// A loopback server of our own: 1 MiB of a pattern, with Range.
fn serve() -> (u16, Vec<u8>) {
    let body: Vec<u8> = (0..1_048_576u32).map(|i| (i * 7 % 251) as u8).collect();
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let b = body.clone();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let mut head = String::new();
            let mut r = BufReader::new(s.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                head.push_str(&line.to_lowercase());
            }
            let from = head.split("range: bytes=").nth(1).and_then(|x| x.split('-').next()).and_then(|n| n.parse::<usize>().ok());
            let (status, slice) = match from {
                Some(n) => ("206 Partial Content", &b[n..]),
                None => ("200 OK", &b[..]),
            };
            let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len());
            let _ = s.write_all(slice);
        }
    });
    (port, body)
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn download_tests() {
    println!("download path (loopback server):");
    let dir = dev_dir().join("dl");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (port, body) = serve();
    let url = format!("http://127.0.0.1:{port}/ok.bin");
    let good = sha(&body);
    // a checksum mismatch is refused: no file, no part
    let dest = dir.join("bad.bin");
    let r = generator::fetch(&url, &dest, &"0".repeat(64), &mut |_, _| {});
    println!("  wrong checksum: {:?}; dest exists: {}; part exists: {}", r.as_ref().err().map(|e| e.to_string()), dest.exists(), dir.join("bad.bin.part").exists());
    assert!(r.is_err() && !dest.exists() && !dir.join("bad.bin.part").exists());
    // right checksum, with progress
    let dest = dir.join("ok.bin");
    let mut last = (0, 0);
    generator::fetch(&url, &dest, &good, &mut |r, t| last = (r, t)).unwrap();
    println!("  right checksum: kept {} bytes, last progress {:?}", std::fs::metadata(&dest).unwrap().len(), last);
    assert_eq!(generator::hash_file(&dest).unwrap(), good);
    // resume from a part
    let dest = dir.join("resumed.bin");
    std::fs::write(dir.join("resumed.bin.part"), &body[..300_000]).unwrap();
    generator::fetch(&url, &dest, &good, &mut |_, _| {}).unwrap();
    println!("  resumed from 300000 bytes: {} bytes, hash ok", std::fs::metadata(&dest).unwrap().len());
    // a corrupt part resumed gives a mismatch and is deleted
    let dest = dir.join("corrupt.bin");
    std::fs::write(dir.join("corrupt.bin.part"), vec![1u8; 1000]).unwrap();
    let r = generator::fetch(&url, &dest, &good, &mut |_, _| {});
    println!("  corrupt part: {:?}; part left: {}", r.err().map(|e| e.to_string()), dir.join("corrupt.bin.part").exists());
    assert!(!dest.exists() && !dir.join("corrupt.bin.part").exists());
    // plain http beyond this machine is refused before any connection
    let r = generator::fetch("http://example.com/x", &dir.join("x"), &good, &mut |_, _| {});
    println!("  plain http elsewhere: {:?}", r.err().map(|e| e.to_string()));
    // unzip with the system tar
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("llama-server.exe"), b"x").unwrap();
    let zip = dir.join("t.zip");
    let tar = std::env::var("SystemRoot").unwrap() + "\\System32\\tar.exe";
    Command::new(tar).args(["-a", "-cf"]).arg(&zip).arg("-C").arg(&src).arg("llama-server.exe").status().unwrap();
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    generator::unzip(&zip, &out).unwrap();
    println!("  unzip: llama-server.exe extracted: {}", out.join("llama-server.exe").is_file());
    assert!(out.join("llama-server.exe").is_file());
}

fn main() {
    if std::env::args().any(|a| a == "--parent") {
        return parent_mode();
    }
    setup();
    download_tests();

    println!("files on this machine:");
    let t = Instant::now();
    let zip = generator::hash_file(Path::new(ZIP)).unwrap();
    println!("  engine zip sha256 matches the table: {} ({:.1} s)", zip == ZIP_SHA, t.elapsed().as_secs_f64());
    assert_eq!(zip, ZIP_SHA);
    println!("  ready(): {}", generator::ready());

    println!("start:");
    let t = Instant::now();
    let url = generator::endpoint().expect("endpoint");
    println!("  endpoint {url} after {:.1} s (model hashed against the table, server loaded, /health ok)", t.elapsed().as_secs_f64());
    let t = Instant::now();
    generator::hash_file(Path::new(MODEL)).unwrap();
    println!("  (hashing the model alone, debug build: {:.1} s)", t.elapsed().as_secs_f64());
    let d = generator::diagnostics().unwrap();
    let on = proc::listeners(d.port).unwrap();
    println!("  listener on port {}: {:?}, server pid {}", d.port, on, d.pid);
    assert!(on.iter().all(|(a, p)| a.is_loopback() && *p == d.pid));
    println!(
        "  priority class {:#x} (below normal is 0x4000); affinity {:#x}; efficiency cores reported by CPU sets: {}",
        d.priority,
        d.affinity,
        d.efficiency.map_or("none".into(), |m| format!("{m:#x}"))
    );
    assert_eq!(d.priority, 0x4000);
    if let Some(m) = d.efficiency {
        assert_eq!(d.affinity, m);
    }
    memory(d.pid);

    println!("two short completions:");
    complete(&url, "What is the capital of France?");
    complete(&url, "Name a primary colour.");
    memory(d.pid);

    println!("pause path (kill_all), then a new start:");
    proc::kill_all();
    let took = wait_gone(d.pid, Duration::from_secs(10));
    println!("  old pid gone after {took:?}; running() after kill_all: {}", generator::running());
    assert!(took.is_some() && !generator::running());
    let t = Instant::now();
    let url = generator::endpoint().expect("restart");
    let d2 = generator::diagnostics().unwrap();
    println!("  restarted after {:.1} s on {url}, new pid {}", t.elapsed().as_secs_f64(), d2.pid);

    println!("idle unload (idle shortened to 8 s by the probe):");
    generator::set_idle(Duration::from_secs(8));
    generator::touch();
    let t = Instant::now();
    let took = wait_gone(d2.pid, Duration::from_secs(40));
    println!("  server process gone: {} after {:.1} s; running(): {}", took.is_some(), t.elapsed().as_secs_f64(), generator::running());
    assert!(took.is_some() && !generator::running());

    println!("dies with its parent:");
    let mut me = Command::new(std::env::current_exe().unwrap());
    me.arg("--parent").stdout(Stdio::piped());
    let mut child = me.spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let pid: u32 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let _ = child.wait();
    let ended = wait_gone(pid, Duration::from_secs(10));
    println!("  parent exited without unloading; server pid {pid} gone after {ended:?}");
    assert!(ended.is_some());
    println!("ok");
}

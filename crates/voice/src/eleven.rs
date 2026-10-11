// The ElevenLabs voice: optional, the user's own key (`voice/elevenlabs-key` in Credential Manager,
// through `notch_core::secrets`), off until the setting `eleven` is on. Filled by package voice-online
// (VOICE §10; read §7.1, §8). Each sentence leaves the PC, so nothing here runs without the switch; a
// sentence that fails or is slow returns `Err` before any audio, and `mouth` then speaks it with the
// Windows voice. The key goes in one request header and is never logged.

use crate::{AudioChunk, AudioSink, Engine, Result, SynthRequest, VoiceDesc, VoiceError};
use notch_core::{log, net, secrets, Ctx};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const KEY: &str = "voice/elevenlabs-key";
const BASE: &str = "https://api.elevenlabs.io";
/// A stock voice for when the setting `elevenVoice` is empty.
const DEFAULT_VOICE: &str = "21m00Tcm4TlvDq8ikWAM";
/// The whole of one sentence, audio included; past it the sentence goes to the main voice.
const SENTENCE: Duration = Duration::from_secs(6);

static ON: AtomicBool = AtomicBool::new(false);
static VOICES: Mutex<Vec<VoiceDesc>> = Mutex::new(Vec::new());
/// A probe's stand-in: (address, key, time limit). Never set by the app.
static STAND_IN: Mutex<Option<(String, String, Duration)>> = Mutex::new(None);

/// For `voice-eleven-probe`: talk to this loopback address with this key instead of the service.
pub fn use_stand_in(base: &str, key: &str, limit: Duration) {
    *STAND_IN.lock().unwrap() = Some((base.into(), key.into(), limit));
}

/// For `voice-eleven-probe`: the setting `eleven`.
pub fn set_on(on: bool) {
    ON.store(on, Ordering::SeqCst);
}

fn target() -> (String, Option<String>, Duration) {
    match STAND_IN.lock().unwrap().clone() {
        Some((base, key, limit)) => (base, Some(key), limit),
        None => (BASE.into(), secrets::get(KEY), SENTENCE),
    }
}

fn read_on(v: &Value) {
    ON.store(v["eleven"].as_bool().unwrap_or(false), Ordering::SeqCst);
}

pub fn start(cx: &Ctx) -> Result<()> {
    read_on(&cx.settings(crate::ID));
    Ok(())
}

pub fn stop(_cx: &Ctx) {
    ON.store(false, Ordering::SeqCst);
    VOICES.lock().unwrap().clear();
}

pub fn settings(_cx: &Ctx, settings: &Value) {
    read_on(settings);
    if !ON.load(Ordering::SeqCst) {
        VOICES.lock().unwrap().clear();
    }
}

/// `status` → `{on: bool, key: bool}` · `voices` → `[VoiceDesc]` (asks the account; kept for the session).
pub fn call(_cx: &Ctx, method: &str, _args: Value) -> Result<Value> {
    match method {
        "status" => Ok(json!({ "on": ON.load(Ordering::SeqCst), "key": target().1.is_some() })),
        "voices" => {
            refresh()?;
            Ok(json!(voices()))
        }
        _ => Err(VoiceError::Failed("unknown method".into())),
    }
}

/// The switch is on and a key is stored.
pub fn ready() -> bool {
    ON.load(Ordering::SeqCst) && target().1.is_some()
}

/// The account's voices as last fetched, ids `eleven:<voice id>`; no request is made here.
pub fn voices() -> Vec<VoiceDesc> {
    if ready() {
        VOICES.lock().unwrap().clone()
    } else {
        Vec::new()
    }
}

/// A client that sends the key only over https or to this computer, and follows no redirect.
fn client(base: &str, limit: Duration) -> Result<(reqwest::blocking::Client, reqwest::Url)> {
    let url = reqwest::Url::parse(base).map_err(|_| VoiceError::Failed("ElevenLabs address".into()))?;
    if !net::key_may_go_to(&url) {
        return Err(VoiceError::Refused("a key is only sent over https".into()));
    }
    let mut b = reqwest::blocking::Client::builder().timeout(limit).redirect(reqwest::redirect::Policy::none());
    if net::is_loopback_url(&url) {
        b = b.no_proxy();
    }
    Ok((b.build().map_err(|_| VoiceError::Failed("ElevenLabs client".into()))?, url))
}

fn refused() -> VoiceError {
    VoiceError::Refused("ElevenLabs is off or has no key".into())
}

/// Asks the account for its voices (kept for the session). `Err(Refused)` while off.
pub fn refresh() -> Result<()> {
    if !ON.load(Ordering::SeqCst) {
        return Err(refused());
    }
    let (base, key, limit) = target();
    let key = key.ok_or_else(refused)?;
    let (http, url) = client(&base, limit)?;
    let body: Value = http
        .get(url.join("v1/voices").map_err(|_| VoiceError::Failed("ElevenLabs address".into()))?)
        .header("xi-api-key", key)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| VoiceError::Failed(format!("ElevenLabs voices: {}", e.without_url())))?;
    let list = body["voices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let id = v["voice_id"].as_str()?;
            Some(VoiceDesc { id: format!("eleven:{id}"), name: v["name"].as_str().unwrap_or(id).into(), engine: Engine::Eleven, lang: "en-US".into() })
        })
        .collect();
    *VOICES.lock().unwrap() = list;
    Ok(())
}

/// One chunk from the service, as PCM pieces to `sink` while they arrive. `Err(Refused)` when not `ready`.
/// An error before the first piece means nothing was played; after it the sentence is cut short and
/// counts as done (the main voice would say it twice).
pub fn synth(request: &SynthRequest, sink: AudioSink) -> Result<()> {
    use std::io::Read;
    if !ON.load(Ordering::SeqCst) {
        return Err(refused());
    }
    let (base, key, limit) = target();
    let key = key.ok_or_else(refused)?;
    let (http, url) = client(&base, limit)?;
    let voice = if request.voice.is_empty() { DEFAULT_VOICE } else { request.voice.as_str() };
    let voice = voice.strip_prefix("eleven:").unwrap_or(voice);
    if !voice.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(VoiceError::Failed("ElevenLabs voice id".into()));
    }
    let to = url
        .join(&format!("v1/text-to-speech/{voice}?output_format=pcm_24000"))
        .map_err(|_| VoiceError::Failed("ElevenLabs address".into()))?;
    let mut resp = http
        .post(to)
        .header("xi-api-key", key)
        .json(&json!({ "text": request.text, "model_id": "eleven_flash_v2_5" }))
        .send()
        .map_err(|e| VoiceError::Failed(format!("ElevenLabs: {}", e.without_url())))?;
    if !resp.status().is_success() {
        log::line(format!("voice: ElevenLabs answered {}", resp.status().as_u16()));
        return Err(VoiceError::Failed(format!("ElevenLabs answered {}", resp.status().as_u16())));
    }
    // 16-bit little-endian mono at 24 kHz; a piece is 100 ms.
    let (mut buf, mut odd, mut pieces) = ([0u8; 4800], None::<u8>, 0usize);
    loop {
        let n = match resp.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) if pieces > 0 => break,
            Err(_) => return Err(VoiceError::Timeout("ElevenLabs")),
        };
        let mut bytes: Vec<u8> = odd.take().into_iter().chain(buf[..n].iter().copied()).collect();
        if bytes.len() % 2 == 1 {
            odd = bytes.pop();
        }
        let samples = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect::<Vec<_>>();
        if samples.is_empty() {
            continue;
        }
        pieces += 1;
        if !sink(AudioChunk { rate: 24_000, samples }) {
            return Err(VoiceError::Cancelled);
        }
    }
    if pieces == 0 {
        return Err(VoiceError::Failed("ElevenLabs sent no audio".into()));
    }
    Ok(())
}

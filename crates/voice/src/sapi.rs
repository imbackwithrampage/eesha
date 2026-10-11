// The Windows voices: the English voices of `Windows.Media.SpeechSynthesis` (`SpeechSynthesizer::AllVoices`,
// synthesised to a stream and decoded to PCM here, never played by Windows itself). Filled by package
// voice-mouth-win (VOICE §7.1). The voice until Kokoro is downloaded, and the one a failed chunk falls to.

use crate::{AudioChunk, AudioSink, Engine, Result, SynthRequest, VoiceDesc, VoiceError};
use std::sync::Mutex;
use windows::core::HSTRING;
use windows::Media::SpeechSynthesis::{SpeechSynthesizer, VoiceInformation};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

// Installed voices change with a Windows settings visit, not while the app runs.
// debt: listed once per process; a voice added meanwhile shows after a restart.
static LIST: Mutex<Option<Vec<VoiceDesc>>> = Mutex::new(None);

fn fail(e: windows::core::Error) -> VoiceError {
    VoiceError::Failed(format!("Windows voice: 0x{:08X}", e.code().0 as u32))
}

fn init() {
    // MTA, so that `.get()` on a WinRT operation is allowed; an already initialised thread keeps its mode.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
}

fn english() -> Vec<VoiceInformation> {
    init();
    let Ok(all) = SpeechSynthesizer::AllVoices() else { return Vec::new() };
    all.into_iter()
        .filter(|v| v.Language().map(|l| l.to_string().to_lowercase().starts_with("en")).unwrap_or(false))
        .collect()
}

/// At least one English voice is installed.
pub fn ready() -> bool {
    !voices().is_empty()
}

/// Ids are `windows:<display name>`.
pub fn voices() -> Vec<VoiceDesc> {
    let mut list = LIST.lock().unwrap();
    list.get_or_insert_with(|| {
        english()
            .iter()
            .filter_map(|v| {
                let name = v.DisplayName().ok()?.to_string();
                Some(VoiceDesc { id: format!("windows:{name}"), name, engine: Engine::Windows, lang: v.Language().ok()?.to_string() })
            })
            .collect()
    })
    .clone()
}

/// Blocks until the chunk is synthesised (about 0.1 s) and hands it to `sink`, at the voice's own rate.
/// `request.rate` and `request.pitch` are the settings `winRate` and `winPitch`. `request.voice` is the
/// display name; empty or unknown is the first English voice.
pub fn synth(request: &SynthRequest, sink: AudioSink) -> Result<()> {
    let installed = english();
    let voice = installed
        .iter()
        .find(|v| v.DisplayName().map(|n| n.to_string() == request.voice).unwrap_or(false))
        .or(installed.first())
        .ok_or_else(|| VoiceError::Missing("a Windows voice".into()))?;
    let synth = SpeechSynthesizer::new().map_err(fail)?;
    synth.SetVoice(voice).map_err(fail)?;
    let options = synth.Options().map_err(fail)?;
    let _ = options.SetSpeakingRate(request.rate.clamp(0.5, 3.0) as f64);
    let _ = options.SetAudioPitch(request.pitch.clamp(0.0, 2.0) as f64);
    let stream = synth.SynthesizeTextToStreamAsync(&HSTRING::from(request.text.as_str())).map_err(fail)?.get().map_err(fail)?;
    let size = stream.Size().map_err(fail)? as u32;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).map_err(fail)?).map_err(fail)?;
    reader.LoadAsync(size).map_err(fail)?.get().map_err(fail)?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).map_err(fail)?;
    let audio = decode_wav(&bytes).ok_or_else(|| VoiceError::Failed("Windows voice: unreadable audio".into()))?;
    if !sink(audio) {
        return Err(VoiceError::Cancelled);
    }
    Ok(())
}

/// A RIFF/WAVE of 16-bit PCM (or 32-bit float) to mono floats at the file's own rate.
fn decode_wav(b: &[u8]) -> Option<AudioChunk> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return None;
    }
    let (mut rate, mut channels, mut bits, mut tag) = (0u32, 0usize, 0u16, 0u16);
    let mut at = 12;
    while at + 8 <= b.len() {
        let id = &b[at..at + 4];
        let len = u32::from_le_bytes(b[at + 4..at + 8].try_into().ok()?) as usize;
        let body = &b[at + 8..(at + 8 + len).min(b.len())];
        if id == b"fmt " && body.len() >= 16 {
            tag = u16::from_le_bytes([body[0], body[1]]);
            channels = u16::from_le_bytes([body[2], body[3]]) as usize;
            rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" && rate > 0 && channels > 0 {
            let samples: Vec<f32> = match (tag, bits) {
                (1, 16) => body.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect(),
                (3, 32) => body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
                _ => return None,
            };
            let mono = samples.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect();
            return Some(AudioChunk { rate, samples: mono });
        }
        at += 8 + len + (len & 1);
    }
    None
}

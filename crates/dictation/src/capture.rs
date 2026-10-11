// Microphone capture for Whistle: WASAPI shared mode on the default input device, asking Windows itself
// for 16 kHz mono float (AUTOCONVERTPCM), so there is no resampling code. The device is opened by
// `Session::start` and released when the thread ends; audio lives in memory only until it is transcribed.
//
// A pause of about a second after speech cuts a phrase, which goes to the worker thread and is inserted
// while the user keeps talking. There is no time limit: it runs until stopped.

use crate::engine::Emit;
use crate::whistle;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use windows::core::Result as WinResult;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

const RATE: usize = 16_000;
const MAX_PHRASE: usize = 25 * RATE; // keeps one clip within what the engine takes, without a pause
const PAUSE: usize = RATE; // one second of quiet
// debt: a fixed loudness threshold; a noisy room or a quiet voice needs an adaptive one.
const QUIET: f32 = 0.012;

pub struct Capture {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Capture {
    pub fn start(emit: Emit, dir: PathBuf) -> Capture {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let join = std::thread::spawn(move || {
            let (tx, rx) = mpsc::channel::<Vec<f32>>();
            let e2 = emit.clone();
            let worker = std::thread::spawn(move || {
                for clip in rx {
                    match whistle::transcribe(&dir, &clip) {
                        Ok(t) if !t.text.is_empty() => e2("text", json!({ "text": t.text, "language": t.language })),
                        Ok(_) => {}
                        Err(m) => e2("error", json!({ "code": "engine", "privacy": false, "message": m })),
                    }
                }
            });
            if let Err(e) = record(&emit, &flag, &tx) {
                emit("error", json!({ "code": format!("0x{:08X}", e.code().0 as u32), "privacy": false, "message": e.message() }));
            }
            drop(tx); // the microphone is already closed; the worker finishes what is queued
            let _ = worker.join();
            emit("ended", json!({}));
        });
        Capture { stop, join: Some(join) }
    }

    /// Closes the microphone (within about 30 ms) and returns; transcription of the last phrase carries on.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // The capture thread joins its worker, which can take a moment; do not make the caller wait.
        self.join.take();
    }
}

fn record(emit: &Emit, stop: &AtomicBool, tx: &mpsc::Sender<Vec<f32>>) -> WinResult<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(eCapture, eConsole)?;
        let ac: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let fmt = WAVEFORMATEX {
            wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
            nChannels: 1,
            nSamplesPerSec: RATE as u32,
            nAvgBytesPerSec: RATE as u32 * 4,
            nBlockAlign: 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        ac.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, 50_000_000, 0, &fmt, None)?;
        let cap: IAudioCaptureClient = ac.GetService()?;
        ac.Start()?;
        emit("listening", json!({}));

        let (mut phrase, mut quiet, mut spoke) = (Vec::<f32>::new(), 0usize, false);
        let (mut peak, mut ticks) = (0f32, 0u32);
        while !stop.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(30));
            // About ten readings a second, for the level on the strip.
            ticks += 1;
            if ticks % 3 == 0 {
                emit("level", json!({ "level": (peak * 6.0).min(1.0) }));
                peak = 0.0;
            }
            while cap.GetNextPacketSize()? > 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let chunk: Vec<f32> = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    vec![0.0; frames as usize]
                } else {
                    std::slice::from_raw_parts(data as *const f32, frames as usize).to_vec()
                };
                cap.ReleaseBuffer(frames)?;
                let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
                peak = peak.max(rms);
                if rms < QUIET {
                    quiet += chunk.len();
                } else {
                    quiet = 0;
                    spoke = true;
                }
                phrase.extend(chunk);
                if spoke && (quiet >= PAUSE || phrase.len() >= MAX_PHRASE) {
                    let _ = tx.send(std::mem::take(&mut phrase));
                    (quiet, spoke) = (0, false);
                } else if !spoke && phrase.len() > PAUSE {
                    phrase.drain(..phrase.len() - PAUSE / 2); // leading silence is not kept
                }
            }
        }
        let _ = ac.Stop();
        if spoke {
            let _ = tx.send(phrase);
        }
        Ok(())
    }
}

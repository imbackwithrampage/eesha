// Speech to text: one Whistle run per call, through `notch_feat_dictation::whistle` (the engine and the
// model are the dictation feature's files, in its data folder) with the language and keyword options
// that package voice-ears added there (VOICE §3.2).
//
// The clip is a temporary file for the length of the run and nothing of it is logged.

use crate::{timing, AudioChunk, Result, VoiceError};
use notch_feat_dictation::whistle;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const WHAT: &str = "the speech model";

/// The engine is quick up to about this much audio and several times slower past it. Measured on this
/// PC: 0.2 to 0.4 s for anything up to 5.1 s, then 1.1 s for 5.5 s, 2.8 s for 13 s, 3.7 s for 20 s.
/// `ears` cuts the quiet off both ends of what it sends, and keeps partials and the wake's share under it.
// debt: a turn longer than this is transcribed whole; cutting it at its pauses would keep every run quick.
pub use crate::timing::STT_QUICK as QUICK;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SttOptions {
    /// `--audio-language`: `Some("en")` for the wake and for commands.
    pub language: Option<String>,
    /// `--audio-keywords`: the wake phrase and its noun (`wakephrase::keywords`); empty for none.
    pub keywords: Vec<String>,
}

/// Size and time of the engine and the model when they last matched their hashes.
type Stamp = [(u64, Option<SystemTime>); 2];

/// Hashing the two files (19 MB) takes about as long as a third of a run, and a turn runs the engine
/// several times: they are hashed again only when one of them has changed, or after `forget`.
static CHECKED: Mutex<Option<(PathBuf, Stamp)>> = Mutex::new(None);

fn stamp(dir: &Path) -> Option<Stamp> {
    let of = |f: &whistle::File| std::fs::metadata(dir.join(f.name)).ok().map(|m| (m.len(), m.modified().ok()));
    Some([of(&whistle::FILES[0])?, of(&whistle::FILES[1])?])
}

/// Engine and model are present and match their hashes. `dir` is `cx.data_dir("dictation")`.
pub fn ready(dir: &Path) -> bool {
    let Some(now) = stamp(dir) else { return false };
    let mut checked = CHECKED.lock().unwrap();
    if checked.as_ref().is_some_and(|(at, then)| at == dir && *then == now) {
        return true;
    }
    let ok = whistle::ready(dir);
    *checked = ok.then(|| (dir.to_path_buf(), now));
    ok
}

/// The next `ready` hashes the files again: after a download, and when the engine seems deaf (§3.4).
pub fn forget() {
    *CHECKED.lock().unwrap() = None;
}

/// 16 kHz mono in, text out (trimmed, may be empty). Blocks for the run (0.12 to 0.20 s measured).
pub fn transcribe(dir: &Path, audio: &AudioChunk, options: &SttOptions) -> Result<String> {
    if audio.rate != timing::CAPTURE_RATE {
        return Err(VoiceError::Failed(format!("speech to text takes {} Hz audio", timing::CAPTURE_RATE)));
    }
    if !ready(dir) {
        return Err(VoiceError::Missing(WHAT.into()));
    }
    let options = whistle::Options { language: options.language.as_deref(), keywords: &options.keywords };
    match whistle::transcribe_with(dir, &audio.samples, options) {
        Ok(heard) => Ok(heard.text),
        Err(e) if e == whistle::TIMED_OUT => Err(VoiceError::Timeout("speech to text")),
        Err(e) => Err(VoiceError::Failed(e)),
    }
}

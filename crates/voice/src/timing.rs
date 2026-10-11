// The one table of timings (VOICE §3.4, row by row, same order). Tuning touches this file only: no
// module keeps a copy. Changed by the integration pass and by voice-live (the session at a microphone);
// every other package asks for a change in its report.

use std::time::Duration;

const fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// ── Capture (§3.2) ────────────────────────────────────────────────────────────
pub const CAPTURE_RATE: u32 = 16_000;
pub const CAPTURE_POLL: Duration = ms(30);
pub const CAPTURE_RING: Duration = ms(500);

// ── §3.4, the table ───────────────────────────────────────────────────────────
/// Calibration.
pub const CALIBRATION: Duration = ms(500);

/// Segment starts / quiet frame / "voiced": power against the noise floor.
pub const SEGMENT_START_RATIO: f32 = 8.0;
pub const QUIET_RATIO: f32 = 2.0;
pub const VOICED_RATIO: f32 = 5.0;

/// Noise adapts: share of the gap closed per second, idle and in quiet; where it starts.
pub const NOISE_ADAPT_IDLE: f32 = 0.194;
pub const NOISE_ADAPT_QUIET: f32 = 0.042;
/// While calibrating: the floor follows the room quickly (voice-ears).
pub const NOISE_ADAPT_CALIBRATING: f32 = 0.999;
/// Whistle is quick only up to this much audio (0.3 s; 1.1 s at 5.5 s; 3.7 s at 20 s): ears trims the quiet to stay under it.
pub const STT_QUICK: Duration = ms(5000);
pub const NOISE_START: f32 = 1e-7;

/// Segment ends: quiet for this long; shorter while waiting for the wake.
pub const SEGMENT_END: Duration = ms(800);
pub const SEGMENT_END_WAITING: Duration = ms(500);

/// Forced end; the noise floor then becomes the segment's mean.
pub const FORCED_END: Duration = ms(30_000);

/// Wake window in long speech: first at, then every, on the last.
pub const WAKE_WINDOW_FIRST: Duration = ms(1200);
pub const WAKE_WINDOW_EVERY: Duration = ms(2000);
pub const WAKE_WINDOW_SPAN: Duration = ms(3500);

/// Worth transcribing: voiced for at least this.
pub const WORTH_TRANSCRIBING: Duration = ms(300);

/// First word after a wake or tap.
pub const FIRST_WORD: Duration = ms(5000);

/// First word of a reply; for yes or no; awaiting a dropped file.
pub const FIRST_WORD_REPLY: Duration = ms(8000);
pub const FIRST_WORD_YES_NO: Duration = ms(5000);
pub const FIRST_WORD_FILE: Duration = ms(20_000);

/// Provisional transcript: after this much quiet.
pub const PROVISIONAL_AFTER: Duration = ms(600);

/// Silence ending a turn; after an extending word; for a free-text answer.
pub const TURN_END: Duration = ms(1600);
pub const TURN_END_EXTENDED: Duration = ms(2600);
pub const TURN_END_FREE_TEXT: Duration = ms(3200);

/// Extra patience: voiced in the last … → look again in ….
pub const PATIENCE_WINDOW: Duration = ms(500);
pub const PATIENCE_AGAIN: Duration = ms(500);

/// Hold to talk: held over … is a hold; it ends … after release.
pub const HOLD_AFTER: Duration = ms(400);
pub const HOLD_RELEASE_TAIL: Duration = ms(200);

/// Turn limit.
pub const TURN_LIMIT: Duration = ms(20_000);

/// Follow-up microphone: this long after speech ends.
pub const FOLLOW_UP_MIC: Duration = ms(150);

/// Conversation memory / failures in a row ending it.
pub const CONVERSATION_MEMORY: Duration = ms(90_000);
pub const CONVERSATION_FAILURES: u32 = 2;

/// Stall / rebuild: stop, pause, start; at least … apart, at most … a minute, then a banner.
pub const STALL: Duration = ms(2000);
pub const REBUILD_PAUSE: Duration = ms(500);
pub const REBUILD_APART: Duration = ms(5000);
pub const REBUILDS_PER_MINUTE: u32 = 5;

/// Deaf engine: this much voiced and empty text, this many times (more with media playing).
pub const DEAF_VOICED: Duration = ms(3000);
pub const DEAF_RUNS: u32 = 2;
pub const DEAF_RUNS_MEDIA: u32 = 6;

/// Needle timeout.
pub const NEEDLE_TIMEOUT: Duration = ms(2000);

/// Text model: first token local and online; tokens; the whole answer.
pub const TEXT_FIRST_TOKEN_LOCAL: Duration = ms(12_000);
pub const TEXT_FIRST_TOKEN_ONLINE: Duration = ms(6000);
pub const TEXT_MAX_TOKENS: u32 = 96;
pub const TEXT_TOTAL: Duration = ms(30_000);

/// Captions stay.
pub const CAPTIONS_STAY: Duration = ms(2000);

// ── Elsewhere in the design, shared by more than one module ───────────────────
/// `voice:level`, per second, during a turn (§3.5).
pub const LEVEL_HZ: u32 = 20;
/// A partial transcript about this often while a command is spoken (§3.2).
pub const PARTIAL_EVERY: Duration = ms(1000);
/// Children leave after this long unused (§3.3); the settings `keepVoice` and `keepModel` replace the last two.
pub const NEEDLE_IDLE: Duration = ms(3 * 60_000);
pub const SPEECH_IDLE: Duration = ms(5 * 60_000);
pub const GENERATOR_IDLE: Duration = ms(10 * 60_000);
/// Needle is ready this long after its start (§4.2); an engine that takes longer than `CHILD_START` is given up.
pub const NEEDLE_READY: Duration = ms(1600);
pub const CHILD_START: Duration = ms(20_000);
/// Player (§7.3, §7.4): between chunks; the fade of a stop; chunks synthesised ahead; fixed replies kept.
pub const PLAYER_GAP: Duration = ms(150);
pub const PLAYER_FADE: Duration = ms(30);
pub const SYNTH_AHEAD: usize = 2;
pub const PHRASE_CACHE: usize = 64;
/// Esc is looked at this often during a turn (§7.4).
pub const ESC_POLL: Duration = ms(30);

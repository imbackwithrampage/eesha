// When a turn ends: a port of upstream's `Voice/TurnEndPolicy.swift` and of the silence timer of
// `Voice/VoiceEngine.swift` (`resetSilenceTimer`, `scheduleSilenceCheck`), with the numbers of
// `crate::timing` (VOICE §3.4). Package voice-ears. Pure: it is told what the detection saw and how much
// time passed, so a probe drives it without a microphone.
//
// Upstream's recogniser writes words while they are spoken, and its timer starts again at each new
// word. Whistle writes only when it is run, so here the words come from transcripts (`extend`): the
// silence is counted from the last sound a transcript found a new word in. Sound that no transcript
// has covered yet is never taken for silence: `Provisional` asks for its transcript after
// `PROVISIONAL_AFTER` of quiet, and the turn does not end before `extend` has answered. A click or a
// cough gets its transcript too, adds no word, and so does not start the silence again.

use crate::timing::*;
use crate::ListenKind;
use serde::Serialize;
use std::time::Duration;

/// A last word that says the sentence is not over (upstream's `extendingWords`, the English ones):
/// linking words, hesitations, articles and prepositions, verbs still without their object.
const EXTENDING: &[&str] = &[
    "and", "then", "but", "or", "um", "uh", "uhm", "erm", "er", "like", "well", "wait", "a", "the", "to", "on", "of", "in", "at", "with", "for", "add", "remove",
    "disable", "enable", "play", "start", "increase", "decrease", "set",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TurnEnd {
    /// The silence that ends a turn (1.6, 2.6 or 3.2 s).
    Silence,
    /// No first word within its time.
    NoSpeech,
    /// `TURN_LIMIT`.
    Limit,
    /// The held key was released, or the key was tapped again (`HOLD_RELEASE_TAIL` later).
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStep {
    Wait,
    /// `PROVISIONAL_AFTER` of quiet: transcribe what there is and hand it to `extend`.
    Provisional,
    End(TurnEnd),
}

pub struct Turn {
    kind: ListenKind,
    held: bool,
    /// Since the key said the turn is over.
    released: Option<Duration>,
    age: Duration,
    /// Since the last voiced poll.
    quiet: Duration,
    /// Since the last word: the quiet there was when a transcript last gained one.
    silent: Duration,
    /// In the longest transcript so far, and whether the newest ends on an extending word.
    words: usize,
    extending: bool,
    /// Sound no transcript has covered yet, and its transcript has been asked for.
    unheard: bool,
    asked: bool,
    /// Extra patience: nothing is decided until this has run out.
    again: Duration,
}

impl Turn {
    /// `held`: the talk key is held down (hold to talk): only release or the limit ends it.
    pub fn new(kind: ListenKind, held: bool) -> Turn {
        let zero = Duration::ZERO;
        Turn { kind, held, released: None, age: zero, quiet: zero, silent: zero, words: 0, extending: false, unheard: false, asked: false, again: zero }
    }

    /// A turn that goes on with what was already heard (the wake phrase and maybe more), `quiet` ago.
    pub fn heard(mut self, quiet: Duration) -> Turn {
        (self.unheard, self.quiet) = (true, quiet);
        self
    }

    /// One capture poll: was it voiced, and how long it was.
    pub fn step(&mut self, voiced: bool, dt: Duration) -> TurnStep {
        self.age += dt;
        self.silent += dt;
        if voiced {
            (self.quiet, self.unheard, self.asked) = (Duration::ZERO, true, false);
        } else {
            self.quiet += dt;
        }
        if let Some(up) = &mut self.released {
            *up += dt;
            return if *up >= HOLD_RELEASE_TAIL { TurnStep::End(TurnEnd::Released) } else { TurnStep::Wait };
        }
        if self.age >= self.limit() {
            return TurnStep::End(TurnEnd::Limit);
        }
        if self.held {
            return TurnStep::Wait;
        }
        if self.unheard && !self.asked && self.quiet >= PROVISIONAL_AFTER {
            self.asked = true;
            return TurnStep::Provisional;
        }
        self.again = self.again.saturating_sub(dt);
        let due = if self.words == 0 { self.age >= self.first_word() } else { self.silent >= self.silence() };
        if !due || !self.again.is_zero() {
            return TurnStep::Wait;
        }
        if self.quiet < PATIENCE_WINDOW {
            // Sound just now, an "uhh" that is no word: look again in a moment.
            self.again = PATIENCE_AGAIN;
            return TurnStep::Wait;
        }
        if self.unheard {
            // Its provisional transcript is on the way.
            return TurnStep::Wait;
        }
        TurnStep::End(if self.words == 0 { TurnEnd::NoSpeech } else { TurnEnd::Silence })
    }

    /// A transcript, provisional or partial: a new word starts the silence again, and an extending last
    /// word ("and", "to"…) makes it longer. It answers `Provisional` when it covers all that was heard.
    pub fn extend(&mut self, transcript: &str) {
        let words: Vec<String> = transcript.split_whitespace().map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()).filter(|w| !w.is_empty()).collect();
        if words.len() > self.words {
            (self.words, self.silent) = (words.len(), self.quiet);
        }
        self.extending = words.last().is_some_and(|last| EXTENDING.contains(&last.as_str()));
        if self.asked {
            (self.asked, self.unheard) = (false, false);
        }
    }

    /// The key says the turn is over: the held key went up, or it was tapped again.
    pub fn release(&mut self) {
        self.released.get_or_insert(Duration::ZERO);
    }

    /// The silence that would end the turn now: 1.6 s, 2.6 s after an extending word, 3.2 s for free text.
    pub fn silence(&self) -> Duration {
        match self.kind {
            ListenKind::FreeText => TURN_END_FREE_TEXT,
            _ if self.extending => TURN_END_EXTENDED,
            _ => TURN_END,
        }
    }

    /// How long the first word may take.
    pub fn first_word(&self) -> Duration {
        match self.kind {
            ListenKind::Command => FIRST_WORD,
            ListenKind::Reply | ListenKind::FreeText => FIRST_WORD_REPLY,
            ListenKind::YesNo => FIRST_WORD_YES_NO,
            ListenKind::File => FIRST_WORD_FILE,
        }
    }

    /// `TURN_LIMIT`, and never less than half of it after the first word's time (upstream:
    /// max(20, first word + 10)).
    pub fn limit(&self) -> Duration {
        TURN_LIMIT.max(self.first_word() + TURN_LIMIT / 2)
    }

    /// The row of §3.4 an end came by, for the log.
    pub fn rule(&self, end: TurnEnd) -> Duration {
        match end {
            TurnEnd::Silence => self.silence(),
            TurnEnd::NoSpeech => self.first_word(),
            TurnEnd::Limit => self.limit(),
            TurnEnd::Released => HOLD_RELEASE_TAIL,
        }
    }
}

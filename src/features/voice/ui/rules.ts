// The rules of what a turn shows (docs/VOICE.md §7.5, §7.7) and of the settings page, without the
// DOM: what the strip row says in each state, which face and sound go with it, what the capsule
// holds, where it sits and when it stays away; which key may be the talk key; what a download's row
// shows. tests/voice-ui.test.mjs feeds them made-up turns.

import type { IslandShape, VoiceFace } from "../../../core/feature";
import type { SoundName } from "../../../core/sound";
import type { VoiceState } from "../index";
import type { TurnView } from "../turn";

type T = (key: string) => string;
const plain: T = (key) => key;

// ── The strip row ─────────────────────────────────────────────────────────────

/** The states of a turn. */
const TURN: ReadonlySet<VoiceState> = new Set(["listening", "thinking", "speaking", "asking"]);

/** The microphone is open for the user's words. */
export const micOpen = (state: VoiceState): boolean => state === "listening" || state === "asking";

/** The row has the strip: during a turn, and while it closes when there is a result to read. */
export function rowActive(view: Pick<TurnView, "state" | "result">): boolean {
  return TURN.has(view.state) || (view.state === "closing" && view.result !== "");
}

export interface Row {
  title: string;
  /** Dimmer, after the title: the words heard so far, the question, the sentence being said. */
  detail: string;
  /** The right end: the microphone's meter, the playing bars, or the Yes of a question that waits. */
  right: "meter" | "bars" | "yes" | null;
}

/** What the row says for a view. `t` translates the three fixed titles. */
export function rowFor(view: TurnView, t: T = plain): Row {
  // A yes or no waits: its question, while it is being said and while the microphone waits for the answer.
  if (view.confirm && (view.state === "speaking" || view.state === "asking")) return { title: view.confirm.text, detail: "", right: "yes" };
  switch (view.state) {
    case "listening":
      return { title: t("Listening"), detail: view.heard, right: "meter" };
    case "asking":
      // Until the reply's first words come, `heard` is still the sentence before: the question is shown.
      return { title: t("Listening"), detail: view.final ? (view.answer[view.answer.length - 1] ?? "") : view.heard, right: "meter" };
    case "thinking":
      return { title: t("Thinking"), detail: view.heard, right: null };
    case "speaking":
      return view.result
        ? { title: view.result, detail: "", right: "bars" }
        : { title: t("Speaking"), detail: view.answer[view.spoken] ?? "", right: "bars" };
    default:
      return { title: view.result, detail: "", right: null };
  }
}

/** The face of the Visor for a state (§7.6); null gives the mascot back. */
export function faceFor(state: VoiceState): VoiceFace | null {
  switch (state) {
    case "listening":
      return "listening";
    case "thinking":
      return "thinking";
    case "speaking":
      return "talking";
    case "asking":
      return "asking";
    default:
      return null;
  }
}

/** The level a face follows: the microphone while it listens, what is played while it talks. */
export function levelFor(state: VoiceState, level: { mic: number; out: number }): number {
  return micOpen(state) ? level.mic : state === "speaking" ? level.out : 0;
}

// ── How a turn ended, and the sounds (§7.7) ───────────────────────────────────

/** `think` is played when the answer takes this long. */
export const THINK_MS = 1200;

/** `spoken`: it said something. `shown`: it answered without a word. `miss`: it heard a sentence and had no answer. */
export type Ending = "spoken" | "shown" | "miss" | "nothing";

/**
 * How the turn that is closing ended. `at`: what the view held when the last sentence was taken
 * (its `answer` array is replaced whenever a chunk is written, so the same array means nothing was).
 * `cancelled`: the sentence was "cancel" or "that's all", which is an answer in itself.
 */
export function ending(view: TurnView, at: Pick<TurnView, "answer" | "spoken" | "result"> | null, cancelled = false): Ending {
  if (cancelled || !view.final || !view.heard.trim()) return "nothing";
  if (at && view.answer === at.answer && view.spoken === at.spoken && view.result === at.result) return "miss";
  if (view.spoken >= 0 && view.spoken !== at?.spoken) return "spoken";
  return view.answer.length > 0 || view.result !== "" ? "shown" : "miss";
}

/** The sound of a change of state: `blip` when it listens, `pop` when the turn is taken, `annoyed` on a miss, `wink` at a clean end with nothing said. */
export function soundFor(was: VoiceState, now: VoiceState, end: Ending = "nothing"): SoundName | null {
  if (now === was) return null;
  if (micOpen(now)) return "blip";
  if (now === "thinking" && micOpen(was)) return "pop";
  if (now === "closing") return end === "miss" ? "annoyed" : end === "shown" ? "wink" : null;
  return null;
}

/** The face played once as the turn closes: a wink at a clean end, surprise at a miss. */
export function emoteFor(end: Ending): VoiceFace | null {
  return end === "miss" ? "surprised" : end === "nothing" ? null : "wink";
}

// ── The capsule ───────────────────────────────────────────────────────────────

/** Logical pixels. `chars`: about what one line of the answer holds (with the Undo button beside it). */
export const CAPSULE = { w: 380, line: 16, pad: 8, bare: 64, openW: 480, openH: 240, gap: 6, chars: 46, charsUndo: 36, outMs: 220 };

/**
 * The capsule is on screen: through the turn, and through `closing` (Rust holds that state for the 2 s
 * captions stay) when there is something to read. A turn that heard nothing leaves at once.
 */
export function capsuleShown(view: Pick<TurnView, "state" | "heard" | "answer">): boolean {
  return TURN.has(view.state) || (view.state === "closing" && (view.heard !== "" || view.answer.length > 0));
}

/** What the capsule's page is sent. */
export interface Caption {
  /** The microphone is open: the dot is red and pulses. */
  mic: boolean;
  heard: string;
  /** The answer: `said`, `now` (being said: bright) or `next` (written, not yet said). Folded, it begins at the sentence being said. The online model that wrote it comes last ("· Claude"), as `next`. */
  answer: { text: string; at: "said" | "now" | "next" }[];
  undo: boolean;
  /** Captions are off: the dot and the stop button only. */
  bare: boolean;
  /** Opened to its large size by a click. */
  open: boolean;
  /** One to three lines of text when folded. */
  lines: number;
  /** The window's size, logical pixels. */
  w: number;
  h: number;
}

export function captionFor(view: TurnView, o: { captions: boolean; open: boolean }): Caption {
  // While it speaks the microphone is open too, for the one word "stop" (ears.rs): the dot says so.
  const mic = micOpen(view.state) || view.state === "speaking";
  if (!o.captions) return { mic, heard: "", answer: [], undo: false, bare: true, open: false, lines: 1, w: CAPSULE.bare, h: CAPSULE.pad * 2 + CAPSULE.line };
  // Written while the mouth may still say it, a sentence waits dim for its turn; otherwise it is simply the answer.
  const waits = view.state === "thinking" || view.state === "speaking";
  const all = view.answer.map((text, i) => ({ text, at: i < view.spoken ? ("said" as const) : i === view.spoken || !waits ? ("now" as const) : ("next" as const) }));
  const answer = o.open ? all : all.slice(Math.max(0, view.spoken));
  // The online model that wrote it: dim after the answer, as a sentence that is not said.
  if (view.by && answer.length) answer.push({ text: `· ${view.by}`, at: "next" });
  const undo = view.undo !== null;
  // debt: the lines are counted from the letters, not measured; the page cuts what does not fit.
  const letters = answer.reduce((n, a) => n + a.text.length + 1, 0);
  const lines = Math.max(1, (view.heard ? 1 : 0) + (letters === 0 ? 0 : letters > (undo ? CAPSULE.charsUndo : CAPSULE.chars) ? 2 : 1));
  const [w, h] = o.open ? [CAPSULE.openW, CAPSULE.openH] : [CAPSULE.w, CAPSULE.pad * 2 + CAPSULE.line * lines];
  return { mic, heard: view.heard, answer, undo, bare: false, open: o.open, lines, w, h };
}

/** Where the island's window hangs (`caption.origin`), physical pixels: its top centre, its scale, its display. */
export interface Origin {
  x: number;
  y: number;
  scale: number;
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * The capsule's place on the screen, physical pixels: centred on the island, 6 px under its lower
 * edge (which is 6 px from the top while the island is hidden under a full-screen app). An island
 * dragged so low that there is no room under it has the capsule 6 px above it instead. It never lies
 * on the island, so never on a card that waits; with no room either way it stays away (null).
 */
export function capsuleRect(shape: IslandShape, o: Origin, size: { w: number; h: number }): Rect | null {
  const px = (v: number) => Math.round(v * o.scale);
  const [w, h] = [px(size.w), px(size.h)];
  const under = o.y + px(shape.top + shape.height + CAPSULE.gap);
  const over = o.y + px(shape.top - CAPSULE.gap) - h;
  const y = under + h <= o.bottom ? under : over >= o.top ? over : null;
  if (y === null || w > o.right - o.left) return null;
  return { x: Math.max(o.left, Math.min(o.x - Math.round(w / 2), o.right - w)), y, w, h };
}

/** False: the island left while the captions were only staying, and the capsule leaves with it instead of jumping to the top edge. */
export function capsuleFollows(state: VoiceState, shape: IslandShape): boolean {
  return !(state === "closing" && shape.mode === "hidden");
}

// ── Settings: the talk key ────────────────────────────────────────────────────

/** `KeyboardEvent.code` → the names crates/voice/src/ears.rs takes (`vk_of`). Not the ones that open something on their own (the Windows key, the menu key). */
const TALK_KEYS: Readonly<Record<string, string>> = {
  ControlRight: "RCtrl",
  ShiftRight: "RShift",
  AltRight: "RAlt",
  ScrollLock: "ScrollLock",
  Pause: "Pause",
  Insert: "Insert",
};

const NAMES: Readonly<Record<string, string>> = { RCtrl: "Right Ctrl", RShift: "Right Shift", RAlt: "Right Alt", ScrollLock: "Scroll Lock" };

/** The key as it is written on the page: "Right Ctrl". */
export const talkKeyName = (key: string): string => NAMES[key] ?? key;

export type PickedKey =
  /** Still listening: the key that is down, to show. */
  | { kind: "held"; key: string }
  /** Still listening: why that key cannot be the talk key (English, shown through `t`). */
  | { kind: "refused"; why: string }
  | { kind: "save"; key: string }
  | { kind: "clear" }
  | { kind: "cancel" };

const OTHER_KEY = "Use Right Ctrl, Right Shift, Right Alt, Scroll Lock, Pause, Insert or a function key.";

/**
 * One listening session of the talk key's picker, as the Dictation page's (src/features/dictation/
 * picker.ts): every keydown goes to `down`, every keyup to `up`, until one answers save, clear or
 * cancel. The talk key is one key pressed on its own, taken when it is let go (so no key is down when
 * the hook that reads it comes back). Esc cancels, Backspace removes.
 */
export function talkPicker(): { down(e: { code: string; key: string }): PickedKey; up(): PickedKey } {
  let ready = "";
  return {
    down(e) {
      ready = "";
      if (e.key === "Escape") return { kind: "cancel" };
      if (e.key === "Backspace") return { kind: "clear" };
      // A layout with AltGr: Right Alt is how its third characters are typed.
      if (e.key === "AltGraph") return { kind: "refused", why: "On this keyboard Right Alt types characters. Pick another key." };
      const key = TALK_KEYS[e.code] ?? (/^F(?:[1-9]|1\d|2[0-4])$/.test(e.code) ? e.code : "");
      if (!key) return { kind: "refused", why: OTHER_KEY };
      ready = key;
      return { kind: "held", key };
    },
    up() {
      return ready ? { kind: "save", key: ready } : { kind: "held", key: "" };
    },
  };
}

// ── Settings: a download ──────────────────────────────────────────────────────

/** `voice:download`. `what`: whistle (speech to text), needle (commands), speech (the natural voice), generator (the text model). */
export interface DownloadEvent {
  what: string;
  received: number;
  total: number;
  done: boolean;
  error?: string;
}

/** "371 MB", "2.7 GB": cut, not rounded up, so it never says more than there is. */
export function megabytes(bytes: number): string {
  return bytes >= 1e9 ? `${(Math.floor(bytes / 1e8) / 10).toFixed(1)} GB` : `${Math.floor(bytes / 1e6)} MB`;
}

export type DownloadLook =
  | { kind: "get" }
  /** `text` is empty until the first count arrives. */
  | { kind: "busy"; fraction: number; text: string }
  /** Every byte is here; the files are being checked against their hashes. */
  | { kind: "check" }
  | { kind: "ready" }
  | { kind: "error"; text: string };

/** What a download's row shows, from what its `status` said and the last `voice:download` about it. */
export function downloadLook(status: { installed: boolean; downloading: boolean } | null, last: DownloadEvent | null): DownloadLook {
  if (last && !last.done) {
    if (last.total > 0 && last.received >= last.total) return { kind: "check" };
    // Just asked for: no count yet.
    if (last.total <= 0) return { kind: "busy", fraction: 0, text: "" };
    return { kind: "busy", fraction: last.received / last.total, text: `${megabytes(last.received)} of ${megabytes(last.total)}` };
  }
  if (last?.error && last.error !== "cancelled") return { kind: "error", text: last.error };
  if (status?.installed || (last?.done && !last.error)) return { kind: "ready" };
  return status?.downloading ? { kind: "busy", fraction: 0, text: "" } : { kind: "get" };
}

// The turn loop (docs/VOICE.md §4): what was heard goes, in this order, through the stop word, the
// cancel and end phrases, a question that waits for its reply, the parser (parse/), the action model
// (brain/needle.ts) and the text model (brain/route.ts); what comes out is acted on (runner.ts) and
// spoken. It also keeps what the strip row and the capsule draw (`TurnView`).
// Package voice-runner (VOICE §10). The registry and the two models are handed in (`TurnDeps`), so
// `node --test` runs the loop against a made-up Rust side (tests/voice-runner*.test.mjs).
//
// Who ends what:
// - The talk key and the wake phrase: Rust stops the mouth and says `listening`; whatever this loop
//   was doing is dropped (`takeover`).
// - Esc: Rust ends the turn (`closing`); and where the island's keyboard model hands the page the key,
//   `endTurn`. The heard word "stop": `endTurn`. None of them is ever read as a command.
// - The queue running dry: the loop opens the microphone for a reply when it asked something, else it
//   ends the turn (`ears.stop`): that is what takes the state on from `speaking`.

import type { FeatureCtx, VoiceArg, VoiceArgs, VoiceCx, VoiceEntry } from "../../core/feature";
import type { NeedlePick } from "./brain/needle";
import type { BrainAsk, BrainId, Exchange } from "./brain/route";
import type { ListenKind, VoiceEvents, VoiceSettings, VoiceState } from "./index";
import { isCancel, isEnd, isYes, normalise, parse, readArg, splitCommands, type Conversation, type Parsed } from "./parse/index";
import { act, fresh, hush, quiet, speak, speakStream, startRunner, waiting, type Outcome, type RunnerEnv, type Understood } from "./runner";

/** What the strip row and the capsule draw. One object, replaced on every change (levels come as `voice:level`). */
export interface TurnView {
  state: VoiceState;
  /** What was heard so far; `final` once the turn's words are settled. */
  heard: string;
  final: boolean;
  /** The answer, chunk by chunk as it is written; `spoken` is the chunk being said, −1 before the first. */
  answer: string[];
  spoken: number;
  /** The online model that wrote the answer ("Claude"): shown dim after it, never said; empty for a local one. */
  by: string;
  /** The result for the strip ("Timer · 12 min"); empty when there is none. */
  result: string;
  /** A yes or no waits: the question, and what a click on Yes or on No does. */
  confirm: { text: string; yes(): void; no(): void } | null;
  /** Offered for a few seconds after an action that can be taken back. */
  undo: (() => void) | null;
}

/** What the loop is handed by index.ts (and by a test): the registry, the two models, the mixer, Esc. */
export interface TurnDeps {
  /** The actions of the features that are on (registry.ts). */
  entries(): readonly VoiceEntry[];
  cxOf: RunnerEnv["cxOf"];
  /** The action model's decision about a sentence the parser missed (brain/needle.ts). */
  choose(cx: Pick<FeatureCtx, "call">, text: string, entries: readonly VoiceEntry[], of: (entry: VoiceEntry) => VoiceCx): Promise<NeedlePick>;
  /** Gets the action model ready while the sentence is still being said. */
  prepare?(cx: Pick<FeatureCtx, "call">, text: string, entries: readonly VoiceEntry[], of: (entry: VoiceEntry) => VoiceCx): Promise<void>;
  /** The text model (brain/route.ts). Null: nothing qualifies. */
  answer(cx: FeatureCtx, ask: BrainAsk, on: (delta: string) => void): Promise<{ text: string; brain: BrainId } | null>;
  /** The kept state a question is about (answers/). */
  stateAbout?(text: string): string;
  mixer?: RunnerEnv["mixer"];
  asking?: RunnerEnv["asking"];
  /** Esc, where the island's keyboard model hands it to the page: a `keydown` on `window`. Observed, never taken. */
  onEsc?(fn: () => void): () => void;
  /** A line for the log: timings, never words (§8). */
  log?(line: string): void;
}

/**
 * The rows of §3.4 the page keeps time by (Rust's `timing.rs` is the table: its own clock ends a
 * reply that does not come, this one only stands behind it). `first`: the first word of a reply, by
 * what is waited for. `word`: once words come, the longest a turn's own silence can be.
 */
export const TIMES = {
  first: { command: 5000, reply: 8000, yesNo: 5000, file: 20_000, freeText: 8000 } satisfies Record<ListenKind, number>,
  slack: 1500,
  word: 4500,
  /** The conversation is remembered this long; this many failures in a row end it; a missing detail is asked for this often. */
  memory: 90_000,
  fails: 2,
  asks: 2,
};

const REST: TurnView = { state: "off", heard: "", final: false, answer: [], spoken: -1, by: "", result: "", confirm: null, undo: null };
const SORRY = "Sorry, say that again?";
const NOTHING = "I can't answer that on this PC yet. You can add a text model in Settings, Voice.";
/** The states of a turn: Esc ends one of these. */
const BUSY: ReadonlySet<VoiceState> = new Set(["listening", "thinking", "speaking", "asking"]);
/** Said to stop it talking: never a command. */
const STOP = /^(?:stop|stop it|stop that|stop talking|stop speaking|be quiet|quiet|shut up|hush|enough|thats enough|silence)$/;
const isStop = (text: string): boolean => STOP.test(normalise(text));

let cx: FeatureCtx | null = null;
let deps: TurnDeps | null = null;
let settings: Partial<VoiceSettings> = {};
let view: TurnView = REST;
const watchers = new Set<(view: TurnView) => void>();

/** Raised whenever a turn starts or is dropped: what still runs for an older number stops at its next step. */
let turnNo = 0;
let live = false;
let ctl: AbortController | null = null;
/** The reply that is waited for. */
let pending: { kind: ListenKind; done(text: string | null): void; heard(): void } | null = null;
/** Our own `ears.stop` is on its way (a click answered the question): the `closing` it brings ends nothing. */
let stopping = false;
let warmed = false;
let conv: Conversation = { last: null, at: 0 };
let history: Exchange[] = [];
let fails = 0;
let talkedAt = 0;
/** The last thing Rust asked to tell the user: said once, shown each time. */
let warned = "";

export function turnView(): TurnView {
  return view;
}

/** `fn` is called with the new view on every change; the returned function ends it. */
export function onTurn(fn: (view: TurnView) => void): () => void {
  watchers.add(fn);
  return () => void watchers.delete(fn);
}

function set(patch: Partial<TurnView>): void {
  view = { ...view, ...patch };
  for (const fn of [...watchers]) fn(view);
}

function forget(): void {
  conv = { last: null, at: 0 };
  history = [];
  fails = 0;
}

// ── What Rust says ────────────────────────────────────────────────────────────

function onState(e: VoiceEvents["state"]): void {
  // An interruption raises the generation (§7.4): Rust silenced the mouth with this change.
  const silenced = fresh(e.gen);
  set({ state: e.state });
  if (e.state === "listening") {
    // Not the reply we wait for: the talk key or the wake phrase, a new turn whatever was going on.
    if (!pending) takeover();
  } else if (e.state === "closing") {
    // Esc, or a stop: the turn is over, whatever it was doing or waiting for.
    if (silenced && !stopping) drop();
    // The listening ended by itself: the reply did not come.
    else pending?.done(null);
  } else if (e.state === "paused" || e.state === "off") {
    drop();
  }
}

function takeover(): void {
  drop();
  warmed = false;
  set({ heard: "", final: false, answer: [], spoken: -1, by: "", result: "", confirm: null });
}

/** The turn is no longer wanted: nothing more of it is thought, asked or said. */
function drop(): void {
  turnNo++;
  live = false;
  ctl?.abort();
  ctl = null;
  pending?.done(null);
  waiting()?.no();
  fresh(0, true);
}

function onHeard(e: VoiceEvents["heard"]): void {
  const speaking = view.state === "speaking";
  // Said to stop it: it stops, and that is all. While it talks a part of the sentence is enough.
  if (isStop(e.text) && (e.final || speaking)) return endTurn();
  // While it talks nothing else is taken (its own voice, somebody in the room).
  if (speaking) return;
  set({ heard: e.text, final: e.final });
  if (!e.final) {
    pending?.heard();
    warm(e.text);
    return;
  }
  if (!e.text.trim()) return void pending?.done(null);
  // It hears again: the next trouble is news.
  warned = "";
  if (isCancel(e.text)) return endTurn();
  if (isEnd(e.text) && pending?.kind !== "freeText") {
    forget();
    return endTurn();
  }
  if (pending) return pending.done(e.text);
  void run(e.text);
}

/** Something the user should be told (the speech model is missing, the microphone is gone): shown, and said once. */
function onError(e: VoiceEvents["error"]): void {
  const c = cx;
  if (!c || !e.message) return;
  const text = c.t(e.message);
  set({ result: text });
  if (text === warned) return;
  warned = text;
  void speak(text);
}

/** The action model starts while a command the parser does not know is still being said (§4.2, "Life"). */
function warm(text: string): void {
  const c = cx;
  const d = deps;
  if (!c || !d?.prepare || warmed || pending || settings.needle === false) return;
  warmed = true;
  const list = d.entries();
  parse(text, list, d.cxOf, conv)
    .then((hit) => (hit ? void (warmed = false) : d.prepare?.(c, text, list, d.cxOf)))
    .catch(() => {});
}

// ── Listening for a reply ─────────────────────────────────────────────────────

/** Opens the microphone for a reply (Rust waits out the tail of our own voice) and answers with what was said, or null. */
function hear(kind: ListenKind): Promise<string | null> {
  const c = cx;
  if (!c) return Promise.resolve(null);
  return new Promise((resolve) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const wait = (ms: number) => {
      clearTimeout(timer);
      timer = setTimeout(() => done(null), ms);
    };
    const done = (text: string | null) => {
      if (pending !== p) return;
      pending = null;
      clearTimeout(timer);
      resolve(text);
    };
    const p = { kind, done, heard: () => wait(TIMES.word) };
    pending?.done(null);
    pending = p;
    wait(TIMES.first[kind] + TIMES.slack);
    void c.call<{ gen: number }>("ears.listen", { kind }).then((r) => (r ? fresh(r.gen) : done(null)));
  });
}

/**
 * Puts a question (spoken and shown), opens the microphone for its reply and answers with what was
 * said, or null when nothing came in time or the turn ended. For the modules that ask in several
 * steps (mail/).
 */
export async function ask(question: string, kind: ListenKind = "reply"): Promise<string | null> {
  if (!cx) return null;
  const turn = turnNo;
  await speak(question);
  return turn === turnNo ? hear(kind) : null;
}

/** Starts a turn as a tap of the talk key does (a click on the capsule). */
export function listen(kind?: ListenKind): void {
  void cx?.call<{ gen: number }>("ears.listen", { kind });
}

/** Ends the turn and what is being said: Esc, the capsule's stop button, the word "stop". */
export function endTurn(): void {
  const c = cx;
  drop();
  if (!c) return;
  void (async () => {
    // `ears.stop` silences the mouth of a turn. What still talks after it (outside a turn) is stopped by itself.
    await c.call("ears.stop");
    if ((await c.call<{ speaking: boolean }>("mouth.speaking"))?.speaking) hush();
  })();
}

/** The turn is over: the state goes to closing and to rest. */
function finish(): void {
  live = false;
  ctl = null;
  waiting()?.no();
  void rest();
}

async function rest(): Promise<void> {
  const c = cx;
  if (!c) return;
  // Without ears (not built, the speech model missing) the state would stay where the mouth left it.
  if ((await c.call("ears.stop")) === null) await c.call("state.set", { state: "idle" });
}

// ── A turn ────────────────────────────────────────────────────────────────────

async function run(text: string): Promise<void> {
  if (live) drop();
  const turn = ++turnNo;
  live = true;
  if (Date.now() - talkedAt > TIMES.memory) forget();
  for (let said: string | null = text; said !== null; ) {
    ctl = new AbortController();
    let next: ListenKind | null = null;
    try {
      next = await understand(said, turn, ctl.signal);
    } catch {
      // No words in the log (§8).
      console.error("[voice] a turn failed");
      if (turn !== turnNo) return;
      // Nothing more of it is said: ending the turn silences the mouth and brings the music back.
      fresh(0, true);
      if (++fails >= TIMES.fails) forget();
    }
    if (turn !== turnNo) return;
    await quiet();
    if (turn !== turnNo) return;
    talkedAt = Date.now();
    // It asked something: one reply is waited for, and taken as what is said next.
    said = next ? await hear(next) : null;
    if (turn !== turnNo) return;
  }
  finish();
}

/** One sentence through the parser, the action model and the text model. Answers what to listen for next, if it asked. */
async function understand(text: string, turn: number, signal: AbortSignal): Promise<ListenKind | null> {
  const c = cx!;
  const d = deps!;
  const list = d.entries();
  const t0 = Date.now();
  let by: Understood = "parser";
  let cmds = await parsed(text, list);
  if (turn !== turnNo) return null;
  const t1 = Date.now();
  if (!cmds) {
    const none: NeedlePick = { kind: "none" };
    const pick = settings.needle === false ? none : await d.choose(c, text, list, d.cxOf).catch(() => none);
    if (turn !== turnNo) return null;
    d.log?.(`voice: turn parse=${t1 - t0} needle=${Date.now() - t1} ms`);
    if (pick.kind === "miss") return missed();
    if (pick.kind !== "act") return think(text, turn, signal, list);
    cmds = [pick.parsed, ...(pick.more ?? [])];
    by = "needle";
  }
  const outs: Outcome[] = [];
  for (const cmd of cmds) {
    const [out, args] = await carry(cmd, by, text, turn);
    if (turn !== turnNo) return null;
    outs.push(out);
    // What a follow-up leans on ("and 5 more minutes").
    if (out.kind === "done") conv = { last: { ...cmd, args, missing: [] }, at: Date.now() };
  }
  // Not on the action model's word alone: as if it had not been understood.
  if (outs.every((o) => o.kind === "refused" && !o.reply)) return missed();
  if (outs.every((o) => o.kind === "failed")) {
    if (++fails >= TIMES.fails) forget();
  } else fails = 0;
  const end = outs[outs.length - 1];
  return end.kind === "done" && end.reply?.say?.trim().endsWith("?") ? "reply" : null;
}

/** Every command of the sentence when each is understood, else the sentence as one. */
async function parsed(text: string, list: readonly VoiceEntry[]): Promise<Parsed[] | null> {
  const d = deps!;
  const parts = splitCommands(text);
  if (parts.length > 1) {
    const all = await Promise.all(parts.map((part) => parse(part, list, d.cxOf, conv)));
    if (all.every((p): p is Parsed => p !== null)) return all;
  }
  const one = await parse(text, list, d.cxOf, conv);
  return one && [one];
}

/** Nothing was understood: once it asks to hear it again, the second time in a row the conversation ends without a word. */
function missed(): ListenKind | null {
  if (++fails >= TIMES.fails) {
    forget();
    return null;
  }
  void speak(cx!.t(SORRY));
  return "reply";
}

/**
 * One command to its end: the details nobody said are asked for, the action goes through its class,
 * a yes or no is waited for. Answers the outcome and the arguments it ran with.
 */
async function carry(cmd: Parsed, by: Understood, heard: string, turn: number): Promise<[Outcome, VoiceArgs]> {
  const args: VoiceArgs = { ...cmd.args };
  const none: Outcome = { kind: "dropped", reply: null };
  // What only the action model understood must have been said in full: nothing is asked for.
  if (by === "needle" && cmd.missing.length) return [{ kind: "refused", reply: null }, args];
  for (const arg of cmd.missing) {
    const value = await detail(cmd.entry, arg, turn);
    if (value === null) return [none, args];
    args[arg.name] = value;
  }
  for (let round = 0; ; round++) {
    // A detail that was asked for was said in so many words: from then on it is no longer the model's guess.
    let out = await act(cmd.entry, args, round ? "parser" : by, heard);
    if (turn !== turnNo) return [out, args];
    const w = waiting();
    if (out.kind === "asked" && w) {
      // The question is said first; a click may answer it at any moment.
      const said = await Promise.race([quiet().then(() => (waiting() === w ? hear("yesNo") : null)), w.done.then(() => null)]);
      // Only a yes is a yes: anything else, and nothing at all, is a no.
      if (said !== null && turn === turnNo && isYes(said)) w.yes();
      else w.no();
      out = await w.done;
      if (turn !== turnNo) return [out, args];
    }
    const need = out.kind === "asked" ? out.reply?.ask : undefined;
    if (!need || round >= TIMES.asks) return [out, args];
    const value = await detail(cmd.entry, need, turn, out.reply?.say);
    if (value === null) return [none, args];
    args[need.name] = value;
  }
}

/** Asks for one argument, twice at most. */
async function detail(entry: VoiceEntry, arg: VoiceArg, turn: number, question?: string): Promise<string | number | null> {
  const v = deps!.cxOf(entry);
  const q = question || (arg.ask ? v.t(arg.ask) : cx!.t("Which one?"));
  for (let n = 0; n < TIMES.asks; n++) {
    const said = await ask(q, arg.kind === "text" ? "freeText" : "reply");
    if (said === null || turn !== turnNo) return null;
    const value = await readArg(arg, said, v).catch(() => null);
    if (value !== null) return value;
  }
  return null;
}

/** What an action did, for the online model that called it. */
function told(out: Outcome): string {
  const say = out.reply?.say ?? "";
  switch (out.kind) {
    case "done":
      return say || "Done.";
    case "prepared":
      return `Prepared, not finished: only the user's click finishes it. ${say}`.trim();
    case "refused":
      return `Refused: voice may not do this, the user has to click. ${say}`.trim();
    case "dropped":
      return "The user did not say yes. Nothing was done.";
    case "asked":
      return `The user was asked: ${say}`;
    default:
      return `It did not work. ${say}`.trim();
  }
}

/** An open question: the text model writes, the mouth says it sentence by sentence. A command never comes here. */
async function think(text: string, turn: number, signal: AbortSignal, list: readonly VoiceEntry[]): Promise<ListenKind | null> {
  const c = cx!;
  const d = deps!;
  const t0 = Date.now();
  const stream = speakStream();
  let wrote = false;
  let by = "";
  let got: { text: string; brain: BrainId } | null = null;
  try {
    got = await d.answer(
      c,
      {
        text,
        history,
        state: d.stateAbout?.(text) || undefined,
        tools: list,
        // An online model's calls pass the same runner and classes.
        act: async (entry, args) => told((await carry({ entry, args, missing: [], score: 0 }, "claude", text, turn))[0]),
        by: (name) => (by = name),
        signal,
      },
      (delta) => {
        wrote = true;
        stream.push(delta);
      },
    );
  } catch {
    got = null;
  } finally {
    // Whatever happened to the model, what it wrote is said to its end and the stream is closed.
    void stream.end();
  }
  if (turn !== turnNo) return null;
  d.log?.(`voice: turn brain=${Date.now() - t0} ms`);
  if (!got?.text.trim()) {
    if (++fails >= TIMES.fails) forget();
    else if (!wrote) void speak(c.t(NOTHING));
    return null;
  }
  fails = 0;
  // An online model is named in the capsule, not aloud (one that was asked and failed named itself too: the brain that answered decides).
  if (by && (got.brain === "free" || got.brain === "claude")) set({ by });
  // Only what a model wrote goes back to a model: never what an action read out (a note, a copy).
  history = [...history, { said: text, answered: got.text }].slice(-3);
  return got.text.trim().endsWith("?") ? "reply" : null;
}

// ── From the feature's start to its stop ──────────────────────────────────────

/** Listens to Rust's events and runs the turns, from the feature's `start` to the dispose it returns. */
export function startTurns(c: FeatureCtx, d: TurnDeps): () => void {
  cx = c;
  deps = d;
  settings = {};
  warned = "";
  stopping = false;
  forget();
  const stops = [
    startRunner({
      cx: c,
      cxOf: d.cxOf,
      settings: () => settings,
      mixer: d.mixer,
      asking: d.asking,
      view: set,
      // Speech outside a turn (the picker's Test, a chat answer read aloud) is under no question: the last turn's words go.
      chunk: (text, at) =>
        set(at === 0 ? { answer: [text], spoken: -1, by: "", ...(live || pending ? {} : { heard: "", final: false }) } : { answer: [...view.answer, text] }),
      // A click on Yes: the microphone that waits for the spoken reply closes before the action runs,
      // or it would take the answer's own voice for a command. The generation that stop leaves is
      // read before anything is said: the `closing` it brings, early or late, then cuts nothing.
      settling: async () => {
        if (!pending) return;
        pending.done(null);
        stopping = true;
        try {
          await c.call("ears.stop");
          const now = await c.call<VoiceEvents["state"]>("state");
          if (now) fresh(now.gen);
        } finally {
          stopping = false;
        }
      },
      // Speech outside a turn (a chat answer read aloud): nobody else takes the state on from `speaking`.
      dry: () => {
        if (!live && !pending) void rest();
      },
    }),
    c.on<VoiceEvents["state"]>("state", onState),
    c.on<VoiceEvents["heard"]>("heard", onHeard),
    c.on<VoiceEvents["error"]>("error", onError),
    c.settings.onChange((value) => (settings = value as Partial<VoiceSettings>)),
    d.onEsc?.(() => {
      if (settings.esc !== false && BUSY.has(view.state)) endTurn();
    }) ?? (() => {}),
  ];
  void c.settings.get().then((value) => {
    if (cx === c) settings = value as Partial<VoiceSettings>;
  });
  void c.call<VoiceEvents["state"]>("state").then((now) => {
    if (now && cx === c) onState(now);
  });
  return () => {
    drop();
    for (const stop of stops) stop();
    cx = null;
    deps = null;
    set(REST);
  };
}

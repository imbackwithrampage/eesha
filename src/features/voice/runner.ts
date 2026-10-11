// Acting and speaking (docs/VOICE.md §5, §8): an action goes through its safety class, the deny list
// stands between an action and its feature, and every word that is spoken leaves from here (one
// mouth: a reply is never said twice).
// Package voice-runner (VOICE §10). No DOM, no Tauri and no registry: `startRunner` is handed what it
// needs, so `node --test` runs all of it (tests/voice-runner*.test.mjs).
//
// What voice may do, in the order `act` asks:
// 1. `NEVER`: an action that exists only to be refused (approve or deny a permission card, answer an
//    agent, send the mail, delete files, prompt an agent). It says why, and to click.
// 2. A command only the action model understood runs only when every argument was really said, for a
//    `run` action, and one that can be taken back (`undo`): one that cannot is not run at all. Its
//    reply names what was done and offers the undo.
// 3. The class. `run` runs. `confirm` asks and waits for a spoken yes or a click; anything else is a
//    no. `prepare` runs what fills the feature's own card in: the click that completes it is the user's.
// 4. Whatever an action does goes through `voiceCx`, whose `call` refuses the methods of `DENIED`.

import type { FeatureCtx, VoiceAction, VoiceArgs, VoiceCx, VoiceEntry, VoiceReply } from "../../core/feature";
import { grounded } from "./brain/needle";
import { Splitter } from "./brain/split";
import type { VoiceEvents, VoiceSettings } from "./index";
import type { TurnView } from "./turn";

// ── What voice never does ─────────────────────────────────────────────────────

const ALLOW = "I can't approve or deny by voice. Click the button on the card.";
const SEND = "I can't send by voice. It's ready: click Send.";
const DELETE = "I can't delete, clear or empty anything by voice. That takes a click.";
const PLAN = "I can't prompt an agent by voice: that would spend your plan.";
const CLICK = "I can't do that by voice. That takes a click.";
const PASTE = "I only paste after a yes.";
const MICS = "I can't unmute the microphones by voice. Press Ctrl+Alt+U.";

/** Refused by name, with what is said: the whole method, or what follows its last dot. `feature:method` names one feature's. */
const NAMES: Readonly<Record<string, string>> = {
  // §5: they send, delete, clear or allow for good.
  mail_send: SEND,
  always: ALLOW,
  bin_empty: DELETE,
  clear: DELETE,
  remove: DELETE,
  delete_reminder: DELETE,
  wallSet: CLICK,
  // A permission card and an agent's question: the answer is a click on the card.
  approval_decision: ALLOW,
  approval_answer: ALLOW,
  approval_decline: ALLOW,
  // The plan: a prompt to an agent or to the chat is the user's to send.
  chat_send: PLAN,
  // Files and programs (§8: delete, install or uninstall, elevate, end a process or the session).
  recycle: DELETE,
  uninstall: DELETE,
  "cleaner-uninstaller:clean": DELETE,
  reset_apply: DELETE,
  install: CLICK,
  launch: CLICK,
  kill: CLICK,
  shutdown: CLICK,
  restart: CLICK,
  signout: CLICK,
};

/** Refused by any word of a method's name: "remove_calendar", "mailSend", "installDriver". */
const WORDS: Readonly<Record<string, string>> = {
  send: SEND,
  allow: ALLOW,
  approve: ALLOW,
  deny: ALLOW,
  prompt: PLAN,
  delete: DELETE,
  remove: DELETE,
  clear: DELETE,
  empty: DELETE,
  erase: DELETE,
  wipe: DELETE,
  purge: DELETE,
  recycle: DELETE,
  uninstall: DELETE,
  install: CLICK,
  kill: CLICK,
};

/** The methods voice never calls on a feature, whatever an action asks (§5, §8): they send, delete, clear, allow or spend for good. */
export const DENIED: readonly string[] = Object.keys(NAMES);

const wordsOf = (method: string): string[] =>
  method.replace(/([a-z0-9])([A-Z])/g, "$1 $2").toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);

/**
 * Why voice may not call `method` of `feature`: empty when it may. `yes`: the call comes from a
 * `confirm` action the user said yes to (a paste needs one).
 */
export function denied(feature: string, method: string, args?: object, yes = false): string {
  const last = method.slice(method.lastIndexOf(".") + 1);
  // A feature's own shortcut does what its name says ("paste-plain"): it is judged by that name.
  const id = last === "shortcut" ? (args as { id?: unknown } | undefined)?.id : undefined;
  if (typeof id === "string" && id !== "shortcut") return denied(feature, id, undefined, yes);
  const named = NAMES[`${feature}:${method}`] ?? NAMES[method] ?? NAMES[last];
  if (named) return named;
  const words = wordsOf(method);
  const word = words.find((w) => w in WORDS);
  if (word) return WORDS[word];
  if (words.includes("paste") && !yes) return PASTE;
  // Muting every microphone is safe (it can no longer hear); opening them again is not voice's to do.
  if (method === "muteMics" && (args as { on?: unknown } | undefined)?.on !== true) return MICS;
  return "";
}

interface Never {
  say: string;
  describe: string;
  forms: string[];
  /** The text the forms may carry (it is never used). */
  text?: string;
  /** It is about the card that waits: the request is read aloud first. */
  card?: boolean;
}

/**
 * What voice never does, whoever understood it and whatever class the action claims: by the action's
 * tool. Each is an action of the registry (`refusals`), so that "allow it" is understood and answered
 * with the reason instead of being guessed at.
 */
const NEVER: Readonly<Record<string, Never>> = {
  permission_allow: {
    say: "I can't approve by voice. Click Allow.",
    describe: "The user wants to allow or approve what an agent asks permission for. Voice cannot: this only says so.",
    forms: [
      "(allow|approve|accept|permit|authorize) [it|that|this|the request|the command|the permission]",
      "always allow [it|that|this]",
      "allow [it|that|this] always",
      "(yes|ok|okay) (allow|approve) [it|that|this]",
      "let (it|claude|codex|the agent) (run|do it|go ahead|continue)",
    ],
    card: true,
  },
  permission_deny: {
    say: "I can't deny by voice. Click Deny.",
    describe: "The user wants to deny or refuse what an agent asks permission for. Voice cannot: this only says so.",
    forms: [
      "(deny|refuse|reject|decline|block) [it|that|this|the request|the command|the permission]",
      "(don't|do not|never) allow [it|that|this]",
    ],
    card: true,
  },
  agent_answer: {
    say: "I can't answer an agent for you. Click an option on the card.",
    describe: "The user wants to answer a question an agent asked. Voice cannot: this only says so.",
    forms: [
      "answer (it|claude|codex|the agent|the question) [with] [{text}]",
      "reply to (it|claude|codex|the agent) [with] [{text}]",
      "(choose|pick|select) option {text}",
    ],
    text: "text",
    card: true,
  },
  mail_send_now: {
    say: "I can't send mail by voice. It's ready: click Send.",
    describe: "The user wants the prepared email sent. Voice cannot send: this only says so.",
    forms: ["send (it|the email|the mail|that email|this email|the message) [now]", "send [it] now", "(yes|ok|okay) send it"],
  },
  files_delete: {
    say: "I can't delete files by voice. Do it yourself, with a click.",
    describe: "The user wants files or folders deleted. Voice cannot delete: this only says so.",
    forms: [
      "(delete|erase|trash|shred) [the|this|that|these|those|all] [my] (file|files|folder|folders) [{text}]",
      "(delete|erase) {text} from (my|the) (desktop|downloads|documents|pictures|disk|computer|pc)",
    ],
    text: "text",
  },
  agent_prompt: {
    say: "I can't prompt an agent by voice: that would spend your plan. Type it in the terminal.",
    describe: "The user wants a coding agent prompted or started. Voice cannot: this only says so.",
    forms: [
      "(tell|ask|prompt) (claude|codex|cursor|copilot|the agent|my agent) to {text}",
      "(start|launch|run|spawn) (a|an|another) [new] (agent|claude session|codex session|agent session)",
    ],
    text: "text",
  },
};

/** The tools of `NEVER`. */
export const REFUSED: readonly string[] = Object.keys(NEVER);

/** The actions that exist only to be refused: base-actions.ts adds them to the voice feature's own. */
export function refusals(): VoiceAction[] {
  return Object.entries(NEVER).map(([tool, never]) => ({
    id: tool,
    tool,
    describe: never.describe,
    forms: never.forms,
    args: never.text ? [{ name: never.text, kind: "text" as const, optional: true }] : undefined,
    // The most careful class there is, should `NEVER` ever miss it: and `run` does nothing but say why.
    safety: "prepare" as const,
    run: async () => ({ say: never.say, ok: false }),
  }));
}

// ── What the runner is handed ─────────────────────────────────────────────────

/** Who understood the command. What only the action model understood runs only when it is safe, and with an Undo. */
export type Understood = "parser" | "needle" | "claude";

export interface Outcome {
  /**
   * `done`: it ran. `asked`: a yes or no waits (`waiting()`), or a detail is missing (`reply.ask`).
   * `prepared`: the feature's card is up, the click is the user's. `refused`: voice may not (with no
   * reply: not on the action model's word alone). `dropped`: the user said no, or nothing. `failed`:
   * it did not work.
   */
  kind: "done" | "asked" | "prepared" | "refused" | "dropped" | "failed";
  reply: VoiceReply | null;
}

/** The user's yes to a `confirm` action: it holds for that one run, through the `VoiceCx` made for it, and is over when the run is. */
export interface Pass {
  yes: boolean;
}

/** A yes or no that waits. `done` settles when it was answered: with what running the action gave, or `dropped`. */
export interface Waiting {
  text: string;
  yes(): void;
  no(): void;
  done: Promise<Outcome>;
}

export interface RunnerEnv {
  /** The voice feature's own context: the mouth and its events. */
  cx: Pick<FeatureCtx, "call" | "on" | "t">;
  /** The `VoiceCx` of an action's own feature (`cxOf` of registry.ts); with `pass`, one made for a single run. */
  cxOf(entry: VoiceEntry, pass?: Pass): VoiceCx;
  /** The settings as last read. */
  settings(): Partial<VoiceSettings>;
  /** The audio mixer while that feature is on: what plays is lowered through it while the mouth speaks. */
  mixer?(): Pick<FeatureCtx, "call"> | null;
  /** What the card that waits is asking, in a sentence (the tool, never the command); "" when none waits. */
  asking?(): string;
  /** The turn's view: the question that waits, the undo, the result, the chunk being said. */
  view?(patch: Partial<TurnView>): void;
  /** A chunk of the answer, `at` counting from 0 since the last interruption. */
  chunk?(text: string, at: number): void;
  /** A click answered the question that waits: the microphone that listens for the reply closes first. */
  settling?(): Promise<void> | void;
  /** The queue ran dry: the state has to leave `speaking` (the mouth does not do it). */
  dry?(): void;
}

/** The capsule shows Undo this long (§4.2); a spoken "undo" works for as long as the conversation is remembered (§3.4). */
export const UNDO_MS = 6000;
const UNDO_SAID_MS = 90_000;
/** What plays is lowered to this, when it is louder (§2). */
const LOW = 0.25;
/** No word of the mouth for this long while something should be playing: it is asked. */
const WATCH_MS = 1500;
/** "Short replies: a sound": a reply of this many words or fewer is shown, not said. */
const SHORT = 3;

let env: RunnerEnv | null = null;

// ── Speaking ──────────────────────────────────────────────────────────────────

/** Rust's generation: what `mouth.say` carries (§7.4); and the next chunk's number within it. */
let gen = 0;
let seq = 0;
/** Ours: raised when a turn is dropped. A stream or an action of an older one says nothing more. */
let epoch = 0;
/** Streams of this epoch that have not ended, and its chunks on their way to the mouth. */
let streams = 0;
let unsent = 0;
/** The mouth was given something of ours and has not been found quiet since. */
let talking = false;
/** The chunks of this generation, as shown; and where each chunk the mouth got stands among them, by its `seq`. */
let said: string[] = [];
let sent: number[] = [];
/** What the generation before said: "repeat". */
let before = "";
/** One call to the mouth at a time, in order. */
let chain: Promise<unknown> = Promise.resolve();
let waiters: (() => void)[] = [];
let watch: ReturnType<typeof setTimeout> | undefined;
let checking = false;
/** The mouth ran dry while a text was still being written: the music stays low and the state where it is until that text ends. */
let owed = false;

/**
 * What Rust's generation is now. When it moved, Rust has silenced the mouth (§7.4) and counts its
 * chunks from nothing again: answers true. That alone ends no stream and no action: the turn loop
 * raises it itself each time it listens for a reply. `cut` does: the turn was dropped, and what is
 * still written or done for it says nothing more (what the mouth already has is watched to its end).
 */
export function fresh(g: number, cut = false): boolean {
  const moved = g > gen;
  if (moved) {
    gen = g;
    seq = 0;
    sent = [];
    newPage();
    settle();
  }
  if (cut) {
    epoch++;
    streams = 0;
    unsent = 0;
    newPage();
    for (const done of waiters.splice(0)) done();
    // What the mouth still has is watched to its end; when it has nothing, the music comes back now.
    // The state is left to whoever dropped the turn.
    if (talking) arm();
    else settle();
  }
  return moved;
}

function newPage(): void {
  if (said.length) before = said.join(" ");
  said = [];
}

/** The mouth is quiet and nothing more is coming: the music comes back, whoever waited goes on. */
function settle(): void {
  clearTimeout(watch);
  talking = false;
  owed = false;
  unduck();
  for (const done of waiters.splice(0)) done();
}

/** All that was written has been said. The mouth sets `speaking` and never leaves it: whoever runs the turns takes the state on. */
function rested(): void {
  settle();
  env?.dry?.();
}

function arm(): void {
  clearTimeout(watch);
  if (talking) watch = setTimeout(() => void check(), WATCH_MS);
}

/** Has the queue run dry? Asked when the player says its level is nothing, and when it says nothing at all. */
async function check(): Promise<void> {
  const e = env;
  if (!e || !talking || checking || unsent > 0) return;
  checking = true;
  const mouth = await e.cx.call<{ speaking: boolean }>("mouth.speaking");
  checking = false;
  if (!talking) return;
  if (mouth?.speaking || unsent > 0) return arm();
  if (!streams) return rested();
  // More is being written (a slow model, or one that waits for a yes): whoever waits for the mouth
  // goes on, and the rest is done when that text ends.
  clearTimeout(watch);
  talking = false;
  owed = true;
  for (const done of waiters.splice(0)) done();
}

function say(text: string, aloud: boolean): void {
  const e = env;
  const chunk = text.trim();
  // One mouth: what was already said in this generation is not said again.
  if (!e || !chunk || said.some((s) => s.toLowerCase() === chunk.toLowerCase())) return;
  const at = said.push(chunk) - 1;
  e.chunk?.(chunk, at);
  if (!aloud || e.settings().speak === false) return;
  const mine = epoch;
  unsent++;
  chain = chain
    .then(async () => {
      if (mine !== epoch) return;
      await duck();
      if (mine !== epoch) return;
      // Numbered as it leaves: an interruption before this starts the count again.
      const n = seq++;
      sent[n] = at;
      const took = await e.cx.call("mouth.say", { gen, seq: n, text: chunk });
      if (mine !== epoch) return;
      unsent--;
      if (took !== null) {
        talking = true;
        arm();
      } else if (talking) void check();
      // The mouth took nothing at all (voice went off): nobody waits for it.
      else if (!unsent) settle();
    })
    .catch(() => {});
}

/** Settles when nothing is being said (or the turn that said it was dropped): at once when nothing is. */
export function quiet(): Promise<void> {
  return talking || unsent > 0 ? new Promise((done) => waiters.push(done)) : Promise.resolve();
}

/** Says a text while it is being written: `push` each piece as it arrives, `end` when the last one has. `aloud` false: shown, not said. */
export function speakStream(aloud = true): { push(delta: string): void; end(): Promise<void> } {
  const mine = epoch;
  const split = new Splitter();
  let open = true;
  streams++;
  const out = (chunks: string[]) => {
    for (const chunk of chunks) if (mine === epoch) say(chunk, aloud);
  };
  return {
    push(delta) {
      if (open) out(split.push(delta));
    },
    end() {
      if (open) {
        open = false;
        out(split.end());
        if (mine === epoch) {
          streams--;
          if (owed && !streams && !talking && !unsent) rested();
          else void check();
        }
      }
      return quiet();
    },
  };
}

/** Says a fixed reply, sentence by sentence (brain/split.ts, `mouth.say`). Settles when it has been said or was interrupted. */
export function speak(text: string): Promise<void> {
  const stream = speakStream();
  stream.push(text);
  return stream.end();
}

/** Stops what is being said and drops what was queued (an interruption, §7.4). */
export function hush(): void {
  const e = env;
  fresh(0, true);
  if (!e) return;
  chain = chain
    .then(async () => {
      const mouth = await e.cx.call<{ gen: number }>("mouth.stop");
      if (mouth && mouth.gen > gen) {
        gen = mouth.gen;
        seq = 0;
        sent = [];
      }
      // Quiet now, unless something was said since `hush` returned: that belongs to the new generation.
      if (!unsent) settle();
    })
    .catch(() => {});
}

/** What was last said, for "repeat"; empty when nothing was. */
export function lastSpoken(): string {
  return said.length ? said.join(" ") : before;
}

// ── The music ─────────────────────────────────────────────────────────────────

interface MixApp {
  id: string;
  volume: number;
  mute: boolean;
  playing: boolean;
  system: boolean;
}

/** The mixer's calls one after the other: a restore never crosses the next lowering. */
let mix: Promise<unknown> = Promise.resolve();
let low = false;
let raise: (() => Promise<void>) | null = null;

function duck(): Promise<unknown> {
  if (low || env?.settings().duck === false) return mix;
  const mixer = env?.mixer?.();
  if (!mixer) return mix;
  low = true;
  return (mix = mix
    .then(async () => {
      if (low) raise = await lower(mixer);
    })
    .catch(() => {}));
}

function unduck(): void {
  if (!low) return;
  low = false;
  mix = mix
    .then(async () => {
      const back = raise;
      raise = null;
      await back?.();
    })
    .catch(() => {});
}

/**
 * Every app that sounds louder than `LOW` goes down to it; answers how to put them back.
 * debt: all that sounds, not only the media player (a call too); an app that closes while lowered keeps the low level.
 */
async function lower(mixer: Pick<FeatureCtx, "call">): Promise<(() => Promise<void>) | null> {
  const apps = async () => (await mixer.call<{ apps?: MixApp[] }>("state"))?.apps ?? [];
  const loud = (await apps()).filter((a) => a.playing && !a.system && !a.mute && a.volume > LOW);
  if (!loud.length) return null;
  await Promise.all(loud.map((a) => mixer.call("appVolume", { id: a.id, level: LOW })));
  return async () => {
    // Only what is still where it was put: a level the user (or a command) changed meanwhile stays.
    const now = await apps();
    const mine = loud.filter((a) => Math.abs((now.find((n) => n.id === a.id)?.volume ?? -1) - LOW) < 0.01);
    await Promise.all(mine.map((a) => mixer.call("appVolume", { id: a.id, level: a.volume })));
  };
}

// ── Acting ────────────────────────────────────────────────────────────────────

class Denied extends Error {}

/** The reason of the last call the deny list refused, and how many it has refused. */
let refusal = "";
let denials = 0;
let waitingFor: Waiting | null = null;
let last: { entry: VoiceEntry; args: VoiceArgs; at: number } | null = null;
let undoTimer: ReturnType<typeof setTimeout> | undefined;

/** What an action is handed: the context of its own feature (`featureCtx(entry.feature)`), with the deny list in front of `call`. */
export function voiceCx(cx: Pick<FeatureCtx, "call" | "t" | "island"> & { id?: string }, pass?: Pass): VoiceCx {
  return {
    call: <T>(method: string, args?: object): Promise<T | null> => {
      const why = denied(cx.id ?? "", method, args, pass?.yes === true);
      if (!why) return cx.call<T>(method, args);
      refusal = why;
      denials++;
      return Promise.reject(new Denied(method));
    },
    t: cx.t,
    open: (section) => cx.island.open(section),
  };
}

/** The yes or no that waits, or null. */
export function waiting(): Waiting | null {
  return waitingFor;
}

/** Says why not, in the words given. */
function refuse(text: string): Outcome {
  utter(text);
  return { kind: "refused", reply: { say: text, ok: false } };
}

function utter(text: string, aloud = true): void {
  const stream = speakStream(aloud);
  stream.push(text);
  void stream.end();
}

/** Every argument was really said (§4.2, rule 2), a choice by one of the ways it is said today. */
async function saidAll(e: RunnerEnv, heard: string, entry: VoiceEntry, args: VoiceArgs): Promise<boolean> {
  if (!heard.trim()) return false;
  try {
    const says: Record<string, string[]> = {};
    for (const arg of entry.action.args ?? []) {
      if (arg.kind !== "choice" || !(arg.name in args)) continue;
      const choice = (await arg.choices?.(e.cxOf(entry)))?.find((c) => c.id === args[arg.name]);
      if (!choice) return false;
      says[arg.name] = choice.say;
    }
    return grounded(heard, entry, args, says);
  } catch {
    return false;
  }
}

/**
 * Runs an action through its class (run, confirm, prepare), speaks its reply and keeps its undo.
 * `heard`: what was said, for a command only the action model understood.
 */
export async function act(entry: VoiceEntry, args: VoiceArgs, by: Understood, heard = ""): Promise<Outcome> {
  const e = env;
  if (!e) return { kind: "failed", reply: null };
  const { action } = entry;
  const never = NEVER[action.tool];
  if (never) return refuse([never.card ? e.asking?.() : "", e.cx.t(never.say)].filter(Boolean).join(" "));
  // On the action model's word alone: only what is safe, was said in full and can be taken back.
  if (by === "needle" && (action.safety !== "run" || !action.undo || !(await saidAll(e, heard, entry, args)))) {
    return { kind: "refused", reply: null };
  }
  if (env !== e) return { kind: "failed", reply: null };
  return action.safety === "confirm" ? confirmFirst(e, entry, args, by) : perform(e, entry, args, by, false);
}

function confirmFirst(e: RunnerEnv, entry: VoiceEntry, args: VoiceArgs, by: Understood): Outcome {
  waitingFor?.no();
  const text = entry.action.confirm?.(args) ?? e.cx.t("Shall I go ahead?");
  let settle!: (out: Outcome) => void;
  const done = new Promise<Outcome>((resolve) => (settle = resolve));
  const close = (run: boolean) => {
    if (waitingFor !== w) return;
    waitingFor = null;
    e.view?.({ confirm: null });
    if (!run) return settle({ kind: "dropped", reply: null });
    void (async () => {
      await e.settling?.();
      return perform(e, entry, args, by, true);
    })().then(settle, () => settle({ kind: "failed", reply: null }));
  };
  const w: Waiting = { text, yes: () => close(true), no: () => close(false), done };
  waitingFor = w;
  e.view?.({ confirm: { text, yes: w.yes, no: w.no } });
  utter(text);
  return { kind: "asked", reply: { say: text } };
}

async function perform(e: RunnerEnv, entry: VoiceEntry, args: VoiceArgs, by: Understood, confirmed: boolean): Promise<Outcome> {
  const { action } = entry;
  const mine = epoch;
  const denialsBefore = denials;
  let reply: VoiceReply = {};
  let threw = false;
  const pass: Pass = { yes: confirmed };
  try {
    reply = (await action.run(args, confirmed ? e.cxOf(entry, pass) : e.cxOf(entry))) ?? {};
  } catch (err) {
    threw = true;
    // No words in the log (§8): the action's name only.
    if (!(err instanceof Denied)) console.error(`[voice] ${entry.feature}.${action.id} failed`);
  } finally {
    // The yes was for this run: a `VoiceCx` kept beyond it pastes nothing.
    pass.yes = false;
  }
  // Its turn was dropped, or voice switched off, while it ran: it says nothing into what took over.
  const stale = mine !== epoch || env !== e;
  // The deny list refused a call of it: whatever the action answers, that is what happened.
  if (denials > denialsBefore) return stale ? { kind: "refused", reply: { say: e.cx.t(refusal), ok: false } } : refuse(e.cx.t(refusal));
  const failed = threw || reply.ok === false;
  const kind: Outcome["kind"] = failed ? "failed" : reply.ask ? "asked" : action.safety === "prepare" ? "prepared" : "done";
  // On the action model's word alone the reply always names what was done and offers to take it
  // back: the model acts on any sentence that holds every detail ("the timer went off twenty minutes ago").
  const model = by === "needle" && kind === "done";
  const own = model && /^(?:done|ok|okay)\W*$/i.test(reply.say?.trim() ?? "") ? undefined : reply.say;
  const named = own ?? (model ? (reply.show ?? e.cx.t("Done: {what}.", { what: action.tool.replace(/_/g, " ") })) : undefined);
  const plain =
    named ?? (failed ? e.cx.t("That didn't work.") : kind === "prepared" ? e.cx.t("It's ready. Click to finish.") : kind === "done" ? e.cx.t("Done.") : "");
  const text = model ? `${plain} ${e.cx.t("Say undo to take it back.")}` : plain;
  const out: Outcome = { kind, reply: { ...reply, say: text } };
  if (stale) return out;
  if (reply.show) e.view?.({ result: reply.show });
  if (kind === "done" && action.undo) offerUndo(e, entry, args);
  // A missing detail is the turn's to ask for. What an online model had done, or failed to do, it
  // words itself: only what must be said exactly (a click is needed) is said here.
  if (kind === "asked" || (by === "claude" && kind !== "prepared")) return out;
  const short = kind === "done" && !model && e.settings().shortReplies === "sound" && text.trim().split(/\s+/).length <= SHORT;
  utter(text, !short);
  return out;
}

function offerUndo(e: RunnerEnv, entry: VoiceEntry, args: VoiceArgs): void {
  last = { entry, args, at: Date.now() };
  clearTimeout(undoTimer);
  e.view?.({ undo: () => void undoLast() });
  undoTimer = setTimeout(() => env?.view?.({ undo: null }), UNDO_MS);
}

/** "Undo": takes back the last action that can be. False when there is none. */
export async function undoLast(): Promise<boolean> {
  const e = env;
  const it = last;
  last = null;
  clearTimeout(undoTimer);
  e?.view?.({ undo: null });
  if (!e || !it?.entry.action.undo || Date.now() - it.at > UNDO_SAID_MS) return false;
  try {
    await it.entry.action.undo(it.args, e.cxOf(it.entry));
    return true;
  } catch {
    return false;
  }
}

// ── From the feature's start to its stop ──────────────────────────────────────

/** Follows the mouth (a chunk starts, the level falls to nothing) from the feature's `start` to the dispose it returns. */
export function startRunner(e: RunnerEnv): () => void {
  env = e;
  const offs = [
    e.cx.on<VoiceEvents["spoke"]>("spoke", (s) => {
      if (s.gen !== gen) return;
      e.view?.({ spoken: sent[s.seq] ?? s.seq });
      arm();
    }),
    // The ears send `{mic}` alone, the mouth `{out}` alone.
    e.cx.on<Partial<VoiceEvents["level"]>>("level", (l) => {
      if (!talking || l.out === undefined) return;
      if (l.out > 0) arm();
      else void check();
    }),
  ];
  return () => {
    for (const off of offs) off();
    waitingFor?.no();
    clearTimeout(undoTimer);
    last = null;
    // Nothing more is said, and the music is put back.
    fresh(0, true);
    settle();
    before = "";
    // Rust's count goes on while voice is off: it is read anew at the next start.
    gen = 0;
    seq = 0;
    sent = [];
    env = null;
  };
}

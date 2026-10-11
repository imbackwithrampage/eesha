// The action model, page side (docs/VOICE.md §4.2): the registry as the model's tools, and the
// decision about what it chose. Its confidence does not decide: a call passes only when every
// argument was really said (grounding) and a word of the action's forms is there (trigger). The model
// itself is Rust's (`needle.tools`, `needle.choose`).
// Package voice-needle (VOICE §10). Pure but for `cx.call`, so `node --test` runs it against a fixture
// registry (tests/voice-needle.test.mjs).
//
// The engine reads every tool when it starts: nine take 1.6 s, sixty take 18 s and it then picks
// wrong ones. So a registry of more than `LIMIT` actions is not handed over whole: each sentence gets
// the actions that fit it best (`shortlist`), and a new list means a new server.

import type { FeatureCtx, VoiceArg, VoiceArgs, VoiceChoice, VoiceEntry } from "../../../core/feature";
import type { NeedleChoice } from "../index";
import { type CxOf, type Parsed, resolveChoice } from "../parse/index";

/** One tool as the model is given it: an action, or `no_action`. `parameters` is a JSON schema object, closed sets as enums. */
export interface NeedleTool {
  name: string;
  description: string;
  parameters: { type: "object"; properties: Record<string, unknown>; required: string[] };
}

/**
 * `act`: the call passed, run it (`more`: the further commands of the same sentence, in the order
 * said). `question`: it did not, and the sentence is a question, or no command at all: the text
 * model's. `miss`: it did not: "Sorry, say that again?". `none`: the model was not asked (switched
 * off, not downloaded, no answer in time): the turn goes on to the text model.
 */
export type NeedlePick = { kind: "act"; parsed: Parsed; more?: Parsed[] } | { kind: "question" } | { kind: "miss" } | { kind: "none" };

/** `needle.choose` as Rust answers it: `more` are the further calls of one sentence. */
export type Chosen = NeedleChoice & { more?: { tool: string; args: Record<string, unknown> }[] };

export const NO_ACTION = "no_action";
/** Under this the model's call is not believed (§4.2, rule 1). */
const FLOOR = 0.3;
/** Tools per sentence (§10, the risk line). */
export const LIMIT = 12;
/** A longer list of choices is not given to the model as a closed set. */
const ENUM_MAX = 24;

// ── What was heard ────────────────────────────────────────────────────────────

/** Words that tell no action from another: never a trigger. */
const SMALL = new Set(
  ("a an the my me i you your it its this that these to for of in on at by with from is are am was be do does did dont can could " +
    "would will please and or not no what whats how when where who which let now right there here some any all").split(" "),
);

/** Lowercase words: an apostrophe joins ("don't" is "dont"), a digit parts from a letter ("5pm", "20%"), all else separates. */
function wordsOf(text: string): string[] {
  const plain = text.toLowerCase().replace(/['’]/g, "").replace(/(\d)(?=\p{L})|(\p{L})(?=\d)/gu, "$1$2 ");
  return plain.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
}

/** "timers" is "timer". */
const stem = (word: string): string => (word.length > 3 && word.endsWith("s") ? word.slice(0, -1) : word);

const UNITS = "zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen".split(" ");
const TENS = "twenty thirty forty fifty sixty seventy eighty ninety".split(" ");

/** Every number said, as digits or in words to 999; "half" 0.5, "quarter" 0.25, "a couple" 2, "an" before a unit of time 1. */
function numbersIn(words: readonly string[]): number[] {
  const out: number[] = [];
  let run: number | null = null;
  words.forEach((word, i) => {
    const unit = UNITS.indexOf(word);
    const ten = TENS.indexOf(word);
    const tail = run === null ? -1 : run % 100;
    const round = run !== null && run > 0 && tail === 0;
    if (run !== null && unit >= 0 && (round || (unit < 10 && tail >= 20 && tail % 10 === 0))) run += unit;
    else if (run !== null && ten >= 0 && round) run += (ten + 2) * 10;
    else if (run !== null && word === "hundred" && run > 0 && run < 10) run *= 100;
    else if (word === "and" && round) return;
    else {
      if (run !== null) out.push(run);
      run = unit >= 0 ? unit : ten >= 0 ? (ten + 2) * 10 : word === "hundred" ? 100 : null;
      if (/^\d+$/.test(word)) out.push(Number(word));
      else if (word === "half") out.push(0.5);
      else if (word === "quarter") out.push(0.25);
      else if (word === "couple") out.push(2);
      else if ((word === "a" || word === "an") && /^(hour|minute|second)s?$/.test(words[i + 1] ?? "")) out.push(1);
    }
  });
  if (run !== null) out.push(run);
  return out;
}

interface Heard {
  words: string[];
  stems: Set<string>;
  /** The words with a space around each: a phrase is in it between two spaces. */
  line: string;
  numbers: number[];
}

function hear(text: string): Heard {
  const words = wordsOf(text);
  return { words, stems: new Set(words.map(stem)), line: ` ${words.join(" ")} `, numbers: numbersIn(words) };
}

const phraseSaid = (h: Heard, phrase: string): boolean => {
  const words = wordsOf(phrase).join(" ");
  return words !== "" && h.line.includes(` ${words} `);
};

/** As it is, or in hours ("two hours" is 120, "an hour and a half" 90, "2 hours 15" 135), or in seconds. */
function numberSaid(h: Heard, value: number): boolean {
  const near = (n: number) => Math.abs(n - value) < 0.01;
  const n = h.numbers;
  if (n.some(near)) return true;
  if (h.words.some((w) => /^(hours?|hrs?|h)$/.test(w)) && n.some((a) => near(a * 60) || n.some((b) => near(a * 60 + b) || near((a + b) * 60)))) return true;
  return h.stems.has("second") && n.some((a) => near(a / 60));
}

/** "17:30" was said as "5 30", "half past 5", "17 30"; "12:00" as "noon". */
function timeSaid(h: Heard, value: string): boolean {
  const m = /^(\d{1,2}):(\d{2})$/.exec(value);
  if (!m) return false;
  const [hour, minute] = [Number(m[1]), Number(m[2])];
  const has = (n: number) => h.numbers.includes(n);
  const hourSaid = has(hour) || has(hour % 12 || 12) || (hour === 12 && h.stems.has("noon")) || (hour === 0 && h.stems.has("midnight"));
  return hourSaid && (minute === 0 || has(minute) || (minute === 30 && has(0.5)) || (minute % 15 === 0 && has(0.25)));
}

/**
 * Every argument is in what was said: a number as digits or words, a choice by a spoken form, free
 * text as a part of it. `says`: for a choice, the ways its value is said (the id alone when left out).
 */
export function grounded(text: string, entry: VoiceEntry, args: VoiceArgs, says: Readonly<Record<string, readonly string[]>> = {}): boolean {
  const h = hear(text);
  return Object.entries(args).every(([name, value]) => {
    const arg = entry.action.args?.find((a) => a.name === name);
    if (!arg) return false;
    if (typeof value === "number") return numberSaid(h, value);
    if (arg.kind === "time") return timeSaid(h, value);
    if (arg.kind === "choice") return [...(says[name] ?? []), value].some((form) => phraseSaid(h, form));
    return phraseSaid(h, value);
  });
}

// ── The words of an action ────────────────────────────────────────────────────

/**
 * The words of an action's forms, small words and arguments left out. `heads`: the word a form opens
 * with after its `[optional]` parts, every alternative of it: its verb ("set", "open", "mute"). A form
 * that opens with an argument or a small word ("{minutes} timer", "what are my agents doing") has
 * none. `rest`: the other words.
 */
function formWords(forms: readonly string[]): { heads: Set<string>; rest: Set<string> } {
  const heads = new Set<string>();
  const rest = new Set<string>();
  for (const form of forms) {
    let optional = 0;
    let group = 0;
    let groups = 0;
    let head: number | null = null;
    for (const [token] of form.toLowerCase().replace(/['’]/g, "").matchAll(/\{[^}]*\}|[[\]()]|[\p{L}\p{N}]+/gu)) {
      if (token === "[") optional++;
      else if (token === "]") optional--;
      else if (token === "(") group = ++groups;
      else if (token === ")") group = 0;
      else if (token.startsWith("{") || SMALL.has(token)) {
        if (optional === 0 && head === null) head = -1;
      } else {
        const opens = optional === 0 && (head === null || (head !== 0 && head === group));
        if (opens) head ??= group;
        (opens ? heads : rest).add(stem(token));
      }
    }
  }
  for (const word of heads) rest.delete(word);
  return { heads, rest };
}

const anySaid = (h: Heard, words: Set<string>): number => [...words].filter((w) => h.stems.has(w)).length;

/**
 * §4.2, rule 3: a verb of the action's forms is there, or another word of them with an argument that
 * was said, or two of them (a question has no verb: "what are my agents doing").
 */
function triggered(h: Heard, entry: VoiceEntry, args: VoiceArgs): boolean {
  const { heads, rest } = formWords(entry.action.forms);
  const others = anySaid(h, rest);
  return anySaid(h, heads) > 0 || others >= 2 || (others > 0 && Object.keys(args).length > 0);
}

async function choicesOf(entry: VoiceEntry, of: CxOf): Promise<Record<string, VoiceChoice[]>> {
  const lists: Record<string, VoiceChoice[]> = {};
  for (const arg of entry.action.args ?? []) {
    if (arg.kind === "choice" && arg.choices) lists[arg.name] = await arg.choices(of(entry));
  }
  return lists;
}

/**
 * How much of what was said is an action's own: 2 for each verb of its forms, 1 for each other word
 * of them, 2 when a choice of its is named; and with any of those, 1 when it takes a number and one
 * was said (the parser's "more slots filled").
 */
function fit(h: Heard, entry: VoiceEntry, lists: Record<string, VoiceChoice[]>): number {
  const { heads, rest } = formWords(entry.action.forms);
  const named = Object.values(lists).some((list) => list.some((choice) => choice.say.some((form) => phraseSaid(h, form))));
  const own = 2 * anySaid(h, heads) + anySaid(h, rest) + (named ? 2 : 0);
  const counts = h.numbers.length > 0 && (entry.action.args ?? []).some((arg) => arg.kind === "number" || arg.kind === "minutes" || arg.kind === "percent");
  return own > 0 && counts ? own + 1 : own;
}

/**
 * The actions the model is asked about for one sentence: all of them when they are `LIMIT` or fewer
 * (one list, so one server). Of a longer registry, the ones that fit the sentence best, and each
 * command of it best ("pause the music and set the volume to 20"). Only the best: measured on 59
 * actions, the model given every action that shares a word with the sentence takes a neighbour
 * ("skip this song" → repeat, "open the clipboard" → clear the clipboard): 17 of 23 right.
 */
export async function shortlist(text: string, entries: readonly VoiceEntry[], of: CxOf): Promise<VoiceEntry[]> {
  if (entries.length <= LIMIT) return [...entries];
  const lists = await Promise.all(entries.map((entry) => choicesOf(entry, of)));
  const best = new Set<VoiceEntry>();
  for (const part of [text, ...text.split(/\b(?:and|then)\b/i)]) {
    const h = hear(part);
    const fits = entries.map((entry, i) => fit(h, entry, lists[i]));
    const top = Math.max(...fits);
    if (top > 0) entries.forEach((entry, i) => fits[i] === top && best.add(entry));
  }
  return [...best].slice(0, LIMIT);
}

// ── Tools ─────────────────────────────────────────────────────────────────────

/** How a choice is named to the model: its first spoken form, in plain words. */
const label = (choice: VoiceChoice): string => wordsOf(choice.say[0] ?? choice.id).join(" ");

/** A form as somebody would say it: "open [the] {section}" is "open the clipboard". */
function example(form: string, args: readonly VoiceArg[], lists: Record<string, VoiceChoice[]>): string {
  const sample = (name: string): string => {
    const first = lists[name]?.[0];
    const samples: Record<VoiceArg["kind"], string> = {
      number: "3", minutes: "10 minutes", percent: "40 percent", time: "5 pm", text: "…", choice: first ? label(first) : name,
    };
    return samples[args.find((a) => a.name === name)?.kind ?? "text"];
  };
  return form
    .replace(/[[(]([^|)\]]*)[^)\]]*[)\]]/g, "$1")
    .replace(/\{([^}]*)\}/g, (_, name: string) => sample(name))
    .replace(/\s+/g, " ")
    .trim();
}

function property(arg: VoiceArg, list: VoiceChoice[] | undefined): Record<string, unknown> {
  switch (arg.kind) {
    case "choice":
      return list && list.length > 0 && list.length <= ENUM_MAX ? { type: "string", enum: [...new Set(list.map(label))] } : { type: "string" };
    case "text":
      return { type: "string" };
    case "time":
      return { type: "string", description: "time of day as HH:MM, 24-hour clock" };
    case "percent":
      return { type: "integer", minimum: 0, maximum: 100 };
    case "minutes":
      return { type: "integer", description: "length in minutes" };
    default:
      return { type: "integer" };
  }
}

const NOTHING: NeedleTool = {
  name: NO_ACTION,
  description: "Use for small talk, thanks, jokes, open questions or anything that none of the other tools can do.",
  parameters: { type: "object", properties: {}, required: [] },
};

/** One tool per action (its `describe`, two spoken examples, its arguments) and `no_action`. */
export async function needleTools(entries: readonly VoiceEntry[], of: CxOf): Promise<NeedleTool[]> {
  const tools: NeedleTool[] = [];
  for (const entry of entries) {
    const { tool, describe, forms, args = [] } = entry.action;
    const lists = await choicesOf(entry, of);
    const said = forms.slice(0, 2).map((form) => `'${example(form, args, lists)}'`);
    tools.push({
      name: tool,
      description: said.length ? `${describe} Said as ${said.join(" or ")}.` : describe,
      parameters: {
        type: "object",
        properties: Object.fromEntries(args.map((arg) => [arg.name, property(arg, lists[arg.name])])),
        required: args.filter((arg) => !arg.optional).map((arg) => arg.name),
      },
    });
  }
  return [...tools, NOTHING];
}

/** `sentence`: the tools are one sentence's, not the registry (Rust then does not start the server at the next command's first word). `warm`: start it now. */
const push = async (cx: Pick<FeatureCtx, "call">, entries: readonly VoiceEntry[], of: CxOf, sentence: boolean, warm = false): Promise<boolean> =>
  (await cx.call("needle.tools", { tools: await needleTools(entries, of), sentence, warm })) !== null;

/**
 * Hands the tools to Rust: when voice starts and after each read of the registry. A registry of more
 * than `LIMIT` actions is handed over sentence by sentence instead (`prepare`, `choose`).
 */
export async function pushTools(cx: Pick<FeatureCtx, "call">, entries: readonly VoiceEntry[], of: CxOf): Promise<void> {
  if (entries.length <= LIMIT) await push(cx, entries, of, false);
}

/**
 * Gets the model ready for a sentence that is still being said (a partial transcript the parser
 * missed): its tools, and the server started with them, so that `choose` finds it up.
 */
export async function prepare(cx: Pick<FeatureCtx, "call">, text: string, entries: readonly VoiceEntry[], of: CxOf): Promise<void> {
  const short = await shortlist(text, entries, of);
  if (short.length) await push(cx, short, of, entries.length > LIMIT, true);
}

// ── The decision ──────────────────────────────────────────────────────────────

const ASKS = /^(what|whats|who|whos|when|where|why|how|which|is|are|am|was|were|do|does|did|can|could|will|would|should|have|has)$/;

const isQuestion = (text: string): boolean => text.trim().endsWith("?") || ASKS.test(wordsOf(text)[0] ?? "");

/** The model's arguments as the action's: a number as a number, a choice as its id. Null: a value the action cannot take. */
function argsOf(entry: VoiceEntry, raw: Record<string, unknown>, lists: Record<string, VoiceChoice[]>): { args: VoiceArgs; says: Record<string, string[]> } | null {
  const args: VoiceArgs = {};
  const says: Record<string, string[]> = {};
  for (const [name, value] of Object.entries(raw)) {
    const arg = entry.action.args?.find((a) => a.name === name);
    if (value === null || value === "") continue;
    if (!arg || (typeof value !== "string" && typeof value !== "number")) return null;
    if (arg.kind === "number" || arg.kind === "minutes" || arg.kind === "percent") {
      if (!Number.isFinite(Number(value))) return null;
      args[name] = Number(value);
    } else if (arg.kind === "choice" && lists[name]) {
      const said = wordsOf(String(value)).join(" ");
      const choice = lists[name].find((c) => label(c) === said) ?? resolveChoice(String(value), lists[name]);
      if (!choice) return null;
      args[name] = choice.id;
      says[name] = choice.say;
    } else {
      args[name] = String(value);
    }
  }
  return { args, says };
}

/**
 * §4.2, rules 1 to 4, on what the model answered (`null`: it did not). Whether an action that passes
 * runs, asks or only prepares is its safety class, and the runner's (`act(…, "needle")`).
 */
export async function pick(text: string, choice: Chosen | null, entries: readonly VoiceEntry[], of: CxOf): Promise<NeedlePick> {
  if (!choice) return { kind: "none" };
  const calls = [{ tool: choice.tool, args: choice.args }, ...(choice.more ?? [])];
  // It called nothing, or `no_action`: not a command. The text model's.
  if (!choice.suppressed && calls.every((call) => !call.tool || call.tool === NO_ACTION)) return { kind: "question" };
  const fail: NeedlePick = { kind: isQuestion(text) ? "question" : "miss" };
  if (choice.suppressed || choice.confidence < FLOOR) return fail;
  const h = hear(text);
  const parsed: Parsed[] = [];
  for (const call of calls) {
    const entry = entries.find((e) => e.action.tool === call.tool);
    const got = entry && argsOf(entry, call.args, await choicesOf(entry, of));
    if (!entry || !got || !grounded(text, entry, got.args, got.says) || !triggered(h, entry, got.args)) return fail;
    const missing = (entry.action.args ?? []).filter((arg) => !arg.optional && !(arg.name in got.args));
    parsed.push({ entry, args: got.args, missing, score: 0 });
  }
  return { kind: "act", parsed: parsed[0], ...(parsed.length > 1 ? { more: parsed.slice(1) } : {}) };
}

/** Asks the model about a sentence the parser missed and decides (§4.2, rules 1 to 4). */
export async function choose(cx: Pick<FeatureCtx, "call">, text: string, entries: readonly VoiceEntry[], of: CxOf): Promise<NeedlePick> {
  const short = await shortlist(text, entries, of);
  // No action has a word of it: none could pass the trigger. Not a command.
  if (!short.length) return { kind: "question" };
  if (!(await push(cx, short, of, entries.length > LIMIT))) return { kind: "none" };
  return pick(text, await cx.call<Chosen>("needle.choose", { text }), short, of);
}

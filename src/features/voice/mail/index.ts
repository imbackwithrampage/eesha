// Email by voice (docs/VOICE.md §2, §4.3 "Mail text", §5): it asks only for what is missing (`ask`
// of ../turn.ts), takes a file dropped within 20 s, has the text written word for word, by the routed
// brain or from a template, opens the chat-plus mail window filled in, and stops there: Send is a
// click (class `prepare`; `mail_send` is on the deny list). An action like any other: base-actions.ts
// adds it to the voice feature's own.
// Package voice-mail (VOICE §10).
//
// What this file may call on chat-plus: `mail_open`, nothing else (and through `voiceCx`, whose deny
// list refuses `mail_send` whatever is asked). No path of this file names `mail_send`.

import { Bridge } from "../../../core/bridge";
import type { VoiceAction, VoiceArg, VoiceArgs, VoiceCx, VoiceReply } from "../../../core/feature";
import { State } from "../../../core/state";
import { isNo, isYes, normalise } from "../parse/index";
import { speak, voiceCx } from "../runner";
import { ask as turnAsk, TIMES } from "../turn";
import type { ListenKind } from "../index";

// ── What is said ──────────────────────────────────────────────────────────────

const SEND_NOTE = "I can't send it by voice. Press Send when you are ready.";
const READY = "The email is ready. Press Send when you are ready.";
const NO_ADDRESS = "I couldn't get the address, so nothing is open.";
const STOPPED = "Okay, I won't open it.";
const OFF = "I couldn't open the mail window. Is email switched on?";

/** A drop is waited for this long (§3.4, the file row). */
export const FILE_MS = TIMES.first.file;
/** After a dropped file shows, its inbox copy replaces its path within this long; then the first path is taken as it is. */
const INGEST_MS = 3000;

// ── The pure parts ────────────────────────────────────────────────────────────

const VALID = /^[a-z0-9._%+-]+@[a-z0-9-]+(?:\.[a-z0-9-]+)+$/;
const LEAD = /^(?:(?:it is|its|the address is|address is|the email is|email address|send it to|email|mail|to|for) )+/;
const SENDING = /^(?:(?:yes|ok|okay|go ahead) )?send(?: (?:it|that|this|the (?:email|mail|message)))?(?: (?:now|off|away))?$/;

/** Said as spoken ("john dot smith at example dot com"): the address it makes, or "" when it is not one. Never guessed: the caller spells it back. */
export function toAddress(heard: string): string {
  let s = ` ${heard.toLowerCase().replace(/[’']/g, "")} `.replace(/[,;!?]/g, " ");
  const at = s.lastIndexOf(" at ");
  if (!s.includes("@") && at >= 0) s = `${s.slice(0, at)} @ ${s.slice(at + 4)}`;
  s = s.replace(/\bat sign\b/g, "@").replace(/\s+/g, " ").trim().replace(LEAD, "");
  s = s
    .replace(/\bdot\b/g, ".")
    .replace(/\bunderscore\b/g, "_")
    .replace(/\b(?:dash|hyphen)\b/g, "-")
    .replace(/\bplus\b/g, "+")
    .replace(/\s+/g, "")
    .replace(/\.+$/, "");
  return VALID.test(s) ? s : "";
}

const KNOWN = new Set("gmail outlook hotmail yahoo icloud live proton protonmail aol msn com org net edu gov io co uk us de fr".split(" "));
const MARK: Record<string, string> = { ".": "dot", "_": "underscore", "-": "dash", "+": "plus" };

/** How an address is read back: the name letter by letter, well-known domain words whole. */
export function spelled(address: string): string {
  const [name, host] = address.split("@");
  const letters = (s: string) => [...s].map((c) => MARK[c] ?? c).join(" ");
  const domain = host
    .split(".")
    .map((label) => (KNOWN.has(label) ? label : letters(label)))
    .join(" dot ");
  return `${letters(name)} at ${domain}`;
}

const cap = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

/** "about the invoice" -> "The invoice". One line, short. */
export function toSubject(heard: string): string {
  const s = heard
    .replace(/\s+/g, " ")
    .trim()
    .replace(/^(?:(?:it'?s|it is|the subject is|subject|about|regarding|re:?) )+/i, "")
    .replace(/[.!]+$/, "");
  return cap(s).slice(0, 120);
}

/** True when what was said is only "send it": voice never does. */
export function isSend(heard: string): boolean {
  return SENDING.test(normalise(heard));
}

const LITERAL = /^(?:word for word|exactly|verbatim|literally)\b[\s,:;-]*/i;

/** The text of the message: "word for word ..." is kept as said; otherwise null (a brain or a template writes it). */
export function literal(heard: string): string | null {
  const m = LITERAL.exec(heard.trim());
  const rest = m ? heard.trim().slice(m[0].length).trim() : "";
  return rest ? cap(rest) : null;
}

/** With no text model at all: the user's own words under a greeting. */
export function template(said: string): string {
  const s = said.trim();
  return `Hello,\n\n${cap(s)}${/[.!?]$/.test(s) ? "" : "."}`;
}

/** What a model wrote, cleaned for an email body: no markdown marks, no quotes around it, no subject line. */
export function tidy(written: string): string {
  return written
    .replace(/^\s*subject:.*\n+/i, "")
    .replace(/[*_`#]+/g, "")
    .trim()
    .replace(/^"([\s\S]*)"$/, "$1")
    .slice(0, 1500)
    .trim();
}

// ── Waiting for a dropped file ────────────────────────────────────────────────

export interface Dropped {
  name: string;
  path: string;
}

/** Where drops are seen: the island's state in the app, a made-up one in a test. */
export interface DropSource {
  current(): Dropped | null;
  subscribe(fn: () => void): () => void;
}

const islandDrops: DropSource = { current: () => State.droppedFile, subscribe: (fn) => State.subscribe(fn) };

/**
 * The file dropped on the island within `ms` of the call, or null. The island shows the dropped path
 * first and swaps in the inbox copy a moment later (the only one the mail window takes): that one is
 * waited for, `settle` ms at most. A file that was already there when the wait began is not taken.
 */
export function waitForDrop(ms: number, source: DropSource = islandDrops, settle = INGEST_MS): Promise<Dropped | null> {
  const before = source.current()?.path;
  return new Promise((resolve) => {
    let seen: string | undefined;
    let late: ReturnType<typeof setTimeout> | undefined;
    const end = (d: Dropped | null) => {
      clearTimeout(timer);
      clearTimeout(late);
      off();
      resolve(d);
    };
    const timer = setTimeout(() => end(null), ms);
    const off = source.subscribe(() => {
      const d = source.current();
      if (!d || d.path === before) return;
      if (seen === undefined) {
        seen = d.path;
        late = setTimeout(() => end(source.current() ?? d), settle);
      } else if (d.path !== seen) end(d);
    });
  });
}

// ── The flow ──────────────────────────────────────────────────────────────────

export interface Draft {
  to: string;
  subject: string;
  message: string;
  /** The dropped file's inbox path, or "". */
  path: string;
}

/** What the flow needs from the app; a test hands in stand-ins. */
export interface MailDeps {
  /** Puts a question (spoken and shown) and answers what was said, or null. */
  ask(question: string, kind: ListenKind): Promise<string | null>;
  /** Says a text; settles when it has been said. */
  say(text: string): Promise<void>;
  /** The file dropped within `ms`, or null. */
  drop(ms: number): Promise<Dropped | null>;
  /** The routed text model's answer, or null when none is on. */
  write(prompt: string): Promise<string | null>;
  /** Opens the mail window with the draft, and nothing more. False when it did not open. */
  open(draft: Draft): Promise<boolean>;
  t(text: string): string;
}

/** Heard "send it" while the details are asked for: the rest is left as it is, and the window opens. */
class SendHeard extends Error {}
class Stopped extends Error {}

const given = (a: VoiceArgs, name: string): string => (typeof a[name] === "string" ? String(a[name]).trim() : "");

/** The address: said, then spelled back and confirmed. Never guessed, never taken unconfirmed. */
async function recipient(a: VoiceArgs, d: MailDeps, hear: (q: string, k: ListenKind) => Promise<string>): Promise<string> {
  let heard = given(a, "to");
  for (let n = 0; n < 2; n++) {
    try {
      if (!heard) heard = await hear(n ? "Say the address again, like name at example dot com." : "Who is it for? Say the email address.", "reply");
      const address = toAddress(heard);
      heard = "";
      if (!address) continue;
      const yes = await hear(`I heard ${spelled(address)}. Is that right?`, "yesNo");
      if (isYes(yes) && !isNo(yes)) return address;
    } catch (err) {
      // "Send it" before there is an address: it cannot, and there is nothing to open yet.
      if (!(err instanceof SendHeard)) throw err;
      heard = "";
      await d.say(d.t(SEND_NOTE));
    }
  }
  return "";
}

/** The message: kept as said when "word for word", else written by the routed model, else the template. */
async function body(said: string, subject: string, d: MailDeps): Promise<string> {
  const exact = literal(said);
  if (exact) return exact;
  const prompt =
    `Write the text of a short, polite email${subject ? ` about "${subject}"` : ""}. The sender wants to say: ${said}\n` +
    "Write only the email text, in the sender's voice, with no subject line.";
  const written = tidy((await d.write(prompt).catch(() => null)) ?? "");
  return written || template(said);
}

/**
 * The whole of it: asks for what is missing (to whom, about what, what to say) in that order, takes a
 * file dropped within 20 s when the user said they want one, and opens the mail window filled in.
 * It never sends: there is no call to `mail_send` anywhere on this path.
 */
export async function prepareMail(a: VoiceArgs, d: MailDeps): Promise<VoiceReply> {
  const draft: Draft = { to: "", subject: "", message: "", path: "" };
  let sendNote = false;
  /** One question; null (silence, "cancel") ends it all; "send it" is never obeyed. */
  const hear = async (question: string, kind: ListenKind): Promise<string> => {
    const said = await d.ask(d.t(question), kind);
    if (said === null) throw new Stopped();
    if (isSend(said)) {
      sendNote = true;
      throw new SendHeard();
    }
    return said;
  };
  let noFile = false;
  try {
    draft.to = await recipient(a, d, hear);
    if (!draft.to) return { say: d.t(NO_ADDRESS), ok: false };
    try {
      const subject = given(a, "subject") || (await hear("What is it about?", "reply"));
      draft.subject = toSubject(subject);
      const said = given(a, "message") || (await hear("What should it say?", "freeText"));
      draft.message = await body(said, draft.subject, d);
      if (a.attach) {
        await d.say(d.t("Drop the file on the island now."));
        const file = await Promise.race([
          d.drop(FILE_MS),
          // Whatever is said meanwhile ends the wait: the user does not want to wait for it.
          d.ask("", "file").then((said) => ((sendNote ||= !!said && isSend(said)), null), () => null),
        ]);
        if (file) draft.path = file.path;
        else noFile = true;
      }
    } catch (err) {
      if (!(err instanceof SendHeard)) throw err;
    }
  } catch (err) {
    if (err instanceof Stopped) return { say: d.t(STOPPED), ok: false };
    throw err;
  }
  if (!(await d.open(draft))) return { say: d.t(OFF), ok: false };
  const lead = sendNote ? SEND_NOTE : noFile ? "No file came, so it is open without one. Press Send when you are ready." : READY;
  return { say: d.t(lead), show: d.t("Email ready") };
}

// ── The action ────────────────────────────────────────────────────────────────

const attach: VoiceArg = {
  name: "attach",
  kind: "choice",
  optional: true,
  choices: () => [{ id: "file", say: ["a file", "a document", "an attachment", "an attached file", "a file attached"] }],
};

const text = (name: string): VoiceArg => ({ name, kind: "text", optional: true });

/** The deny list is in the way of every call: `mail_send` is refused whatever is asked. Only `mail_open` is ever called. */
function chatPlus(v: Pick<VoiceCx, "t">): VoiceCx {
  return voiceCx({
    id: "chat-plus",
    call: (method, args) => Bridge.featureCall("chat-plus", method, args),
    t: v.t,
    island: { open() {} } as never,
  });
}

/** The one call the flow makes on chat-plus: the window, filled in. False when it did not open (email is off). */
export async function openMail(v: Pick<VoiceCx, "t">, draft: Draft): Promise<boolean> {
  const args = { to: draft.to, subject: draft.subject, message: draft.message, ...(draft.path ? { path: draft.path } : {}) };
  return (await chatPlus(v).call("mail_open", args).catch(() => null)) !== null;
}

function realDeps(v: VoiceCx): MailDeps {
  return {
    ask: (question, kind) => turnAsk(question, kind),
    say: (line) => speak(line),
    drop: (ms) => waitForDrop(ms),
    write: async (prompt) => {
      const [{ featureCtx }, { answer }] = await Promise.all([import("../../../core/registry"), import("../brain/route")]);
      const got = await answer(featureCtx("voice"), { text: prompt, history: [], signal: new AbortController().signal }, () => {});
      return got?.text ?? null;
    },
    open: (draft) => openMail(v, draft),
    t: v.t,
  };
}

export const mail: VoiceAction[] = [
  {
    id: "mail_prepare",
    tool: "mail_prepare",
    describe: "Prepare an email in the mail window: it asks for the address, the subject and the text, may take a file dropped on the island, and stops there. It never sends: the user presses Send.",
    forms: [
      "(write|draft|compose|prepare|start) [me] [an|a] (email|mail) [with {attach}] [to {to}] [about {subject}] [saying {message}]",
      "send [an|a] (email|mail) [with {attach}] [to {to}] [about {subject}] [saying {message}]",
      "(email|mail) {to} [about {subject}] [saying {message}]",
    ],
    args: [text("to"), text("subject"), text("message"), attach],
    safety: "prepare",
    run: (a, v) => prepareMail(a, realDeps(v)),
  },
];

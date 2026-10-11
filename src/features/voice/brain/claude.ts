// The online brains (docs/VOICE.md §4.3, §8), each behind its own switch and never implied by a key
// saved for chat: Claude with the user's key ("Voice may use my Claude key": the registry's tools, its
// calls through the same runner and classes, 5 rounds, 24 messages, 90 s; web search is a second
// switch) and a free online model ("Voice may use a free online model"). And chat answers read aloud.
// Package voice-online (VOICE §10).
//
// The rule, in the owner's words: "local first, free api second, paid api optional" and "only if
// chosen by user ... not by default". So `ready` and `ask` both look at the switch: with it off no
// request is ever made, whatever keys exist. Claude only proposes tool calls; `ask.act` (the runner)
// decides and runs them: nothing here touches an action's `run`.
// route.ts is loaded when needed, not at import: it imports this file for `BRAINS`.

import { Bridge, onEvent } from "../../../core/bridge";
import type { FeatureCtx, VoiceEntry } from "../../../core/feature";
import { State } from "../../../core/state";
import type { VoiceSettings } from "../index";
import { speak } from "../runner";
import { needleTools } from "./needle";
import type { Brain, BrainAsk } from "./route";

const ROUNDS = 5;
const MESSAGES = 24;
const CLAUDE_TOKENS = 1024;
const TOOLS_NOTE =
  " You may call the tools to do what the user asks of the app. Say an action worked only if its result says so; a result that asks for a click means the user has to click.";

/** The chat's headless `ask-done` (src-tauri/src/chat/headless.rs). */
interface Done {
  stop: string;
  text: string;
  tools: { id: string; name: string; input: Record<string, unknown> }[];
  messages: unknown[];
}

let count = 0;

/** One `chat.ask`: its words to `on`, its end as the host sent it. Rejects when it failed or was cancelled. */
function askOnce(args: Record<string, unknown>, on: (delta: string) => void, signal: AbortSignal): Promise<Done> {
  if (signal.aborted) return Promise.reject(new Error("aborted"));
  const id = `voice-online-${Date.now().toString(36)}-${count++}`;
  return new Promise<Done>((resolve, reject) => {
    const offs: (() => void)[] = [];
    let over = false;
    const end = (fn: () => void) => {
      if (over) return;
      over = true;
      signal.removeEventListener("abort", cancel);
      for (const off of offs) {
        try {
          off();
        } catch {
          // the listener is already gone
        }
      }
      fn();
    };
    const cancel = () => {
      void Bridge.featureCall("chat", "cancel", { id });
      end(() => reject(new Error("cancelled")));
    };
    signal.addEventListener("abort", cancel, { once: true });
    void (async () => {
      offs.push(
        await onEvent<{ id: string; text: string }>("chat:ask-delta", (e) => e.id === id && !over && on(e.text)),
        await onEvent<Done & { id: string }>("chat:ask-done", (e) => {
          if (e.id !== id) return;
          if (e.stop === "error" || e.stop === "cancel") end(() => reject(new Error(`chat.ask ${e.stop}`)));
          else end(() => resolve({ stop: e.stop, text: e.text ?? "", tools: e.tools ?? [], messages: e.messages ?? [] }));
        }),
      );
      if (over) return;
      void Bridge.featureCall("chat", "ask", { id, stream: true, ...args }).then((r) => r === null && end(() => reject(new Error("chat.ask failed"))));
    })();
  });
}

/** For a test: where the `VoiceCx` maker comes from (the real one is the registry's, which needs the app). */
export const hooks = {
  cxOf: async () => (await import("../registry")).cxOf,
};

async function settingsOf(cx: FeatureCtx): Promise<Partial<VoiceSettings>> {
  return ((await cx.settings.get()) ?? {}) as Partial<VoiceSettings>;
}

const hasKey = async (name: string) => !!(await Bridge.secretPresent(name).catch(() => false));

/** What Claude asked for, run by the runner (`ask.act`): a missing or unknown tool is an error result, never a guess. */
async function result(call: Done["tools"][number], by: Map<string, VoiceEntry>, act: NonNullable<BrainAsk["act"]>) {
  const entry = by.get(call.name);
  if (!entry) return { id: call.id, content: "There is no such tool.", error: true };
  const args: Record<string, string | number> = {};
  for (const [k, v] of Object.entries(call.input ?? {})) if (typeof v === "number" || typeof v === "string") args[k] = v;
  const missing = (entry.action.args ?? []).filter((a) => !a.optional && !(a.name in args)).map((a) => a.name);
  if (missing.length) return { id: call.id, content: `Missing: ${missing.join(", ")}. Ask the user for it.`, error: true };
  try {
    return { id: call.id, content: await act(entry, args) };
  } catch {
    return { id: call.id, content: "It did not work.", error: true };
  }
}

/** Claude through `chat.ask`, with the user's own key. Ready only with its switch on and a key stored. */
export const claude: Brain = {
  id: "claude",
  async ready(cx) {
    return !!(await settingsOf(cx)).claude && (await hasKey("anthropic-api-key"));
  },
  async ask(cx, ask, on) {
    const s = await settingsOf(cx);
    if (!s.claude) throw new Error("Claude is not switched on for voice");
    ask.by?.("Claude");
    const { SYSTEM, messagesFor } = await import("./route");
    const act = ask.act;
    const list = act ? (ask.tools ?? []) : [];
    const by = new Map(list.map((e) => [e.action.tool, e]));
    // `no_action` is the action model's; Claude simply does not call anything.
    const tools = list.length ? (await needleTools(list, await hooks.cxOf())).filter((t) => by.has(t.name)) : [];
    let messages: unknown[] = messagesFor(ask);
    let results: unknown[] | undefined;
    let text = "";
    for (let round = 0; round < ROUNDS; round++) {
      const done = await askOnce(
        {
          provider: "anthropic",
          system: SYSTEM + (tools.length ? TOOLS_NOTE : ""),
          messages,
          max_tokens: CLAUDE_TOKENS,
          ...(results ? { results } : {}),
          ...(tools.length ? { tools } : {}),
          ...(s.claudeSearch ? { web_search: true } : {}),
        },
        (d) => {
          text += d;
          on(d);
        },
        ask.signal,
      );
      if (done.stop !== "tool_use" || !done.tools.length || !act || done.messages.length >= MESSAGES) return text;
      results = [];
      for (const call of done.tools) results.push(await result(call, by, act));
      messages = done.messages;
    }
    return text;
  },
};

/** The providers the free model may use, in order, with the key each needs (the same keys as chat's). */
const FREE: readonly { provider: string; key: string; name: string }[] = [
  { provider: "openrouter-free", key: "openrouter-api-key", name: "OpenRouter's free model" },
  { provider: "google", key: "google-api-key", name: "Google's free tier" },
];

async function freeOnes(): Promise<(typeof FREE)[number][]> {
  const have = await Promise.all(FREE.map((f) => hasKey(f.key)));
  return FREE.filter((_, i) => have[i]);
}

/**
 * A free online model (OpenRouter's free models, Google's free tier). Ready only with its switch on and a
 * key for one of them. Which one is asked goes to `ask.by`, so the caption shows it: it is never said.
 */
export const free: Brain = {
  id: "free",
  async ready(cx) {
    return !!(await settingsOf(cx)).freeModel && (await freeOnes()).length > 0;
  },
  async ask(cx, ask, on) {
    if (!(await settingsOf(cx)).freeModel) throw new Error("the free model is not switched on for voice");
    const { SYSTEM, LIMITS, messagesFor } = await import("./route");
    let last: unknown = new Error("no free model has a key");
    for (const f of await freeOnes()) {
      let said = "";
      ask.by?.(f.name);
      try {
        const done = await askOnce(
          { provider: f.provider, system: SYSTEM, messages: messagesFor(ask), max_tokens: LIMITS.tokens },
          (d) => {
            said += d;
            on(d);
          },
          ask.signal,
        );
        return done.text || said;
      } catch (err) {
        // A provider that failed before saying anything hands over to the next; words already said stand.
        if (said || ask.signal.aborted) throw err;
        last = err;
      }
    }
    throw last;
  },
};

/** "Read chat answers aloud": speaks the chat's answers while the setting is on. From the feature's `start` to the dispose it returns. */
export function startChatAloud(cx: FeatureCtx): () => void {
  const lastOf = () => State.chatHistory[State.chatHistory.length - 1];
  // What is already in the history is not read.
  let seen = lastOf()?.id ?? -1;
  let on = false;
  void settingsOf(cx).then((s) => (on = !!s.chatAloud));
  const off = cx.settings.onChange((v) => (on = !!(v as Partial<VoiceSettings>).chatAloud));
  const unsub = State.subscribe(() => {
    const m = lastOf();
    if (!m || m.id === seen) return;
    seen = m.id;
    if (on && m.role === "assistant") void speak(m.content.slice(0, 1200));
  });
  return () => {
    off();
    unsub();
  };
}

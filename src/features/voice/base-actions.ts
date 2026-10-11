// What can be said to the app itself (docs/VOICE.md §5, the rows "base" and "voice"): open a section,
// close, Settings; jump to the terminal; open an app, search the web, repeat, undo, stop listening for
// the wake phrase; turn a feature on or off. And what is only ever refused (`refusals` of runner.ts:
// allow, deny, answer, send, delete, prompt an agent). The questions answered from kept state are
// answers/, the email is mail/.
// This is the voice feature's own `voice.ts` (see `FeatureModule.voice`), loaded like every other.
// Package voice-runner (VOICE §10).
//
// debt: the pills (add, remove, main, replace, only), the app's sounds and "ask the chat …" are not
// here: each needs a seam the island and the chat view do not have yet (they change the island's own
// state, or fill the chat's field).

import { Bridge } from "../../core/bridge";
import { defineVoice, type VoiceAction, type VoiceArg, type VoiceChoice } from "../../core/feature";
import { FEATURES, featureCtx, featureInfos, featureOn } from "../../core/registry";
import { State } from "../../core/state";
import { t } from "../../i18n/i18n";
import { jumpToSession } from "../../island/agents";
import { answers } from "./answers/index";
import { mail } from "./mail/index";
import { lastSpoken, refusals, undoLast } from "./runner";

const VOICE = "voice";

/** Every section of the features that are on, as `<feature>/<section>`, said by its title. */
const sections = (): VoiceChoice[] =>
  FEATURES.filter((f) => featureOn(f.id)).flatMap((f) => (f.sections ?? []).map((s) => ({ id: `${f.id}/${s.id}`, say: [t(s.title)] })));

const section: VoiceArg = { name: "section", kind: "choice", choices: sections, ask: "Which section?" };

/** The Start Menu's programs as `files.apps` lists them now: the shortcut's path, said by its name. */
const apps: NonNullable<VoiceArg["choices"]> = async (v) =>
  ((await v.call<{ name: string; path: string }[]>("files.apps")) ?? []).map((a) => ({ id: a.path, say: [a.name] }));

const app: VoiceArg = { name: "app", kind: "choice", choices: apps, ask: "Which app?" };

const nameOf = (id: string | number): string => t(featureInfos().find((f) => f.id === id)?.name ?? String(id));

/** The features whose switch is `on`, said by their names. Voice does not switch itself. */
const features = (on: boolean) => (): VoiceChoice[] =>
  featureInfos()
    .filter((f) => f.on === on && f.id !== VOICE)
    .map((f) => ({ id: f.id, say: [t(f.name), f.name] }));

const base: VoiceAction[] = [
  {
    id: "open",
    tool: "island_open",
    describe: "Open a section of the island.",
    forms: ["open [the] {section}", "show [me] [the] {section}", "go to [the] {section}"],
    args: [section],
    safety: "run",
    run: async (a, v) => {
      const [feature, id] = String(a.section).split("/");
      featureCtx(feature).island.open(id);
      return { say: v.t("Here it is.") };
    },
    undo: async () => featureCtx(VOICE).island.close(),
  },
  {
    id: "close",
    tool: "island_close",
    describe: "Fold the island away.",
    forms: ["close [the] island", "close it", "hide [the] island", "fold [the] island"],
    safety: "run",
    run: async () => featureCtx(VOICE).island.close(),
  },
  {
    id: "settings",
    tool: "settings_open",
    describe: "Open the Settings window.",
    forms: ["open [the] settings", "show [me] [the] settings", "settings"],
    safety: "run",
    run: async (_a, v) => {
      await Bridge.openSettingsWindow();
      return { say: v.t("Settings are open.") };
    },
  },
  {
    id: "terminal",
    tool: "agent_terminal",
    describe: "Bring the terminal or app of the coding agent's session to the front.",
    forms: [
      "(jump|go|switch|take me) to [the|my] (terminal|session|agent)",
      "(open|show) [me] [the|my] (terminal|session)",
    ],
    safety: "run",
    // debt: the session in focus is read from the island's state, which has no seam for it yet.
    run: async (_a, v) => (jumpToSession(State.focusTask) ? { say: v.t("Here it is.") } : { say: v.t("No agent session is open."), ok: false }),
  },
  {
    id: "app",
    tool: "app_open",
    describe: "Start a program from the Start Menu.",
    forms: ["open {app}", "launch {app}", "start {app}"],
    args: [app],
    safety: "run",
    // Only a shortcut the list holds right now is opened: never a path, or anything else, taken from
    // what was heard (an online model's call arrives here with whatever it wrote). Rust checks again.
    run: async (a, v) => {
      const listed = (await apps(v)).find((c) => c.id === a.app);
      if (!listed || (await v.call("files.open", { path: listed.id })) === null) return { say: v.t("I couldn't open it."), ok: false };
      return { say: v.t("Opening {name}.", { name: listed.say[0] }) };
    },
  },
  {
    id: "search",
    tool: "web_search",
    describe: "Search the web for something, in the browser.",
    forms: ["search [the web|the internet|online] for {query}", "google {query}", "look up {query}"],
    args: [{ name: "query", kind: "text", ask: "What should I search for?" }],
    safety: "run",
    run: async (a, v) => {
      await Bridge.openUrl(`https://duckduckgo.com/?q=${encodeURIComponent(String(a.query))}`);
      return { say: v.t("It's in your browser.") };
    },
  },
  {
    id: "repeat",
    tool: "voice_repeat",
    describe: "Say the last answer again.",
    forms: ["repeat [that|it]", "say (that|it) again", "what did you say", "come again"],
    safety: "run",
    run: async (_a, v) => ({ say: lastSpoken() || v.t("I haven't said anything yet.") }),
  },
  {
    id: "undo",
    tool: "voice_undo",
    describe: "Take back the last thing voice did.",
    forms: ["undo [that|it]", "take (that|it) back", "revert (that|it)"],
    safety: "run",
    run: async (_a, v) => ((await undoLast()) ? { say: v.t("Undone.") } : { say: v.t("There is nothing to undo."), ok: false }),
  },
  {
    id: "wake-off",
    tool: "voice_wake_off",
    describe: "Stop listening for the wake phrase; the talk key still works.",
    forms: ["(turn|switch) off the wake (phrase|word)", "(turn|switch) the wake (phrase|word) off", "stop listening for the wake (phrase|word)"],
    safety: "run",
    // Off only: opening the microphone for good is a switch the user turns in Settings.
    run: async (_a, v) => {
      await featureCtx(VOICE).settings.patch({ wake: "off" });
      return { say: v.t("I'll only listen when you press the key.") };
    },
  },
  {
    id: "feature-off",
    tool: "feature_off",
    describe: "Turn one of the app's features off.",
    forms: ["(turn|switch) off [the] {feature} [feature]", "(turn|switch) [the] {feature} [feature] off", "disable [the] {feature} [feature]"],
    args: [{ name: "feature", kind: "choice", choices: features(true), ask: "Which feature?" }],
    safety: "confirm",
    confirm: (a) => t("Turn off {name}?", { name: nameOf(a.feature) }),
    run: async (a, v) => {
      try {
        await Bridge.featureSetEnabled(String(a.feature), false);
        return { say: v.t("It's off.") };
      } catch {
        return { say: v.t("I couldn't turn it off."), ok: false };
      }
    },
  },
  {
    id: "feature-on",
    tool: "feature_on",
    describe: "Turn one of the app's features on: it opens Settings, where the user switches it.",
    forms: ["(turn|switch) on [the] {feature} [feature]", "(turn|switch) [the] {feature} [feature] on", "enable [the] {feature} [feature]"],
    args: [{ name: "feature", kind: "choice", choices: features(false), ask: "Which feature?" }],
    // A feature may use the camera, the network, a hook or elevation (§8): its switch is a click.
    safety: "prepare",
    run: async (a, v) => {
      await Bridge.openSettingsWindow();
      return { say: v.t("Settings are open: switch {name} on there.", { name: nameOf(a.feature) }) };
    },
  },
];

export default defineVoice([...refusals(), ...base, ...answers, ...mail]);

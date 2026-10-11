// What a turn shows (docs/VOICE.md §7.5, §7.7): the strip row (the activity `turn`: its mark is the
// Visor, its title "Listening", "Thinking" or the result, its right end the microphone meter, the
// playing bars or Yes), the capsule under the island (a factory window, page `capsule`, placed by
// crates/voice/src/caption.rs under the box `cx.island.shape()` gives), the island's own mascot
// (`cx.island.face`) and the sounds. It draws `turnView()` of ../turn.ts.
// Package voice-ui (VOICE §10). What is shown when, and where, is decided in ./rules.ts (plain
// functions, tests/voice-ui.test.mjs); this file only draws it.
//
// Two pages run this file. The island's page knows the turn: it owns the row and tells the capsule
// what to show and where to be. The capsule's page is a window of its own and knows nothing: it is
// sent a `Caption` (`caption.show` → `voice:caption`) and sends its clicks back (`caption.act` →
// `voice:caption-act`). The window never activates and takes no key (ARCHITECTURE §4.1); it lies
// under the island, or above one dragged to the bottom of the screen, never on it, so never on a card.

import { Bridge, onEvent } from "../../../core/bridge";
import type { ActivityDef, FeatureCtx, IslandShape, VoiceFace } from "../../../core/feature";
import { Sound } from "../../../core/sound";
import { BotEngine, wipe } from "../../../mascot/engine";
import { btn, dotMark, meter, setDotMark, setMeter, stripRow } from "../../../ui/components";
import { icon } from "../../../ui/icons";
import { h } from "../../../views/dom";
import type { VoiceEvents, VoiceSettings, VoiceState } from "../index";
import { isCancel, isEnd } from "../parse/index";
import { speak } from "../runner";
import { endTurn, onTurn, turnView, type TurnView } from "../turn";
import {
  CAPSULE, THINK_MS, capsuleFollows, capsuleRect, capsuleShown, captionFor, emoteFor, ending, faceFor, levelFor, rowActive, rowFor, soundFor,
  type Caption, type Origin,
} from "./rules";

const ID = "turn";
/** The Visor on the strip, logical pixels across. */
const D = 22;
/** A rounded square: the stop button's icon, this feature's own. */
const STOP = "M8 7h8a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H8a1 1 0 0 1-1-1V8a1 1 0 0 1 1-1z";

// ── The island's page ─────────────────────────────────────────────────────────

/** The feature's context from `startUi` to its dispose: nothing below runs without it. */
let app: FeatureCtx | null = null;
let level = { mic: 0, out: 0 };
/** Settings › Mascot: hidden, the mark is the dot with its ring. */
let mascot = true;
let captions = true;
/** The capsule was opened to its large size by a click; folded again with each new turn. */
let opened = false;

let was: VoiceState = "off";
let heard = "";
/** What the view held when the last sentence was taken: how the turn ended is read against it. */
let taken: Pick<TurnView, "answer" | "spoken" | "result"> | null = null;
let thinking: number | undefined;
let rowWas = "";
let activeWas = false;

// The mark: one small mascot, or its dot, kept across the row's redraws.
let visor: { el: HTMLElement; canvas: HTMLCanvasElement; engine: BotEngine } | null = null;
let dot: { el: HTMLElement; mark: HTMLElement } | null = null;
let mic: HTMLElement | null = null;
let frame = 0;
let frameAt = 0;
/** The island's mascot wears a face of ours. */
let worn = false;

/** Draws the Visor while its row is on the strip; `render` starts it, and it ends by itself. */
function draw(now: number): void {
  frame = 0;
  const v = visor;
  if (!v || !v.canvas.isConnected || !rowActive(turnView())) return;
  const ctx = v.canvas.getContext("2d");
  if (ctx) {
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    v.engine.update(Math.min(0.1, (now - frameAt) / 1000));
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    wipe(ctx);
    v.engine.draw(ctx, D, D, D);
  }
  frameAt = now;
  frame = requestAnimationFrame(draw);
}

function mark(): HTMLElement {
  if (!mascot) {
    if (!dot) {
      const m = dotMark("", "idle");
      dot = { el: h("span", { class: "mark-slot f-voice-visor" }, m), mark: m };
    }
    return dot.el;
  }
  if (!visor) {
    // As a mini mascot (src/mascot/minibots.ts): the slot is its size, the canvas twice it, so the glow of the bar is not cut.
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const canvas = h("canvas", { width: Math.round(D * 2 * dpr), height: Math.round(D * 2 * dpr), style: `width:${D * 2}px;height:${D * 2}px` });
    const engine = new BotEngine("idle");
    engine.isMini = true;
    visor = { el: h("span", { class: "mini f-voice-visor" }, canvas), canvas, engine };
  }
  if (!frame) {
    frameAt = performance.now();
    frame = requestAnimationFrame(draw);
  }
  return visor.el;
}

/** The face and its level on the row's mark. */
function markFace(state: VoiceState): void {
  const face = faceFor(state);
  const lv = levelFor(state, level);
  if (visor) visor.engine.voice = face ? { face, level: lv } : null;
  if (dot) setDotMark(dot.mark, "", face === "asking" ? "question" : (face ?? "idle"), lv);
}

/** The same on the island's own mascot too, which is asked nothing outside a turn. */
function wear(state: VoiceState): void {
  const face = faceFor(state);
  markFace(state);
  if (face || worn) app?.island.face(face, levelFor(state, level));
  worn = face !== null;
}

function render(): HTMLElement {
  const view = turnView();
  const t = app?.t ?? ((s: string) => s);
  const r = rowFor(view, t);
  const el = mark();
  // Not a mark the island carries to a section when it opens (a canvas cannot be copied).
  el.dataset.shared = "visor";
  const confirm = view.confirm;
  const row = stripRow({
    mark: el,
    title: r.title,
    detail: r.detail || undefined,
    bars: r.right === "bars" || undefined,
    action: r.right === "yes" && confirm ? { label: t("Yes"), run: () => confirm.yes() } : undefined,
  });
  mic = r.right === "meter" ? meter(level.mic) : null;
  if (mic) {
    // Shorter than a level's meter (120 px), so the words heard keep the room.
    mic.style.width = "64px";
    row.append(mic);
  }
  // The mark only: the island is drawing its strip right now, and is told of a face when the state changes.
  markFace(view.state);
  return row;
}

/** The strip row of a turn: order −1, it holds the strip for as long as the turn lasts. A card that waits still has the island (the arbiter's rule). */
export const turnActivity: ActivityDef = {
  id: ID,
  order: -1,
  hold: true,
  isActive: () => app !== null && rowActive(turnView()),
  render,
};

// The ears send `{mic}` alone, the mouth `{out}` alone. Nothing is drawn again: the meter and the face move.
function onLevel(l: Partial<VoiceEvents["level"]>): void {
  level = { ...level, ...l };
  if (mic) setMeter(mic, level.mic);
  const state = turnView().state;
  if (faceFor(state)) wear(state);
}

// ── The capsule, from the island's page ───────────────────────────────────────

/** The capsule is on screen, or on its way there. */
let shown = false;
let sized = "";
let placed = "";
let leaving: number | undefined;
/** The capsule left with the island while its captions were only staying: it does not come back for that turn. */
let gone = false;
/** One call to the window at a time, in the order they were asked. */
let chain: Promise<unknown> = Promise.resolve();

function queue(job: (c: FeatureCtx) => Promise<unknown>): void {
  const c = app;
  if (!c) return;
  chain = chain.then(() => (app === c ? job(c) : undefined)).catch(() => {});
}

/** Puts the capsule where the island's box says, or away when there is no room that is not the island's. */
function place(size: { w: number; h: number }): void {
  queue(async (c) => {
    const origin = await c.call<Origin>("caption.origin");
    if (!shown || app !== c) return;
    const rect = origin && capsuleRect(c.island.shape(), origin, size);
    const key = JSON.stringify(rect);
    if (key === placed) return;
    placed = key;
    await (rect ? c.call("caption.place", rect) : c.call("caption.hide"));
  });
}

function leave(atOnce: boolean): void {
  const c = app;
  if (!c || !shown) return;
  shown = false;
  sized = placed = "";
  c.island.shape(null);
  // A turn that began meanwhile keeps the window.
  const hide = () => queue(async (c) => (shown ? undefined : c.call("caption.hide")));
  window.clearTimeout(leaving);
  if (atOnce) return hide();
  void c.call("caption.show", { out: true });
  leaving = window.setTimeout(hide, CAPSULE.outMs);
}

function onShape(shape: IslandShape): void {
  if (!shown) return;
  if (!capsuleFollows(turnView().state, shape)) {
    gone = true;
    return leave(false);
  }
  sized = "";
  caption(turnView());
}

function caption(view: TurnView): void {
  const c = app;
  if (!c) return;
  if (view.state !== "closing") gone = false;
  if (gone) return;
  if (!capsuleShown(view)) return leave(view.state === "off" || view.state === "paused");
  window.clearTimeout(leaving);
  if (!shown) {
    shown = true;
    opened = false;
    // debt: the box is asked about during a turn only; an island dragged in the middle of one is followed at the next change.
    c.island.shape({ changed: onShape });
  }
  const cap = captionFor(view, { captions, open: opened });
  void c.call("caption.show", cap);
  const size = `${cap.w}x${cap.h}`;
  if (size === sized) return;
  sized = size;
  place(cap);
}

/** A click in the capsule's window, or the settings page's "Try the voice". Never an answer to anything: stop, undo, open, try. */
function onAct(a: { what?: string }): void {
  const view = turnView();
  switch (a.what) {
    case "ready":
      // The window has just been made: it missed what it was sent.
      if (shown) void app?.call("caption.show", captionFor(view, { captions, open: opened }));
      break;
    case "stop":
      endTurn();
      break;
    case "undo":
      view.undo?.();
      break;
    case "open":
      opened = !opened;
      if (shown) caption(view);
      break;
    case "try":
      // Through the one mouth, as any answer: the row and the capsule show it too.
      if (app && !capsuleShown(view)) void speak(app.t("Hello. This is the voice I answer with."));
      break;
  }
}

// ── Following the turn ────────────────────────────────────────────────────────

function sync(view: TurnView): void {
  const c = app;
  if (!c) return;
  const state = view.state;
  const words = `${view.final}~${view.heard}`;
  if (words !== heard) {
    heard = words;
    if (view.final) taken = { answer: view.answer, spoken: view.spoken, result: view.result };
  }
  if (state !== was) {
    const end = state === "closing" ? ending(view, taken, isCancel(view.heard) || isEnd(view.heard)) : "nothing";
    const sound = soundFor(was, state, end);
    if (sound) Sound.play(sound);
    window.clearTimeout(thinking);
    if (state === "thinking") thinking = window.setTimeout(() => Sound.play("think"), THINK_MS);
    if (!faceFor(state)) level = { mic: 0, out: 0 };
    wear(state);
    const emote: VoiceFace | null = state === "closing" ? emoteFor(end) : null;
    if (emote) c.island.face(emote);
    was = state;
  }
  const active = rowActive(view);
  const key = active ? JSON.stringify([state, rowFor(view)]) : "";
  // Coming and going slide the strip out; a word more is drawn where the row stands.
  if (active !== activeWas) c.island.activityChanged(ID);
  else if (key !== rowWas) c.island.redraw(ID);
  activeWas = active;
  rowWas = key;
  caption(view);
}

/** Follows the turn (row, capsule, face, sounds) from the feature's `start` to the dispose it returns. */
export function startUi(cx: FeatureCtx): () => void {
  app = cx;
  was = turnView().state;
  const live = () => app === cx;
  const setCaptions = (s: Partial<VoiceSettings>) => {
    captions = s.captions !== false;
    if (shown) caption(turnView());
  };
  let offMascot = () => {};
  const offs = [
    onTurn(sync),
    cx.on<Partial<VoiceEvents["level"]>>("level", onLevel),
    cx.on<{ what?: string }>("caption-act", onAct),
    cx.settings.onChange((s) => setCaptions(s as Partial<VoiceSettings>)),
  ];
  void cx.settings.get().then((s) => live() && setCaptions(s as Partial<VoiceSettings>));
  void Bridge.boot().then((boot) => {
    if (live() && boot) mascot = boot.settings.showMascot !== false;
  });
  void onEvent<{ showMascot?: boolean }>("settings-changed", (s) => (mascot = s.showMascot !== false)).then((off) => (live() ? (offMascot = off) : off()));
  return () => {
    for (const off of offs) off();
    offMascot();
    window.clearTimeout(thinking);
    window.clearTimeout(leaving);
    cancelAnimationFrame(frame);
    cx.island.face(null);
    cx.island.shape(null);
    // Rust closes the window when the feature stops; while it only restarts, the capsule is put away.
    void cx.call("caption.hide");
    app = null;
    visor = dot = mic = taken = null;
    frame = 0;
    level = { mic: 0, out: 0 };
    shown = opened = activeWas = worn = gone = false;
    sized = placed = rowWas = heard = "";
    was = "off";
  };
}

// ── The capsule's own page ────────────────────────────────────────────────────

/** The capsule's page: `window.html?feature=voice&page=capsule`. It draws what it is sent and reports its clicks; it reads no key. */
export function capsulePage(root: HTMLElement, cx: FeatureCtx): void {
  const act = (what: string) => (e: Event) => {
    e.stopPropagation();
    void cx.call("caption.act", { what });
  };
  const heardEl = h("div", { class: "f-voice-cap-heard" });
  const answer = h("div", { class: "f-voice-cap-answer" });
  const undo = btn(cx.t("Undo"), "ghost", () => {});
  undo.classList.add("sm");
  undo.addEventListener("click", act("undo"));
  // No `title`: a tooltip has no room in a window this small.
  const stop = h("button", { class: "btn ghost sm icon", "aria-label": cx.t("Stop"), onclick: act("stop") }, icon(STOP, 14));
  const el = h(
    "div",
    { class: "f-voice-cap", onclick: act("open") },
    h("i", { class: "f-voice-cap-dot" }),
    h("div", { class: "f-voice-cap-text" }, heardEl, answer),
    undo,
    stop,
  );
  let out = true;
  cx.on<Caption | { out: true }>("caption", (c) => {
    if ("out" in c) {
      out = true;
      // Out in 220 ms: the fade of a notice. Nothing pulses in a window that is put away.
      el.classList.remove("fx-drop");
      el.classList.remove("mic");
      el.classList.add("fx-out");
      return;
    }
    if (out) {
      out = false;
      el.classList.remove("fx-out");
      // In 120 ms, as a notice comes in; the reflow starts the animation again.
      void el.offsetWidth;
      el.classList.add("fx-drop");
      if (!el.isConnected) root.append(el);
    }
    el.classList.toggle("mic", c.mic);
    el.classList.toggle("bare", c.bare);
    el.classList.toggle("open", c.open);
    el.style.setProperty("--lines", String(c.lines - (c.heard ? 1 : 0)));
    heardEl.textContent = c.heard || (c.mic && !c.answer.length ? cx.t("Listening…") : "");
    answer.replaceChildren(...c.answer.map((a) => h("span", { class: a.at, text: `${a.text} ` })));
    undo.style.display = c.undo ? "" : "none";
    if (c.open) answer.querySelector(".now")?.scrollIntoView({ block: "nearest" });
  });
  void cx.call("caption.act", { what: "ready" });
}

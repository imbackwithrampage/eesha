// The product's one tooltip (docs/ARCHITECTURE.md §7). Every `title` on a page that called
// `installTooltip` is taken over: its text moves to `data-tip`, so the system's box never shows,
// and comes up as a small panel of the design language (`.tip`, components.css) once the pointer
// has rested on the control. Nothing runs while no hinted control is under the pointer: the one
// timer is the rest of the pointer before the first hint.
//
// The rules (`placeTip`, `splitTip`, `TipTimer`) need no page: tests/tooltip.test.mjs.

import { h } from "../views/dom";
import { kbd } from "./components";

const TIP = "data-tip";
/** The pointer rests this long before the first hint… */
export const REST_MS = 400;
/** …and for this long after a hint was up, the next control's comes at once. */
export const WARM_MS = 500;
/** Between a control and its hint. */
const GAP = 6;
/** Kept from the page's edge. */
const EDGE = 4;
/** What the floating shadow wants beside and below the hint before the page ends. */
const SHADOW = 24;

export interface Box { left: number; top: number; right: number; bottom: number }
export interface Size { w: number; h: number }
export interface Placed { x: number; y: number; side: "below" | "above" | "right" | "left"; shadow: boolean }

/**
 * Where the hint of the control `at` goes on a page of `page`: under it, else over it, else beside
 * it, on the first side that holds all of it, slid along that side to stay inside. Null when no
 * side has the room: it is then not shown rather than shown cut (the island's window is small).
 */
export function placeTip(at: Box, tip: Size, page: Size): Placed | null {
  const maxX = page.w - EDGE - tip.w;
  const maxY = page.h - EDGE - tip.h;
  if (maxX < EDGE || maxY < EDGE) return null;
  const within = (v: number, max: number) => Math.round(Math.max(EDGE, Math.min(max, v)));
  const cx = within((at.left + at.right) / 2 - tip.w / 2, maxX);
  const cy = within((at.top + at.bottom) / 2 - tip.h / 2, maxY);
  const sides: [Placed["side"], number, number][] = [
    ["below", cx, Math.round(at.bottom + GAP)],
    ["above", cx, Math.round(at.top - GAP - tip.h)],
    ["right", Math.round(at.right + GAP), cy],
    ["left", Math.round(at.left - GAP - tip.w), cy],
  ];
  for (const [side, x, y] of sides) {
    if (x < EDGE || x > maxX || y < EDGE || y > maxY) continue;
    return { x, y, side, shadow: x >= SHADOW && x <= page.w - tip.w - SHADOW && y <= page.h - tip.h - SHADOW };
  }
  return null;
}

// ponytail: English key names only; a hint written in another language's keys ("Strg+K") stays plain text.
const KEYS = /^(?:(?:Ctrl|Alt|Shift|Win)\+)*(?:F\d{1,2}|Esc|Enter|Tab|Space|Del|Delete|Backspace|Home|End|PgUp|PgDn|Up|Down|Left|Right|[A-Z0-9]|[↑↓←→,./;'[\]\\=-])$/;

/** "Fold (Esc)" is the words "Fold" and the key Esc; "All sections (Ctrl+K)" has two keys. Each is drawn as a key cap. */
export function splitTip(text: string): { label: string; keys: string[] } {
  const m = /^(.*\S)\s*\(([^()]+)\)$/.exec(text.trim());
  return m && KEYS.test(m[2]) ? { label: m[1], keys: m[2].split("+") } : { label: text, keys: [] };
}

/**
 * When a hint comes up and what puts it away. `show` draws the hint of a control and says whether
 * it had room; `hide` takes it away at once.
 */
export class TipTimer<T> {
  /** The control whose hint is up. */
  shown: T | null = null;
  /** The hinted control under the pointer. */
  private at: T | null = null;
  private timer: number | undefined;
  private warm = -Infinity;
  private show: (el: T) => boolean;
  private hide: () => void;
  private now: () => number;

  constructor(show: (el: T) => boolean, hide: () => void, now: () => number = () => performance.now()) {
    this.show = show;
    this.hide = hide;
    this.now = now;
  }

  /** A timer is running: the pointer rests on a control whose hint is not up yet. */
  get waiting(): boolean {
    return this.timer !== undefined;
  }

  /** The pointer is on `el`, or on nothing hinted (null). */
  over(el: T | null) {
    if (el === this.at) return;
    this.away();
    this.at = el;
    if (el == null) return;
    const up = () => {
      this.timer = undefined;
      if (this.show(el)) this.shown = el;
    };
    if (this.now() - this.warm < WARM_MS) up();
    else this.timer = window.setTimeout(up, REST_MS);
  }

  /** A press, a key, a scroll: away at once, and not back until the pointer finds another control. */
  away() {
    window.clearTimeout(this.timer);
    this.timer = undefined;
    if (this.shown == null) return;
    this.shown = null;
    this.warm = this.now();
    this.hide();
  }
}

// ── The page ──────────────────────────────────────────────────────────────────

let rules: TipTimer<Element> | null = null;

/** Puts the hint away: what it pointed at is leaving (the island folds, its pointer is gone). */
export function hideTip() {
  rules?.over(null);
}

/**
 * Takes a control's `title` over; false when it had none. The control keeps a name: one that had
 * none but its title, or the one this gave it before, gets the new text as its `aria-label`.
 */
function adopt(el: Element): boolean {
  const text = el.getAttribute("title");
  if (text == null) return false;
  const old = el.getAttribute(TIP);
  const label = el.getAttribute("aria-label");
  el.removeAttribute("title");
  if (text) el.setAttribute(TIP, text);
  else el.removeAttribute(TIP);
  if (text && (label == null ? !el.textContent?.trim() : label === old)) el.setAttribute("aria-label", text);
  return true;
}

function draw(tip: HTMLElement, el: Element): boolean {
  const text = el.getAttribute(TIP);
  if (!text || !el.isConnected) return false;
  // A hint that only repeats what the control shows whole says nothing, and it would lie over what
  // is under the control (a question's own words over its answers). Text that is cut keeps its hint.
  const cut = el.scrollWidth > el.clientWidth || el.scrollHeight > el.clientHeight;
  if (!cut && text.trim() === (el as HTMLElement).innerText?.trim()) return false;
  const { label, keys } = splitTip(text);
  tip.replaceChildren(label, ...keys.map(kbd));
  tip.classList.remove("on");
  if (!tip.isConnected) document.body.append(tip);
  const size = tip.getBoundingClientRect();
  const at = placeTip(el.getBoundingClientRect(), { w: size.width, h: size.height }, { w: window.innerWidth, h: window.innerHeight });
  if (!at) return false;
  tip.style.left = `${at.x}px`;
  tip.style.top = `${at.y}px`;
  tip.classList.toggle("flat", !at.shadow);
  // Laid out once in its hidden look, so the fade and the rise start from there.
  void tip.offsetWidth;
  tip.classList.add("on");
  return true;
}

/** Once per page, by the code that sets the page up. */
export function installTooltip() {
  if (rules) return;
  const tip = h("div", { class: "tip", "aria-hidden": "true" });
  const r = (rules = new TipTimer<Element>((el) => draw(tip, el), () => tip.classList.remove("on")));
  const hinted = (target: EventTarget | null) => {
    const el = (target as Element | null)?.closest?.(`[title], [${TIP}]`) ?? null;
    if (el) adopt(el);
    return el?.hasAttribute(TIP) ? el : null;
  };
  document.addEventListener("pointerover", (e) => r.over(e.pointerType === "touch" ? null : hinted(e.target)), true);
  document.documentElement.addEventListener("pointerleave", () => r.over(null));
  window.addEventListener("blur", () => r.over(null));
  for (const type of ["pointerdown", "keydown", "wheel", "scroll"]) {
    window.addEventListener(type, () => r.away(), { capture: true, passive: true });
  }
  // A title written on a control later (a label that follows a state, a new language) is taken
  // over as it is written: told by the browser, nothing is watched on a timer.
  new MutationObserver((changes) => {
    for (const { target } of changes) {
      const el = target as Element;
      const was = el.getAttribute(TIP);
      // The hint that is up follows its text when that changes (the same text written again is
      // nothing, or a row redrawn on every sync would blink its hint); one that lost it goes.
      if (adopt(el) && el === r.shown && el.getAttribute(TIP) !== was && !draw(tip, el)) r.away();
    }
  }).observe(document.documentElement, { attributes: true, subtree: true, attributeFilter: ["title"] });
}

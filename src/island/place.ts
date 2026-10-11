// Where the island lives once it has been dragged (docs/ARCHITECTURE.md §4):
// the deciding rules only, no DOM and no bridge (tests/island-place.test.mjs).
// The behaviour follows upstream pull request 428
// (`windows/src-tauri/src/island/placement.rs`); the code is written anew.

import { EXPANDED_W, SECTION_MAX_H } from "../core/layout";

/** A display in screen pixels, with its work area (the taskbar left out). `key` is what Rust remembers it by. */
export interface Display {
  key: string;
  x: number;
  y: number;
  w: number;
  h: number;
  wx: number;
  wy: number;
  ww: number;
  wh: number;
  scale: number;
}

/** What is saved: the island's top-centre point, in logical pixels from its display's corner. */
export interface Spot {
  display: string;
  x: number;
  y: number;
  docked: boolean;
}

/** Where that is now, in screen pixels. */
export interface Place {
  x: number;
  y: number;
  scale: number;
  floating: boolean;
}

/** The pointer must travel further than this with the button held before a press is a drag. */
export const DRAG_MIN = 6;
/** Let go this close to a display's top edge and the island docks there. */
export const DOCK_WITHIN = 24;
/** Docked this close to the middle of the edge, it is the middle. */
export const SNAP_WITHIN = 24;
/** Room above a floating island inside its window, for its shadow. */
export const FLOAT_TOP = 12;
/** The hidden state of a floating island. Same numbers in src/style.css and src-tauri/src/island.rs. */
export const PILL_W = 72;
export const PILL_H = 14;

const HALF = EXPANDED_W / 2;

export function dragStarted(dx: number, dy: number): boolean {
  return Math.hypot(dx, dy) > DRAG_MIN;
}

/** The display a point is on, or the nearest one: a drop is never over nothing. */
export function displayAt(x: number, y: number, displays: Display[]): Display | undefined {
  let best: Display | undefined;
  let near = Infinity;
  for (const d of displays) {
    const far = Math.hypot(Math.max(d.x - x, 0, x - (d.x + d.w - 1)), Math.max(d.y - y, 0, y - (d.y + d.h - 1)));
    if (far < near) [best, near] = [d, far];
  }
  return best;
}

/** A display found again after the layout, the resolution or the numbering changed; undefined when it is gone. */
export function findDisplay(key: string, displays: Display[]): Display | undefined {
  const [at, name, size] = key.split("|");
  const part = (d: Display, i: number) => d.key.split("|")[i];
  const named = displays.filter((d) => part(d, 1) === name);
  return (
    displays.find((d) => d.key === key) ??
    displays.find((d) => part(d, 0) === at && part(d, 1) === name) ??
    displays.find((d) => part(d, 1) === name && part(d, 2) === size) ??
    displays.find((d) => part(d, 0) === at && part(d, 2) === size) ??
    (named.length === 1 ? named[0] : undefined)
  );
}

/** Between two limits; a span too small for the island centres it. */
function between(v: number, lo: number, hi: number): number {
  return hi < lo ? (lo + hi) / 2 : Math.min(hi, Math.max(lo, v));
}

/** The spot with the whole open island on its display: on the top edge when docked, in the work area when floating. */
export function kept(spot: Spot, d: Display): Spot {
  const s = d.scale;
  if (spot.docked) return { ...spot, x: between(spot.x, HALF, d.w / s - HALF), y: 0 };
  const left = (d.wx - d.x) / s;
  const top = (d.wy - d.y) / s;
  return {
    ...spot,
    x: between(spot.x, left + HALF, left + d.ww / s - HALF),
    y: between(spot.y, top, top + d.wh / s - SECTION_MAX_H),
  };
}

/** The island was let go with its top-centre point at (x, y), screen pixels: docked or floating, and where. */
export function settle(x: number, y: number, displays: Display[]): Spot | null {
  const d = displayAt(x, y, displays);
  if (!d) return null;
  const lx = (x - d.x) / d.scale;
  const ly = (y - d.y) / d.scale;
  const mid = d.w / d.scale / 2;
  const docked = ly <= DOCK_WITHIN;
  return kept({ display: d.key, x: docked && Math.abs(lx - mid) <= SNAP_WITHIN ? mid : lx, y: ly, docked }, d);
}

/** Where a saved spot is on the displays there are now; null when its display is gone (the top centre of the main one). */
export function resolve(spot: Spot | null | undefined, displays: Display[]): Place | null {
  if (!spot || typeof spot.display !== "string" || !Number.isFinite(spot.x) || !Number.isFinite(spot.y)) return null;
  const d = findDisplay(spot.display, displays);
  if (!d) return null;
  const s = kept(spot, d);
  return { x: Math.round(d.x + s.x * d.scale), y: Math.round(d.y + s.y * d.scale), scale: d.scale, floating: !s.docked };
}

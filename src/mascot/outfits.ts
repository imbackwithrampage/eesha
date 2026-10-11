// The wardrobe: six outfits for the Visor, drawn as vector paths in the engine's own
// construction (round caps and joins, the orb's top-lit gradient, the token colours) and
// layered over `BotEngine.draw`. Original drawings; nothing here is a bitmap.
//
// An outfit is drawn after the body, in the body's frame: `poseOf` carries the bob, the
// hop, the lean and the shake of the engine across, so a hat stays on the head.
// debt: this reads the engine's private fields (`t`, `w`, `lx`, `emoteT`); a hook in
// the engine that draws the outfit inside its own transform would remove `poseOf`.

import type { BotEngine } from "./engine";

export type OutfitId = "antenna" | "headphones" | "sprout" | "sunhat" | "woolly" | "leaf";
/** What the setting stores: an outfit, no outfit, or the one of the season. */
export type OutfitSetting = OutfitId | "none" | "auto";

/** The shelf, in order. Labels are English keys for `t()`. */
export const OUTFITS: readonly { id: OutfitId; label: string }[] = [
  { id: "antenna", label: "Antenna" },
  { id: "headphones", label: "Headset" },
  { id: "sprout", label: "Sprout" },
  { id: "sunhat", label: "Sun hat" },
  { id: "woolly", label: "Woolly cap" },
  { id: "leaf", label: "Maple leaf" },
];

/** Auto: the cap in winter, the sprout in spring, the sun hat in summer, the leaf in autumn. */
export function seasonOutfit(date = new Date()): OutfitId {
  const m = date.getMonth(); // 0 = January
  if (m === 11 || m < 2) return "woolly";
  if (m < 5) return "sprout";
  if (m < 8) return "sunhat";
  return "leaf";
}

export function resolveOutfit(setting: string, date?: Date): OutfitId | null {
  if (setting === "auto") return seasonOutfit(date);
  return OUTFITS.some((o) => o.id === setting) ? (setting as OutfitId) : null;
}

const TAU = Math.PI * 2;
const REDUCED = typeof matchMedia === "function" ? matchMedia("(prefers-reduced-motion: reduce)") : null;
const unit = (v: number) => Math.min(1, Math.max(0, v));

// ── Colours: the tokens, read once ────────────────────────────────────────────

interface Ink { bg: string; ink: string; ink2: string; card2: string; accent: string; ok: string; warn: string; bad: string; info: string }
let ink: Ink | null = null;
export function inkTokens(): Ink {
  if (ink) return ink;
  const css = getComputedStyle(document.documentElement);
  const v = (n: string, f: string) => css.getPropertyValue(n).trim() || f;
  return (ink = {
    bg: v("--bg", "#000"), ink: v("--ink", "#f5f6f8"), ink2: v("--ink-2", "#c9cdd4"), card2: v("--card-2", "#22252c"),
    accent: v("--accent", "#8ea2ff"), ok: v("--ok", "#34d399"), warn: v("--warn", "#f5a524"), bad: v("--bad", "#f4505e"), info: v("--info", "#22d3ee"),
  });
}
/** `pct` % of `b` mixed into `a`. */
export const mix = (a: string, b: string, pct: number) => `color-mix(in srgb, ${a}, ${b} ${pct}%)`;

// ── The engine's pose ─────────────────────────────────────────────────────────

/** Where the engine puts the body this frame, so the outfit can follow it. */
export function poseOf(e: BotEngine, d: number) {
  const u = d / 58;
  const w = e["w"];
  const M = REDUCED?.matches ? 0 : 1;
  const em = e["emoteT"];
  const bob = M * Math.sin((e.t * TAU) / 2.8) * 1.5 * u * (1 - w.sleeping) * (1 - 0.6 * w.ratelimit);
  const hop = M * u * (w.surprised * 6 * Math.sin(unit(em / 0.42) * Math.PI) + w.happy * 4 * Math.abs(Math.sin(unit(em / 0.84) * TAU)));
  const lean =
    e["lx"] * 8 * (1 - w.sleeping) - w.proud * 6 - w.question * 6 + M * w.dizzy * 7 * Math.sin((e.t * TAU) / 1.1);
  return { dx: (e.ox * d) / 2, dy: bob - hop + w.ratelimit * u, rotate: (lean * Math.PI) / 180 };
}

// ── Drawing ───────────────────────────────────────────────────────────────────

type Ctx = CanvasRenderingContext2D;

/** The orb's own light: lit from above, a third darker below. */
function shade(ctx: Ctx, y0: number, y1: number, base: string) {
  const g = ctx.createLinearGradient(0, y0, 0, y1);
  g.addColorStop(0, mix(base, "white", 30));
  g.addColorStop(0.55, base);
  g.addColorStop(1, mix(base, inkTokens().bg, 32));
  return g;
}

/** Fills the current path with the orb's light and a soft outline. */
function solid(ctx: Ctx, d: number, y0: number, y1: number, base: string) {
  ctx.fillStyle = shade(ctx, y0 * d, y1 * d, base);
  ctx.fill();
  ctx.lineWidth = Math.max(1, d * 0.022);
  ctx.strokeStyle = mix(base, inkTokens().bg, 62);
  ctx.stroke();
}

/** A leaf along +x from the origin: pointed at both ends, fuller on top. */
function leafPath(ctx: Ctx, len: number, wid: number) {
  ctx.beginPath();
  ctx.moveTo(0, 0);
  ctx.bezierCurveTo(len * 0.25, -wid * 1.15, len * 0.75, -wid * 0.9, len, 0);
  ctx.bezierCurveTo(len * 0.7, wid * 0.7, len * 0.25, wid * 0.8, 0, 0);
  ctx.closePath();
}

const DRAW: Record<OutfitId, (ctx: Ctx, d: number, c: Ink) => void> = {
  // A short mast with a lit tip.
  antenna(ctx, d, c) {
    ctx.strokeStyle = mix(c.ink2, c.bg, 40);
    ctx.lineWidth = Math.max(1.25, d * 0.03);
    ctx.beginPath();
    ctx.moveTo(0, -0.44 * d);
    ctx.quadraticCurveTo(-0.06 * d, -0.58 * d, 0.04 * d, -0.73 * d);
    ctx.stroke();
    ctx.beginPath();
    ctx.roundRect(-0.08 * d, -0.5 * d, 0.16 * d, 0.075 * d, 0.035 * d);
    solid(ctx, d, -0.5, -0.425, mix(c.card2, c.ink2, 45));
    ctx.save();
    ctx.shadowColor = mix(c.accent, "transparent", 25);
    ctx.shadowBlur = Math.max(3, d * 0.16);
    ctx.fillStyle = c.accent;
    ctx.beginPath();
    ctx.arc(0.04 * d, -0.77 * d, Math.max(2.2, d * 0.07), 0, TAU);
    ctx.fill();
    ctx.restore();
  },

  // A band over the top and a cup on each side.
  headphones(ctx, d, c) {
    ctx.lineCap = "round";
    ctx.strokeStyle = mix(c.ink2, c.bg, 40);
    ctx.lineWidth = Math.max(2, d * 0.058);
    ctx.beginPath();
    ctx.arc(0, 0, 0.545 * d, Math.PI + 0.2, TAU - 0.2);
    ctx.stroke();
    for (const s of [-1, 1]) {
      ctx.beginPath();
      ctx.roundRect(s * 0.515 * d - 0.08 * d, -0.1 * d, 0.16 * d, 0.32 * d, 0.065 * d);
      solid(ctx, d, -0.1, 0.22, c.accent);
    }
  },

  // Two leaves on a short stem.
  sprout(ctx, d, c) {
    ctx.strokeStyle = mix(c.ok, c.bg, 25);
    ctx.lineWidth = Math.max(1.4, d * 0.036);
    ctx.beginPath();
    ctx.moveTo(0, -0.43 * d);
    ctx.quadraticCurveTo(0.01 * d, -0.52 * d, 0, -0.6 * d);
    ctx.stroke();
    for (const [a, len, wid] of [[-Math.PI + 0.62, 0.27, 0.095], [-0.62, 0.31, 0.11]] as const) {
      ctx.save();
      ctx.translate(0, -0.6 * d);
      ctx.rotate(a);
      leafPath(ctx, len * d, wid * d);
      solid(ctx, d, -0.1, 0.1, c.ok);
      ctx.strokeStyle = mix(c.ok, c.bg, 38);
      ctx.lineWidth = Math.max(1, d * 0.014);
      ctx.beginPath();
      ctx.moveTo(len * d * 0.12, 0);
      ctx.lineTo(len * d * 0.78, 0);
      ctx.stroke();
      ctx.restore();
    }
  },

  // A wide brim, a round crown and a band.
  sunhat(ctx, d, c) {
    const straw = mix(c.warn, "white", 32);
    ctx.beginPath();
    ctx.ellipse(0, -0.37 * d, 0.62 * d, 0.1 * d, 0, 0, TAU);
    solid(ctx, d, -0.47, -0.27, straw);
    ctx.beginPath();
    ctx.moveTo(-0.3 * d, -0.385 * d);
    ctx.bezierCurveTo(-0.3 * d, -0.74 * d, 0.3 * d, -0.74 * d, 0.3 * d, -0.385 * d);
    ctx.closePath();
    solid(ctx, d, -0.66, -0.385, straw);
    ctx.beginPath();
    ctx.roundRect(-0.3 * d, -0.48 * d, 0.6 * d, 0.085 * d, 0.03 * d);
    solid(ctx, d, -0.48, -0.395, c.accent);
  },

  // A knitted cap: ribbed cuff, two seams, a pompom.
  woolly(ctx, d, c) {
    const a0 = Math.atan2(-0.31, -0.392);
    const a1 = Math.atan2(-0.31, 0.392);
    ctx.beginPath();
    ctx.arc(0, 0, 0.5 * d, a0, a1);
    ctx.closePath();
    solid(ctx, d, -0.5, -0.31, c.accent);
    ctx.strokeStyle = mix(c.accent, c.bg, 30);
    ctx.lineWidth = Math.max(1, d * 0.016);
    for (const x of [-0.15, 0.15]) {
      ctx.beginPath();
      ctx.moveTo(x * d, -0.34 * d);
      ctx.quadraticCurveTo(x * 0.95 * d, -0.43 * d, x * 0.45 * d, -0.495 * d);
      ctx.stroke();
    }
    ctx.beginPath();
    ctx.roundRect(-0.435 * d, -0.37 * d, 0.87 * d, 0.115 * d, 0.05 * d);
    solid(ctx, d, -0.37, -0.255, mix(c.accent, c.bg, 22));
    ctx.beginPath();
    ctx.arc(0, -0.55 * d, 0.088 * d, 0, TAU);
    solid(ctx, d, -0.64, -0.46, c.ink);
  },

  // A maple leaf pinned at the top right.
  leaf(ctx, d, c) {
    const half: [number, number][] = [
      [0, -1], [0.17, -0.55], [0.46, -0.64], [0.38, -0.22], [0.9, -0.16], [0.55, 0.1], [0.64, 0.34], [0.2, 0.24], [0.05, 0.5],
    ];
    const pts = [...half, ...half.slice(1, -1).reverse().map(([x, y]) => [-x, y] as [number, number])];
    const base = mix(c.warn, c.bad, 48);
    ctx.save();
    ctx.translate(0.2 * d, -0.5 * d);
    ctx.rotate(0.5);
    ctx.scale(d * 0.2, d * 0.2);
    ctx.lineJoin = "round";
    ctx.strokeStyle = mix(base, c.bg, 20);
    ctx.lineWidth = 0.16;
    ctx.beginPath();
    ctx.moveTo(0, 0.4);
    ctx.lineTo(0, 1.05);
    ctx.stroke();
    ctx.beginPath();
    pts.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
    ctx.closePath();
    ctx.fillStyle = base;
    ctx.fill();
    ctx.strokeStyle = base;
    ctx.stroke();
    ctx.strokeStyle = mix(base, c.bg, 45);
    ctx.lineWidth = 0.07;
    ctx.beginPath();
    for (const [x, y] of [[0, -0.9], [0.8, -0.14], [-0.8, -0.14]]) {
      ctx.moveTo(0, 0.5);
      ctx.lineTo(x, y);
    }
    ctx.stroke();
    ctx.restore();
  },
};

/** Draws `id` on a body of diameter `d` centred on (x, y), after `engine.draw` put the body there. */
export function drawOutfit(ctx: Ctx, id: OutfitId, engine: BotEngine, x: number, y: number, d: number) {
  const p = poseOf(engine, d);
  ctx.save();
  ctx.translate(x + p.dx, y + p.dy);
  ctx.rotate(p.rotate);
  ctx.lineCap = ctx.lineJoin = "round";
  DRAW[id](ctx, d, inkTokens());
  ctx.restore();
}

/** Where an outfit reaches above the body's centre, in body diameters (for the hit region and the clipping). */
export const OUTFIT_REACH = 0.85;

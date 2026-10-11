// Reactions: short moments the companion plays when something happens. Each one drives the
// engine (an emote or a held face, the hatch) and draws a few shapes of its own around the
// body, in the Visor's language: capsules of light, rings, arcs, in the token colours.
// A reaction runs its frames and stops; nothing here keeps a loop alive.

import type { BotEmoteName } from "../core/layout";
import type { BotEngine, Face } from "./engine";
import { inkTokens, mix } from "./outfits";
import type { SoundName } from "../core/sound";

export type ReactionId = "download" | "agent" | "charger" | "timer" | "screenshot";

type Ctx = CanvasRenderingContext2D;
const TAU = Math.PI * 2;
const unit = (v: number) => Math.min(1, Math.max(0, v));
const out = (p: number) => 1 - Math.pow(1 - unit(p), 3);
const inn = (p: number) => unit(p) * unit(p);
const seg = (p: number, a: number, b: number) => unit((p - a) / (b - a));

interface Reaction {
  /** Seconds. */
  ms: number;
  emote?: BotEmoteName;
  /** An emote that comes in part-way, at `p`. */
  late?: [number, BotEmoteName];
  /** Held on the bar of light for the whole reaction. */
  face?: Face;
  /** Moves the engine along (the hatch), `p` = 0…1. */
  step?(e: BotEngine, p: number): void;
  /** Draws the shapes around the body centred on (x, y), `d` across. */
  draw(ctx: Ctx, x: number, y: number, d: number, p: number): void;
}

/** A capsule of light: the shape of the bar. */
function capsule(ctx: Ctx, x: number, y: number, w: number, h: number) {
  ctx.beginPath();
  ctx.roundRect(x - w / 2, y - h / 2, w, h, Math.min(w, h) / 2);
  ctx.fill();
}

function glow(ctx: Ctx, color: string, d: number) {
  ctx.fillStyle = ctx.strokeStyle = color;
  ctx.shadowColor = mix(color, "transparent", 25);
  ctx.shadowBlur = Math.max(2, d * 0.12);
}

export const REACTIONS: Record<ReactionId, Reaction> = {
  // A file arrives: a capsule falls into the hatch, which opens for it, then the bar smiles.
  download: {
    ms: 1.5,
    late: [0.5, "happy"],
    step(e, p) {
      e.hatch = out(seg(p, 0.12, 0.38)) * (1 - inn(seg(p, 0.5, 0.72)));
    },
    draw(ctx, x, y, d, p) {
      const k = seg(p, 0, 0.42);
      if (k <= 0 || k >= 1) return;
      const th = Math.max(1.5, d * 0.074);
      const top = y - d * 0.95 + inn(k) * d * 0.78;
      ctx.save();
      glow(ctx, inkTokens().info, d);
      capsule(ctx, x, top, th, d * 0.2 * (1 - seg(k, 0.7, 1)));
      ctx.restore();
    },
  },

  // An agent finished: the bar goes green and two rings leave the body.
  agent: {
    ms: 1.5,
    face: "finished",
    draw(ctx, x, y, d, p) {
      ctx.save();
      ctx.strokeStyle = inkTokens().ok;
      ctx.lineWidth = Math.max(1.25, d * 0.036);
      for (let i = 0; i < 2; i++) {
        const k = seg(p, i * 0.22, 0.62 + i * 0.22);
        if (k <= 0 || k >= 1) continue;
        ctx.globalAlpha = 0.7 * (1 - k);
        ctx.beginPath();
        ctx.arc(x, y, d * (0.5 + 0.42 * out(k)), 0, TAU);
        ctx.stroke();
      }
      ctx.restore();
    },
  },

  // The charger is in: a bolt snaps on above the body.
  charger: {
    ms: 1.6,
    emote: "happy",
    draw(ctx, x, y, d, p) {
      const k = out(seg(p, 0, 0.16));
      const fade = 1 - inn(seg(p, 0.78, 1));
      if (k <= 0 || fade <= 0) return;
      const s = d * 0.2 * (0.7 + 0.3 * k);
      ctx.save();
      ctx.translate(x + d * 0.36, y - d * 0.4 + (1 - k) * d * 0.06);
      ctx.globalAlpha = fade * k;
      glow(ctx, inkTokens().ok, d);
      ctx.beginPath();
      ctx.moveTo(s * 0.15, -s);
      ctx.lineTo(-s * 0.55, s * 0.12);
      ctx.lineTo(-s * 0.05, s * 0.12);
      ctx.lineTo(-s * 0.2, s);
      ctx.lineTo(s * 0.55, -s * 0.2);
      ctx.lineTo(s * 0.05, -s * 0.2);
      ctx.closePath();
      ctx.lineJoin = "round";
      ctx.lineWidth = Math.max(1, d * 0.02);
      ctx.fill();
      ctx.stroke();
      ctx.restore();
    },
  },

  // A timer ended: it starts, and two arcs ring out on each side, three times.
  timer: {
    ms: 1.8,
    emote: "surprised",
    draw(ctx, x, y, d, p) {
      const pulse = Math.max(0, Math.sin(seg(p, 0.05, 0.95) * 3 * Math.PI));
      if (pulse <= 0.02) return;
      ctx.save();
      ctx.strokeStyle = inkTokens().warn;
      ctx.lineWidth = Math.max(1.25, d * 0.04);
      for (const s of [-1, 1]) {
        for (const [i, r] of [0.62, 0.78].entries()) {
          ctx.globalAlpha = pulse * (i ? 0.45 : 0.9);
          ctx.beginPath();
          const a = s < 0 ? Math.PI : 0;
          ctx.arc(x, y - d * 0.04, d * r, a - 0.5, a + 0.5);
          ctx.stroke();
        }
      }
      ctx.restore();
    },
  },

  // A screenshot: the band flashes and an iris closes onto it as the bar winks.
  screenshot: {
    ms: 1.2,
    emote: "wink",
    draw(ctx, x, y, d, p) {
      const k = seg(p, 0, 0.28);
      if (k >= 1) return;
      ctx.save();
      ctx.fillStyle = ctx.strokeStyle = inkTokens().ink;
      ctx.globalAlpha = 0.55 * (1 - out(k));
      ctx.beginPath();
      ctx.roundRect(x - d * 0.35, y - d * 0.24, d * 0.7, d * 0.28, d * 0.14);
      ctx.fill();
      ctx.globalAlpha = 0.8 * (1 - k);
      ctx.lineWidth = Math.max(1.25, d * 0.04);
      ctx.beginPath();
      ctx.arc(x, y, d * (0.78 - 0.32 * out(k)), 0, TAU);
      ctx.stroke();
      ctx.restore();
    },
  },
};

/** Starts a reaction on the engine; returns what to call each frame with the seconds gone. */
export function playReaction(e: BotEngine, id: ReactionId) {
  const r = REACTIONS[id];
  if (r.emote) e.triggerEmote(r.emote);
  if (r.face) e.face = r.face;
  let late = r.late;
  return {
    seconds: r.ms,
    step: (t: number) => {
      const p = unit(t / r.ms);
      r.step?.(e, p);
      if (late && p >= late[0]) {
        e.triggerEmote(late[1]);
        late = undefined;
      }
    },
    draw: (ctx: Ctx, x: number, y: number, d: number, t: number) => r.draw(ctx, x, y, d, unit(t / r.ms)),
    end: () => {
      if (r.face) e.face = null;
      e.hatch = 0;
    },
  };
}

/** The sleeper's z's: still, no loop. */
export function drawSleep(ctx: Ctx, x: number, y: number, d: number) {
  ctx.save();
  ctx.strokeStyle = inkTokens().ink2;
  ctx.lineCap = ctx.lineJoin = "round";
  ctx.lineWidth = Math.max(1.25, d * 0.032);
  for (const [dx, dy, s, a] of [[0.4, -0.3, 0.11, 0.85], [0.52, -0.5, 0.075, 0.5]]) {
    ctx.globalAlpha = a;
    ctx.beginPath();
    ctx.moveTo(x + dx * d, y + dy * d);
    ctx.lineTo(x + (dx + s) * d, y + dy * d);
    ctx.lineTo(x + dx * d, y + (dy + s) * d);
    ctx.lineTo(x + (dx + s) * d, y + (dy + s) * d);
    ctx.stroke();
  }
  ctx.restore();
}

// ── The gate ──────────────────────────────────────────────────────────────────

const APART = 2500; // ms between two reactions
const SAME = 30_000; // ms before the same one plays again

/** Lets one reaction through at a time: 2.5 s apart, the same one not within 30 s, a waiting one is played when its turn comes (never more than 2.5 s, well inside the 8 s allowed). */
export function reactionGate(play: (id: ReactionId) => void) {
  let last = -Infinity;
  const lastOf: Partial<Record<ReactionId, number>> = {};
  let timer: ReturnType<typeof setTimeout> | undefined;

  const run = (id: ReactionId) => {
    last = lastOf[id] = performance.now();
    play(id);
  };
  return {
    fire(id: ReactionId) {
      const now = performance.now();
      if (now - (lastOf[id] ?? -Infinity) < SAME) return;
      const wait = APART - (now - last);
      if (wait <= 0) return run(id);
      clearTimeout(timer); // the newest waits; an older one gives way
      timer = setTimeout(() => run(id), wait);
    },
    clear() {
      clearTimeout(timer);
    },
  };
}

// ── Sounds ────────────────────────────────────────────────────────────────────

/** Which of the product's own sounds (src/core/sound.ts) goes with what the companion does. Quiet and short ones only. */
export const COMPANION_SOUND: Record<ReactionId | "land" | "home" | "lift" | "set" | "pat" | "dizzy" | "sleep" | "yawn", SoundName> = {
  download: "attach", agent: "finish", charger: "approve", timer: "approval", screenshot: "wink",
  land: "open", home: "close", lift: "pop", set: "tick", pat: "blip", dizzy: "dizzy", sleep: "sleep", yawn: "yawn",
};
export type CompanionEvent = keyof typeof COMPANION_SOUND;

const GESTURE_GAP = 400; // ms between two sounds that are not reactions (pats, lifts); reactions keep to their gate

/** The sound to play now, or null when it is held back: the companion's own switch or the master switch is off, the window is hidden (full screen, lock), or a gesture came too soon after the last sound. */
export function companionSound(
  ev: CompanionEvent,
  s: { on: boolean; master: boolean; visible: boolean; now: number; last: number },
): SoundName | null {
  if (!s.on || !s.master || !s.visible) return null;
  if (!(ev in REACTIONS) && s.now - s.last < GESTURE_GAP) return null;
  return COMPANION_SOUND[ev];
}

// The mascot: the Visor (docs/ARCHITECTURE.md §9). A round orb in the pill colour with one
// dark band across its upper half; one bar of light inside the band is the whole face.
// Drawing and motion follow candidate A of the owner's design preview (dev/design.js).
//
// Surface: setState, triggerEmote, bodyColor, lookX/lookY (lookY is positive upward),
// setDancing, update(dt), draw(ctx, x, y, d), busy. It draws and nothing else: sounds,
// clicks and timers belong to whoever owns the engine.

import { clamp, lerp } from "../core/anim";
import type { BotEmoteName, BotStateName } from "../core/layout";

export type RGB = readonly [number, number, number]; // components 0…1
/** What the bar of light can show: one face per state and per emote. */
export type Face = BotStateName | BotEmoteName;

export const BOT_STATES: readonly BotStateName[] = [
  "idle", "working", "thinking", "searching", "approval", "question",
  "error", "finished", "ratelimit", "sleeping", "dizzy",
];
/** Every emote and how long it plays, in seconds. */
export const BOT_EMOTES: Record<BotEmoteName, number> = {
  love: 1.8, surprised: 1.2, proud: 1.9, wink: 1.2, happy: 1.4, yawn: 1.8, annoyed: 0.8,
};
const FACES = [...BOT_STATES, ...Object.keys(BOT_EMOTES)] as Face[];

const TAU = Math.PI * 2;
const WHITE: RGB = [1, 1, 1];
const REDUCED = typeof matchMedia === "function" ? matchMedia("(prefers-reduced-motion: reduce)") : null;
const now = () => performance.now() / 1000;
const unit = (v: number) => clamp(v, 0, 1);

export function hexToRGB(hex: string): RGB {
  let h = hex.replace("#", "");
  if (h.length === 3) h = h.replace(/./g, "$&$&");
  const v = parseInt(h, 16);
  return [((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255];
}

const mix = (a: RGB, b: RGB, t: number): RGB => [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)];
const rgba = (c: RGB, a = 1) =>
  `rgba(${Math.round(c[0] * 255)},${Math.round(c[1] * 255)},${Math.round(c[2] * 255)},${a})`;

/** The body when no pill colours it. */
const NEUTRAL = hexToRGB("#2a2d35");

interface Palette { black: RGB; glass: RGB; accent: RGB; tone: Partial<Record<Face, RGB>> }
let pal: Palette | null = null;

/** The design tokens, read once. The fallbacks are the §7 values, for a page that has not loaded them. */
function palette(): Palette {
  if (pal) return pal;
  const css = getComputedStyle(document.documentElement);
  const tok = (name: string, fallback: string) => hexToRGB(css.getPropertyValue(name).trim() || fallback);
  const black = tok("--bg", "#000");
  const bad = tok("--bad", "#f4505e");
  return (pal = {
    black,
    glass: mix(black, tok("--panel", "#0b0c0e"), 0.6),
    accent: tok("--accent", "#7c8cff"),
    // What the bar turns when a state wants the user; it is white otherwise.
    tone: {
      approval: tok("--warn", "#f5a524"), question: tok("--info", "#22d3ee"),
      error: bad, finished: tok("--ok", "#34d399"), love: mix(bad, WHITE, 0.4),
    },
  });
}

/**
 * Clears the whole bitmap, whatever transform the context draws with. A canvas
 * sized in CSS pixels has a bitmap of whole device pixels: on a scaled display
 * it can be a fraction of a pixel larger than the space its drawing code
 * thinks in, and a clearRect in that space leaves the last row half cleared.
 * What is drawn there with some transparency then piles up, frame after frame,
 * into a bright line along the edge. (Ported from upstream
 * `windows/src/core/canvas.ts`, pull request 70.)
 */
export function wipe(ctx: CanvasRenderingContext2D) {
  ctx.save();
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, ctx.canvas.width, ctx.canvas.height);
  ctx.restore();
}

function heart(ctx: CanvasRenderingContext2D, x: number, y: number, s: number) {
  ctx.beginPath();
  ctx.moveTo(x, y + s * 0.9);
  ctx.bezierCurveTo(x - s * 1.5, y - s * 0.1, x - s * 0.75, y - s * 1.2, x, y - s * 0.4);
  ctx.bezierCurveTo(x + s * 0.75, y - s * 1.2, x + s * 1.5, y - s * 0.1, x, y + s * 0.9);
  ctx.fill();
}

/** A four-point sparkle. */
function spark(ctx: CanvasRenderingContext2D, x: number, y: number, s: number) {
  ctx.beginPath();
  ctx.moveTo(x, y - s);
  ctx.quadraticCurveTo(x, y, x + s, y);
  ctx.quadraticCurveTo(x, y, x, y + s);
  ctx.quadraticCurveTo(x, y, x - s, y);
  ctx.quadraticCurveTo(x, y, x, y - s);
  ctx.fill();
}

/**
 * The open part of the hatch, below the bar: where a dropped file goes in. Relative to
 * the centre of a mascot `d` across whose `hatch` is open that far.
 */
export function hatchRect(d: number, hatch: number) {
  return { x: -d * 0.28, y: -d * (0.04 + 0.06 * hatch), w: d * 0.56, h: d * (0.08 + 0.32 * hatch) };
}

// ── Voice faces (docs/VOICE.md §7.6) ──────────────────────────────────────────

/** What the island hands the mascot for a voice turn: the two new faces, or a pose it already has. */
export interface VoiceLook { face: string; level: number }
/** The faces the engine already has, for the voice faces that are not new. */
const VOICE_POSE: Record<string, Face> = { thinking: "thinking", asking: "question", wink: "wink", surprised: "surprised" };
/** Height of the five listening segments, as a share of the loudest (three under d = 30: the middle three). */
const EAR = [0.5, 0.8, 1, 0.8, 0.5];
/** The level a still pose stands for, when motion is reduced. */
const STILL = 0.35;

// ── Dance ─────────────────────────────────────────────────────────────────────

/** The dance fades in over 0.3 s and out over 0.5 s. */
export function stepDanceLevel(level: number, dancing: boolean, dt: number): number {
  const target = dancing ? 1 : 0;
  if (level < target) return Math.min(target, level + dt / 0.3);
  if (level > target) return Math.max(target, level - dt / 0.5);
  return level;
}

/** The tempo of the song that plays, when a feature measured it (audio-extras); 112 BPM otherwise. */
let songBeat: { bpm: number; at: number } | null = null;
export function setDanceBeat(bpm: number, at: number) {
  songBeat = bpm > 0 ? { bpm, at } : null;
}
const danceBeats = (seconds: number) => (songBeat ? ((Date.now() - songBeat.at) * songBeat.bpm) / 60000 : (seconds * 112) / 60);

/**
 * The 112-BPM bounce at `seconds`, for a body of radius R and a dance `level`
 * (0…1): a sideways sway, a hop, a tilt, and a squash on landing.
 */
export function danceTransform(seconds: number, level: number, R: number) {
  const beat = danceBeats(seconds);
  const hop = Math.abs(Math.sin(Math.PI * beat));
  const land = Math.pow(1 - hop, 6);
  return {
    dx: 0.08 * R * Math.sin(Math.PI * beat) * level,
    dy: -0.2 * R * hop * level,
    rotate: 0.1 * Math.sin(Math.PI * beat) * level,
    sx: 1 + 0.045 * land * level,
    sy: 1 - 0.06 * land * level,
  };
}

// ── Engine ────────────────────────────────────────────────────────────────────

export class BotEngine {
  /** The pill colour; null is the neutral dark grey. */
  bodyColor: RGB | null = null;
  /** Where the bar looks, −1…1. lookY is positive upward. */
  lookX = 0;
  lookY = 0;
  state: BotStateName;
  /** A face held until cleared, over the state and any emote (the drop sequence, a pill with a fixed mood). */
  face: Face | null = null;
  /** Mini pills are the body and the bar only: no hearts, no sparkle. */
  isMini = false;
  /** The hatch: the band opened into a mouth for a dropped file, 0 shut … 1 wide. */
  hatch = 0;
  /** Listening or talking, with the level 0…1 of the microphone or of what is played; null = none (costs nothing). */
  voice: VoiceLook | null = null;

  isDancing = false;
  dancingLevel = 0;
  /** Sideways shake, in body radii. */
  ox = 0;
  /** The animation clock, in seconds. */
  t = Math.random() * 20;
  /** Seconds until it next blinks on its own. */
  blinkIn = 1 + Math.random() * 4;

  private w = {} as Record<Face, number>;
  private emote: BotEmoteName | null = null;
  private emoteT = 9;
  private emoteFor = 0;
  private lx = 0;
  private ly = 0;
  private open = 1;
  private blinkT = 0;
  private shakeT = 9;
  private fading = false;
  /** Weights of the two voice faces, and the level they follow (attack 60 ms, release 180 ms). */
  private wl = 0;
  private wt = 0;
  private vl = 0;

  constructor(state: BotStateName = "idle") {
    this.state = state;
    for (const n of FACES) this.w[n] = n === state ? 1 : 0;
  }

  // ── Surface ─────────────────────────────────────────────────────────────────

  setState(next: BotStateName) {
    if (next === this.state) return;
    this.state = next;
    if (next === "error") this.shakeT = 0;
  }

  triggerEmote(emote: BotEmoteName, seconds = BOT_EMOTES[emote]) {
    this.emote = emote;
    this.emoteT = 0;
    this.emoteFor = seconds;
  }

  setDancing(dancing: boolean) {
    this.isDancing = dancing;
  }

  /** True while the next frame would differ: always, unless motion is reduced and nothing is changing. */
  get busy(): boolean {
    return !REDUCED?.matches || this.fading || this.emote !== null || this.dancingLevel > 0.001;
  }

  blink() {
    this.blinkT = 0.16;
  }

  // ── Update ──────────────────────────────────────────────────────────────────

  update(dt: number) {
    const still = REDUCED?.matches ?? false;
    const k = 1 - Math.exp(-dt * 11);
    const w = this.w;
    if (!still) this.t += dt;
    if (this.emote && (this.emoteT += dt) > this.emoteFor) this.emote = null;

    // Faces cross-fade.
    const face = this.face ?? this.emote ?? (this.voice && VOICE_POSE[this.voice.face]) ?? this.state;
    this.fading = false;
    for (const n of FACES) {
      const to = n === face ? 1 : 0;
      if (Math.abs(to - w[n]) > 0.004) this.fading = true;
      w[n] += (to - w[n]) * k;
    }

    if (this.voice || this.wl || this.wt) this.stepVoice(dt, k);

    this.lx += (this.lookX - this.lx) * k;
    this.ly += (this.lookY - this.ly) * k;

    if (!still && (this.blinkIn -= dt) < 0) {
      this.blinkIn = 3 + Math.random() * 3;
      this.blink();
    }
    this.blinkT -= dt;
    this.open += ((this.blinkT > 0 ? 0.1 : 1) - this.open) * (1 - Math.exp(-dt * 50));

    this.shakeT += dt;
    // An error shakes once on arrival; annoyed shakes three times. Both by 2 px at Ø 58.
    const s = this.shakeT;
    const e = this.emoteT;
    const error = s < 0.4 ? Math.sin(s * TAU * 7.5) * (1 - s / 0.4) : 0;
    const annoyed = this.emote === "annoyed" && e < 0.45 ? Math.sin((e / 0.45) * 3 * TAU) : 0;
    this.ox = still ? 0 : (4 / 58) * (error + annoyed);

    this.dancingLevel = stepDanceLevel(this.dancingLevel, this.isDancing, dt);
  }

  private stepVoice(dt: number, k: number) {
    const v = this.voice;
    const to = (on: boolean, was: number) => {
      const next = was + ((on ? 1 : 0) - was) * k;
      if (!on && next < 0.004) return 0;
      if (on ? next < 0.996 : true) this.fading = true;
      return next;
    };
    this.wl = to(v?.face === "listening", this.wl);
    this.wt = to(v?.face === "talking", this.wt);
    const level = v && Number.isFinite(v.level) ? unit(v.level) : 0;
    this.vl += (level - this.vl) * (1 - Math.exp(-dt / (level > this.vl ? 0.06 : 0.18)));
  }

  // ── Draw ────────────────────────────────────────────────────────────────────

  /** The dance, around the foot of the body. */
  private sway(ctx: CanvasRenderingContext2D, px: number, py: number, R: number) {
    if (this.dancingLevel <= 0.001) return;
    const k = danceTransform(now(), this.dancingLevel, R);
    ctx.translate(px + k.dx, py + k.dy);
    ctx.rotate(k.rotate);
    ctx.scale(k.sx, k.sy);
    ctx.translate(-px, -py);
  }

  /** Draws the mascot centred on (x, y), `d` CSS pixels across. It needs about d/2 of room around it. */
  draw(ctx: CanvasRenderingContext2D, x: number, y: number, d: number) {
    const { w, t } = this;
    const e = this.emoteT;
    const u = d / 58;
    const M = REDUCED?.matches ? 0 : 1;
    const A = ctx.globalAlpha;
    const tone = palette().tone;

    let tint = WHITE;
    for (const n of Object.keys(tone) as Face[]) tint = mix(tint, tone[n]!, w[n]);

    const yawn = w.yawn * Math.sin(unit(e / BOT_EMOTES.yawn) * Math.PI);
    const hop = M * u * (
      w.surprised * 6 * Math.sin(unit(e / 0.42) * Math.PI) +
      w.happy * 4 * Math.abs(Math.sin(unit(e / 0.84) * TAU))
    );
    let bob = M * Math.sin((t * TAU) / 2.8) * 1.5 * u * (1 - w.sleeping) * (1 - 0.6 * w.ratelimit);
    if (this.wl || this.wt) {
      bob *= 1 - 0.5 * this.wl; // listening: the bob halved
      if (d < 30) bob = Math.round(bob); // whole pixels at 22
    }
    const breathe = 1 + 0.03 * w.sleeping * (0.5 - 0.5 * Math.cos((t * TAU) / 3));
    // A yawn opens the hatch a little.
    const hatch = unit(Math.max(this.hatch, 0.3 * yawn));
    const lean = this.lx * 8 * (1 - w.sleeping) - w.proud * 6 - w.question * 6 + M * w.dizzy * 7 * Math.sin((t * TAU) / 1.1);

    ctx.save();
    this.sway(ctx, x, y + d * 0.46, d / 2);
    ctx.translate(x + (this.ox * d) / 2, y + bob - hop + w.ratelimit * u);
    ctx.rotate((lean * Math.PI) / 180);
    ctx.scale(breathe * (1 - 0.04 * yawn), breathe * (1 + 0.08 * yawn));
    ctx.lineCap = ctx.lineJoin = "round";
    if (this.wl) tint = mix(tint, palette().accent, this.wl);
    this.paint(ctx, d, tint, hatch, yawn, A);
    ctx.restore();

    if (!this.isMini) {
      if (w.love > 0.02) {
        // Two hearts rising 24 px.
        ctx.fillStyle = rgba(tone.love!);
        for (let i = 0; i < 2; i++) {
          const p = unit((e - i * 0.3) / 1.3);
          ctx.globalAlpha = A * w.love * Math.sin(p * Math.PI);
          heart(ctx, x + (i ? 0.26 : -0.22) * d, y - 0.36 * d - p * 24 * u, Math.max(2, 3.5 * u));
        }
      }
      if (w.proud > 0.02) {
        ctx.globalAlpha = A * w.proud;
        ctx.fillStyle = rgba(WHITE);
        spark(ctx, x + 0.42 * d, y - 0.4 * d, Math.max(2.5, 7 * u) * (0.75 + 0.25 * Math.sin(t * 6)));
      }
    }
    ctx.globalAlpha = A;
  }

  /** The orb, the band and the face, around the orb's centre. */
  private paint(ctx: CanvasRenderingContext2D, d: number, tint: RGB, hatch: number, yawn: number, A: number) {
    const { w, t } = this;
    const e = this.emoteT;
    const p = palette();
    const body = this.bodyColor ?? NEUTRAL;
    const r = d * 0.46;
    const th = Math.max(1.5, d * 0.074);
    const wl = this.wl;
    const wt = this.wt;
    // The faces of the states give way to a voice face.
    const give = wl || wt ? 1 - unit(wl + wt) : 1;
    const alpha = (v: number) => { ctx.globalAlpha = A * v * give; };

    const shade = ctx.createLinearGradient(0, -r, 0, r);
    shade.addColorStop(0, rgba(mix(body, WHITE, 0.3)));
    shade.addColorStop(0.55, rgba(body));
    shade.addColorStop(1, rgba(mix(body, p.black, 0.32)));
    ctx.fillStyle = shade;
    ctx.beginPath();
    ctx.arc(0, 0, r, 0, TAU);
    ctx.fill();
    if (d > 24) {
      // A thin rim of light, top left.
      ctx.strokeStyle = rgba(WHITE, 0.28);
      ctx.lineWidth = Math.max(1, d * 0.022);
      ctx.beginPath();
      ctx.arc(0, 0, r - d * 0.03, -2.75, -1.2);
      ctx.stroke();
    }

    // The band. Opened, it is the hatch: it grows downward and its lower lip catches the light.
    const vw = d * 0.7;
    const vh = d * (0.28 + 0.28 * hatch);
    const top = -d * (0.24 + 0.02 * hatch);
    ctx.fillStyle = rgba(p.glass, 0.94);
    ctx.beginPath();
    ctx.roundRect(-vw / 2, top, vw, vh, d * 0.14);
    ctx.fill();
    if (hatch > 0.02 && d > 24) {
      const lip = ctx.createLinearGradient(0, top + vh * 0.6, 0, top + vh);
      lip.addColorStop(0, rgba(WHITE, 0));
      lip.addColorStop(1, rgba(WHITE, 0.3 * hatch));
      ctx.strokeStyle = lip;
      ctx.lineWidth = Math.max(1, d * 0.02);
      ctx.stroke();
    }

    // The bar of light.
    const bx = this.lx * d * 0.07 * (1 - w.sleeping);
    const by = -d * (0.1 + 0.06 * hatch) - this.ly * d * 0.025;
    if ((wl || wt) && d < 30) ctx.translate(Math.round(bx), Math.round(by));
    else ctx.translate(bx, by);
    ctx.fillStyle = ctx.strokeStyle = rgba(tint);
    ctx.shadowColor = rgba(tint, 0.75);
    ctx.shadowBlur = Math.max(2, d * 0.12);
    ctx.lineWidth = th;
    const bar = (x: number, y: number, wd: number, ht: number) => {
      ht = Math.max(ht, 0.7);
      ctx.beginPath();
      ctx.roundRect(x - wd / 2, y - ht / 2, wd, ht, ht / 2);
      ctx.fill();
    };
    const dot = (x: number, y: number, rad = th * 0.62) => {
      ctx.beginPath();
      ctx.arc(x, y, rad, 0, TAU);
      ctx.fill();
    };
    const pulse = 0.5 + 0.5 * Math.sin((t * TAU) / 1.2);

    let a = w.idle + w.sleeping + w.surprised + w.wink + w.yawn;
    if (a > 0.02) {
      // The steady bar: blinks, widens when surprised, thins asleep and in a yawn, half-closes
      // to wink, stretches along the open hatch and pulses on the beat.
      const beat = this.dancingLevel * Math.abs(Math.sin(Math.PI * danceBeats(now())));
      const wd = ((d * (0.3 * (a - w.surprised) + 0.5 * w.surprised)) / a) * (1 + 0.4 * hatch + 0.25 * beat) * (1 - 0.25 * yawn);
      const wk = w.wink * Math.sin(unit(e / 0.8) * Math.PI);
      alpha(a * (1 - 0.5 * w.sleeping));
      bar(-wd * 0.28 * wk, 0, wd * (1 - 0.56 * wk),
        th * lerp(this.open, 0.32, w.sleeping) * (1 + 0.3 * w.surprised) * (1 - 0.65 * yawn) * (1 - 0.3 * hatch));
    }
    if (w.working > 0.02) {
      // Scanning, with a trail.
      for (let i = 3; i >= 0; i--) {
        alpha(w.working * [1, 0.4, 0.18, 0.08][i]);
        bar(Math.sin(((t - i * 0.05) * TAU) / 1.1) * d * 0.2, 0, d * 0.14, th);
      }
    }
    if (w.thinking > 0.02) {
      // Three dots in turn.
      for (let i = 0; i < 3; i++) {
        const q = 0.5 + 0.5 * Math.sin((t * TAU) / 1.2 - i * 1.1);
        alpha(w.thinking * (0.3 + 0.7 * q));
        dot((i - 1) * d * 0.14, -q * d * 0.015);
      }
    }
    if (w.searching > 0.02) {
      // The bar split in two: a pair of small dots that drifts across, each bobbing on its own.
      const cx = Math.sin((t * TAU) / 2.6) * d * 0.1;
      const gap = d * (0.085 + 0.02 * Math.sin((t * TAU) / 1.3)) * w.searching;
      alpha(w.searching);
      for (const sd of [-1, 1]) dot(cx + sd * gap, Math.sin((t * TAU) / 1.9 + sd) * d * 0.018, th * 0.7);
    }
    if (w.approval > 0.02) {
      alpha(w.approval * (0.5 + 0.5 * pulse));
      bar(0, 0, d * 0.46, th * 1.3);
    }
    if (w.question > 0.02) {
      // A short bar and a raised dot.
      alpha(w.question);
      bar(-d * 0.07, d * 0.012, d * 0.2, th);
      dot(d * 0.13, -d * 0.03 + Math.sin(t * 3) * d * 0.012);
    }
    if (w.error > 0.02) {
      // The bar snapped and crossed, flickering.
      alpha(w.error * (Math.sin(t * 31) > 0.8 ? 0.35 : 1));
      ctx.beginPath();
      ctx.moveTo(-d * 0.1, -d * 0.065);
      ctx.lineTo(d * 0.1, d * 0.065);
      ctx.moveTo(d * 0.1, -d * 0.065);
      ctx.lineTo(-d * 0.1, d * 0.065);
      ctx.stroke();
    }
    a = w.finished + w.happy + w.proud;
    if (a > 0.02) {
      // An arc.
      alpha(a);
      ctx.beginPath();
      ctx.moveTo(-d * 0.13, d * 0.04);
      ctx.quadraticCurveTo(0, -d * 0.12, d * 0.13, d * 0.04);
      ctx.stroke();
    }
    if (w.ratelimit > 0.02) {
      // Dimmed to half and drooping: the middle of the bar sags, slowly heavier and lighter.
      const sag = d * (0.11 + 0.03 * Math.sin((t * TAU) / 3.2));
      alpha(w.ratelimit * 0.5);
      ctx.beginPath();
      ctx.moveTo(-d * 0.14, -d * 0.035);
      ctx.quadraticCurveTo(0, sag, d * 0.14, -d * 0.035);
      ctx.stroke();
    }
    if (w.dizzy > 0.02) {
      // The bar spins a full turn every 0.8 s, with a trail.
      for (let i = 2; i >= 0; i--) {
        alpha(w.dizzy * [1, 0.35, 0.12][i]);
        ctx.save();
        ctx.rotate(((t - i * 0.035) * TAU) / 0.8);
        bar(0, 0, d * 0.24, th);
        ctx.restore();
      }
    }
    if (w.annoyed > 0.02) {
      // The bar tilted.
      alpha(w.annoyed);
      ctx.save();
      ctx.rotate(0.26);
      bar(0, 0, d * 0.3, th);
      ctx.restore();
    }
    if (w.love > 0.02) {
      alpha(w.love);
      heart(ctx, 0, 0, d * 0.085 * (1 + 0.12 * Math.sin((t * TAU) / 0.6)));
    }
    if (wl > 0.02) this.listening(ctx, d, th, A * wl);
    if (wt > 0.02) this.talking(ctx, d, th, A * wt);
  }

  /**
   * Listening: five segments (three under d = 30) that rise with the microphone, each shimmering;
   * silent, a flat dashed bar breathing. Motion reduced: one still pose.
   */
  private listening(ctx: CanvasRenderingContext2D, d: number, th: number, a: number) {
    const still = REDUCED?.matches ?? false;
    const small = d < 30;
    const n = small ? 3 : 5;
    const lv = still ? STILL : this.vl;
    const snap = small ? Math.round : (v: number) => v;
    const sw = snap(d * (small ? 0.1 : 0.06));
    const gap = snap(d * 0.03);
    const first = small ? 1 : 0;
    const x0 = snap(-(n * sw + (n - 1) * gap) / 2);
    const breath = still ? 1 : 1 - 0.15 * (0.5 + 0.5 * Math.sin((this.t * TAU) / 1.2)) * unit(1 - lv * 4);
    ctx.globalAlpha = a;
    for (let i = 0; i < n; i++) {
      const shimmer = still ? 1 : 1 + 0.15 * Math.sin(this.t * TAU * 9 + i * 2.1);
      const ht = snap(Math.max(1, th * (0.6 + 2.2 * lv * EAR[first + i] * shimmer) * breath));
      const x = x0 + i * (sw + gap);
      ctx.beginPath();
      ctx.roundRect(x, snap(-ht / 2), sw, ht, sw / 2);
      ctx.fill();
    }
  }

  /** Talking: one white bar, narrower and taller as the output grows, bouncing while it plays. */
  private talking(ctx: CanvasRenderingContext2D, d: number, th: number, a: number) {
    const still = REDUCED?.matches ?? false;
    const small = d < 30;
    const snap = small ? Math.round : (v: number) => v;
    const out = still ? STILL : this.vl;
    const wd = snap(d * 0.34 * (1 - 0.12 * out));
    const ht = snap(Math.max(1, th * (0.7 + 1.6 * out)));
    const bounce = still ? 0 : -d * 0.018 * Math.abs(Math.sin(this.t * Math.PI * 3.5)) * unit((out - 0.05) / 0.05);
    ctx.globalAlpha = a;
    ctx.beginPath();
    ctx.roundRect(snap(-wd / 2), snap(-ht / 2 + bounce), wd, ht, ht / 2);
    ctx.fill();
  }
}

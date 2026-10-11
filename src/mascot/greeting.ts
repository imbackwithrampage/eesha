// The launch greeting (docs/ARCHITECTURE.md §9), 1.6 s: once the island's strip has slid
// down, the mascot rises from below on a 0.5 s spring and blinks twice. The island moves
// the strip; this draws the mascot, through the engine.

import { Spring } from "../core/anim";
import { Sound } from "../core/sound";
import { BotEngine, wipe } from "./engine";

export const GREETING_END = 1.6;

/** The strip is down by now: the body starts to rise and the motif plays. */
const RISE_AT = 0.25;
const BLINK_AT = [0.95, 1.2];
const SOUND = "greeting";

export class Greeting {
  onComplete: (() => void) | null = null;

  private engine = new BotEngine();
  /** How far below its place the body is, in diameters. */
  private drop = new Spring(1.8);
  private startMs = 0;
  private lastMs = 0;
  private blinks = 0;
  private timers: number[] = [];

  start() {
    this.startMs = this.lastMs = performance.now();
    this.drop.set(1.8);
    this.engine.blinkIn = 3; // only the two blinks below
    this.blinks = 0;
    this.timers.forEach((id) => window.clearTimeout(id));
    this.timers = [
      window.setTimeout(() => Sound.play(SOUND), RISE_AT * 1000),
      window.setTimeout(() => this.onComplete?.(), GREETING_END * 1000),
    ];
  }

  /** The island left the greeting early: nothing more fires, and the motif fades. */
  interrupt() {
    this.timers.forEach((id) => window.clearTimeout(id));
    Sound.fadeOut(SOUND, 0.25);
  }

  /**
   * Draws one frame: the mascot `d` across, coming to rest centred on (cx, cy).
   * The canvas (or the island's clip) must end less than 1.3 d below cy, so the
   * mascot starts out of sight.
   */
  draw(x: CanvasRenderingContext2D, cx: number, cy: number, d: number) {
    const nowMs = performance.now();
    const dt = Math.min(0.05, (nowMs - this.lastMs) / 1000);
    this.lastMs = nowMs;
    const t = (nowMs - this.startMs) / 1000;

    if (t >= RISE_AT) this.drop.target = 0;
    this.drop.step(dt);
    if (this.blinks < BLINK_AT.length && t >= BLINK_AT[this.blinks]) {
      this.blinks += 1;
      this.engine.blink();
    }
    this.engine.update(dt);

    wipe(x);
    this.engine.draw(x, cx, cy + this.drop.value * d, d);
  }
}

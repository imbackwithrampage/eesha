// Mini mascots: the pills and the compact grid (docs/ARCHITECTURE.md §9: body and bar only).
// Each canvas owns an engine; the island's frame loop ticks every live one.

import { BotEngine, hexToRGB, wipe } from "./engine";
import type { AgentTask } from "../core/state";

interface MiniBot {
  canvas: HTMLCanvasElement;
  engine: BotEngine;
  /** Diameter of the mascot, in CSS pixels. */
  d: number;
  taskId: string;
  /** Asked every frame: whether this one dances (the music pill's). */
  dancing?: () => boolean;
}

const live = new Map<HTMLCanvasElement, MiniBot>();

/**
 * Creates a mini mascot `d` CSS pixels across. The slot is that size; the canvas
 * is twice it and centred on the slot (`.mini canvas`), so a hop, a shake or the
 * glow of the bar is not cut off.
 */
export function createMiniBot(task: AgentTask, d: number, dancing?: () => boolean): HTMLElement {
  const slot = document.createElement("span");
  slot.className = "mini";
  slot.style.width = slot.style.height = `${d}px`;

  const canvas = document.createElement("canvas");
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = canvas.height = Math.round(d * 2 * dpr);
  canvas.style.width = canvas.style.height = `${d * 2}px`;
  slot.append(canvas);

  const engine = new BotEngine(task.state);
  engine.isMini = true;
  engine.bodyColor = hexToRGB(task.color);
  engine.face = task.emote ?? task.miniEye ?? null;

  live.set(canvas, { canvas, engine, d, taskId: task.id, dancing });
  return slot;
}

export function releaseMiniBot(canvas: HTMLCanvasElement) {
  live.delete(canvas);
}

/** Drops every canvas no longer in the document (views are rebuilt wholesale). */
export function pruneMiniBots() {
  for (const [canvas] of live) {
    if (!canvas.isConnected) live.delete(canvas);
  }
}

export function syncMiniBotStates(tasks: AgentTask[]) {
  for (const mb of live.values()) {
    const task = tasks.find((t) => t.id === mb.taskId);
    if (!task) continue;
    mb.engine.setState(task.state);
    mb.engine.bodyColor = hexToRGB(task.color);
  }
}

export function tickMiniBots(dt: number) {
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  for (const mb of live.values()) {
    const ctx = mb.canvas.getContext("2d");
    if (!ctx) continue;
    mb.engine.setDancing(mb.dancing?.() ?? false);
    mb.engine.update(dt);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    wipe(ctx);
    mb.engine.draw(ctx, mb.d, mb.d, mb.d);
  }
}

export const miniBotCount = () => live.size;

// The selector: the page of one overlay window, one per display. It shows the
// frozen picture of its display pixel for pixel and lets the user point at what
// to take: a region, a window or the whole display (screenshot), one pixel
// (colour), a region to read (text and QR codes) or a region to measure (ruler).
// While the recorder feature is on there is a fifth tool, Record: the region or
// the window pointed at is handed to the recorder, which films it.
//
// Everything is counted in pixels of the picture, which are the physical pixels
// of the display: the page only turns pointer positions into them, and Rust
// cuts exactly the rectangle it is given. The colours are read from the bytes
// Rust sent, never from what the canvas shows.
//
// The loupe's geometry and the click and drag rules follow
// reference/vorssaint-utils/Sources/Vorssaint/Services/QuickTools/ScreenshotSupport.swift
// (captureLoupeSampleSide, captureLoupeZoom, cropLoupeSampleRect, selectionRect, isClick).

import { Bridge } from "../../core/bridge";
import type { FeatureCtx } from "../../core/feature";
import { iconBtn, tab } from "../../ui/components";
import { clear, h } from "../../views/dom";

export type Tool = "shot" | "color" | "text" | "ruler" | "record";
/** The tools that are always there; "record" joins them while the recorder feature runs. */
export const TOOLS: Tool[] = ["shot", "color", "text", "ruler"];

/** A ruler: the one icon the shared set does not have. */
export const RULER = "M3.5 8h17a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1h-17a1 1 0 0 1-1-1V9a1 1 0 0 1 1-1zM6.5 8v3M10.5 8v4.5M14.5 8v3M18 8v4.5";
export const TOOL_ICON: Record<Tool, string> = { shot: "shot", color: "drop", text: "text", ruler: RULER, record: "record" };

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
  /** A window's handle. */
  id?: number;
}

interface Frame {
  tool: Tool;
  w: number;
  h: number;
  /** The picture as Rust holds it: RGBA, top row first, base64. */
  rgba: string;
  /** Front to back, in pixels of this display. */
  windows: Box[];
  ocr: boolean;
  settings: Record<string, unknown>;
}

/** The label of the window this page runs in. */
export function windowLabel(): string {
  const tauri = (window as unknown as { __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } } }).__TAURI_INTERNALS__;
  return tauri?.metadata?.currentWindow?.label ?? "";
}

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));

export const hexOf = (r: number, g: number, b: number) =>
  `#${[r, g, b].map((v) => v.toString(16).padStart(2, "0")).join("")}`.toUpperCase();

/** A colour as the user wants it copied: "hex", "rgb" or "hsl". */
export function colourText(r: number, g: number, b: number, format: string): string {
  if (format === "rgb") return `rgb(${r}, ${g}, ${b})`;
  if (format !== "hsl") return hexOf(r, g, b);
  const [rf, gf, bf] = [r / 255, g / 255, b / 255];
  const max = Math.max(rf, gf, bf);
  const min = Math.min(rf, gf, bf);
  const l = (max + min) / 2;
  const d = max - min;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  const hue = d === 0 ? 0 : max === rf ? ((gf - bf) / d) % 6 : max === gf ? (bf - rf) / d + 2 : (rf - gf) / d + 4;
  return `hsl(${Math.round((hue * 60 + 360) % 360)}, ${Math.round(s * 100)}%, ${Math.round(l * 100)}%)`;
}

/** The magnifier square, in CSS pixels, and the block of picture pixels it shows at zoom 1. */
const LOUPE = 148;
const BASE = 13;
const LEAST = 3;
/** An odd number of pixels, so one real pixel is the centre cell. */
const sampleSide = (zoom: number) => Math.max(LEAST, 2 * Math.floor(BASE / clamp(zoom, 0.5, BASE / LEAST) / 2) + 1);

function bytes(base64: string): Uint8ClampedArray<ArrayBuffer> {
  const text = atob(base64);
  const out = new Uint8ClampedArray(text.length);
  for (let i = 0; i < text.length; i++) out[i] = text.charCodeAt(i);
  return out;
}

export function mountSelector(root: HTMLElement, cx: FeatureCtx) {
  const t = cx.t;
  const label = windowLabel();
  root.className = "f-capture-sel";

  const canvas = h("canvas", { class: "f-capture-frame" });
  const dim = h("div", { class: "f-capture-dim" });
  const box = h("div", { class: "f-capture-box", hidden: true });
  const size = h("span", { class: "f-capture-size", hidden: true });
  const glass = h("canvas");
  const swatch = h("i", { class: "f-capture-swatch" });
  const value = h("b");
  const where = h("small");
  const loupe = h("div", { class: "f-capture-loupe", hidden: true }, glass, h("div", { class: "f-capture-read" }, swatch, value, where));
  const bar = h("div", { class: "f-capture-bar fx-in", hidden: true });
  root.append(canvas, dim, box, size, loupe, bar);

  // The loupe is a canvas: its lines take the tokens' colours from the page.
  const tokens = getComputedStyle(document.documentElement);
  const ink = tokens.getPropertyValue("--ink").trim();
  const dark = tokens.getPropertyValue("--bg").trim();
  const hairline = tokens.getPropertyValue("--hairline").trim();

  let frame: Frame | null = null;
  let pixels = new Uint8ClampedArray(0);
  let tool: Tool = "shot";
  /** The tools on the bar: Record is there only while the recorder runs. */
  let tools: Tool[] = TOOLS;
  /** A window can be pointed at: a click takes it. */
  const takesWindow = () => tool === "shot" || tool === "record";
  /** The pixel under the pointer. */
  let at = { x: 0, y: 0 };
  /** The pointer is over this display, and not on the bar. */
  let inside = false;
  let onBar = false;
  /** A press: the pixel it began on (and the pointer in CSS pixels), and whether it has become a drag. */
  let from: { x: number; y: number } | null = null;
  let down = { x: 0, y: 0 };
  let moved = false;
  /** The ruler's last measure stays on screen. */
  let held: Box | null = null;
  let zoom = 1;
  let glassOn = true;
  let format = "hex";
  /** "Copied" in the loupe until then. */
  let saidUntil = 0;
  /** The choice is made: nothing answers the pointer any more. */
  let done = false;
  let raf = 0;

  /** CSS pixels per pixel of the picture: the window covers its display exactly. */
  const k = () => 1 / window.devicePixelRatio;
  /** From one pixel to another, both in it: what the loupe showed at either end is in the capture. */
  const rectOf = (a: { x: number; y: number }, b: { x: number; y: number }): Box => ({
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x) + 1,
    h: Math.abs(a.y - b.y) + 1,
  });
  const colourAt = (x: number, y: number): [number, number, number] => {
    const i = (y * (frame?.w ?? 0) + x) * 4;
    return [pixels[i] ?? 0, pixels[i + 1] ?? 0, pixels[i + 2] ?? 0];
  };
  /** The window a click would take: the first one, front to back, under the pointer. */
  const windowAt = () => frame?.windows.find((w) => at.x >= w.x && at.y >= w.y && at.x < w.x + w.w && at.y < w.y + w.h) ?? null;

  // ── Drawing ─────────────────────────────────────────────────────────────────

  const drawGlass = (f: Frame) => {
    const g = glass.getContext("2d");
    if (!g) return;
    const n = sampleSide(zoom);
    // Near an edge the block slides inward instead of shrinking; the ring still marks the pixel itself.
    const sx = clamp(at.x - (n - 1) / 2, 0, Math.max(0, f.w - n));
    const sy = clamp(at.y - (n - 1) / 2, 0, Math.max(0, f.h - n));
    const side = glass.width;
    const cell = side / n;
    g.imageSmoothingEnabled = false;
    g.clearRect(0, 0, side, side);
    g.drawImage(canvas, sx, sy, n, n, 0, 0, side, side);
    // The grid earns its ink once a cell is big enough to be one.
    if (LOUPE / n >= 6) {
      g.strokeStyle = hairline;
      g.lineWidth = 1;
      g.beginPath();
      for (let i = 1; i < n; i++) {
        const p = Math.round(i * cell) + 0.5;
        g.moveTo(p, 0);
        g.lineTo(p, side);
        g.moveTo(0, p);
        g.lineTo(side, p);
      }
      g.stroke();
    }
    const [tx, ty] = [(at.x - sx) * cell, (at.y - sy) * cell];
    g.lineWidth = 3;
    g.strokeStyle = dark;
    g.strokeRect(tx, ty, cell, cell);
    g.lineWidth = 1.5;
    g.strokeStyle = ink;
    g.strokeRect(tx, ty, cell, cell);

    const [r, gr, b] = colourAt(at.x, at.y);
    swatch.style.background = hexOf(r, gr, b);
    value.textContent = colourText(r, gr, b, format);
    where.textContent = performance.now() < saidUntil ? t("Copied") : `${at.x}, ${at.y}`;
  };

  const draw = () => {
    raf = 0;
    const f = frame;
    if (!f || done) return;
    const s = k();
    const drag = from && moved && tool !== "color" ? rectOf(from, at) : null;
    const pointing = inside && !onBar;
    const hover = takesWindow() && pointing && !from ? windowAt() : null;
    const shown = drag ?? (tool === "ruler" ? held : hover);
    box.hidden = !shown;
    // The box dims what is around it; without one the whole display is dimmed (never for colours).
    dim.hidden = !!shown;
    if (shown) {
      box.classList.toggle("hover", shown === hover);
      box.style.left = `${shown.x * s}px`;
      box.style.top = `${shown.y * s}px`;
      box.style.width = `${shown.w * s}px`;
      box.style.height = `${shown.h * s}px`;
    }
    const sized = drag ?? (tool === "ruler" ? held : null);
    size.hidden = !sized;
    if (sized) {
      const text = `${sized.w} × ${sized.h}`;
      size.textContent = !drag && tool === "ruler" ? t("{size} copied", { size: text }) : text;
      // Above the selection's top left corner, or just inside it at the top of the display.
      const top = sized.y * s - 26;
      size.style.left = `${clamp(sized.x * s, 4, window.innerWidth - size.offsetWidth - 4)}px`;
      size.style.top = `${top < 4 ? sized.y * s + 6 : top}px`;
    }
    bar.classList.toggle("away", !!drag);

    const glassShown = pointing && (tool === "color" || glassOn);
    loupe.hidden = !glassShown;
    if (glassShown) {
      drawGlass(f);
      const [px, py] = [at.x * s, at.y * s];
      const [w, hgt] = [loupe.offsetWidth, loupe.offsetHeight];
      const x = px + 24 + w + 8 > window.innerWidth ? px - 24 - w : px + 24;
      const y = py + 24 + hgt + 8 > window.innerHeight ? py - 24 - hgt : py + 24;
      loupe.style.transform = `translate(${Math.round(x)}px, ${Math.round(y)}px)`;
    }
  };
  const redraw = () => {
    if (!raf) raf = requestAnimationFrame(draw);
  };

  // ── What ends the session ───────────────────────────────────────────────────

  const leave = (method: "finish" | "cancel", args: object = {}) => {
    if (done) return;
    done = true;
    root.classList.add("done");
    void cx.call(method, { label, tool, ...args });
  };
  const cancel = () => leave("cancel");

  /** The colour under the pointer: copied and done, or with `keep` copied while the picture stays. */
  const pick = (keep: boolean) => {
    const [r, g, b] = colourAt(at.x, at.y);
    const text = colourText(r, g, b, format);
    if (!keep) return leave("finish", { text, hex: hexOf(r, g, b) });
    void cx.call("copy_text", { text });
    saidUntil = performance.now() + 900;
    window.setTimeout(redraw, 950);
    redraw();
  };

  // ── The bar: the tools, what the pointer does, cancel ───────────────────────

  const drawBar = () => {
    const f = frame;
    if (!f) return;
    const names: Record<Tool, string> = {
      shot: t("Screenshot"),
      color: t("Colour"),
      text: f.ocr ? t("Text") : t("QR code"),
      ruler: t("Ruler"),
      record: t("Record"),
    };
    const hints: Record<Tool, string> = {
      shot: t("Drag an area or click a window. Enter takes the whole screen."),
      color: t("Click to copy the colour. C copies and keeps going."),
      text: f.ocr ? t("Drag over text or a QR code to read it.") : t("Drag over a QR code to read it."),
      ruler: t("Drag to measure. The size is copied."),
      record: t("Drag an area or click a window to record it. Enter records the whole screen."),
    };
    clear(bar);
    tools.forEach((id, i) => {
      const button = tab(TOOL_ICON[id], names[id], () => setTool(id, true));
      button.title = `${names[id]} (${i + 1})`;
      button.classList.toggle("on", id === tool);
      bar.append(button);
    });
    bar.append(h("i"), h("span", { class: "f-capture-hint", text: hints[tool] }), iconBtn("close", t("Cancel (Esc)"), cancel));
  };

  /** `tell`: the other displays' selectors follow. */
  const setTool = (next: Tool, tell: boolean) => {
    if (!tools.includes(next)) return;
    tool = next;
    root.dataset.tool = next;
    from = null;
    moved = false;
    held = null;
    drawBar();
    redraw();
    if (tell) void cx.call("tool", { tool: next });
  };
  cx.on<Tool>("tool", (next) => next !== tool && setTool(next, false));
  for (const [name, on] of [["pointerenter", true], ["pointerleave", false]] as const) {
    bar.addEventListener(name, () => {
      onBar = on;
      redraw();
    });
  }

  // ── Pointer and keys ────────────────────────────────────────────────────────

  const locate = (e: MouseEvent) => {
    const f = frame;
    if (!f) return;
    // The pointer sits on a whole pixel of the display, which CSS reports as a fraction: back to the pixel.
    const s = k();
    at = { x: clamp(Math.round(e.clientX / s), 0, f.w - 1), y: clamp(Math.round(e.clientY / s), 0, f.h - 1) };
    inside = true;
  };

  root.addEventListener("pointerdown", (e) => {
    if (done || !frame || e.button !== 0 || bar.contains(e.target as Node)) return;
    locate(e);
    from = { ...at };
    down = { x: e.clientX, y: e.clientY };
    moved = false;
    held = null;
    root.setPointerCapture(e.pointerId);
    redraw();
  });
  root.addEventListener("pointermove", (e) => {
    if (done || !frame) return;
    locate(e);
    // A press that never travels 4 px is a click.
    if (from && (Math.abs(e.clientX - down.x) >= 4 || Math.abs(e.clientY - down.y) >= 4)) moved = true;
    redraw();
  });
  root.addEventListener("pointerleave", () => {
    inside = false;
    redraw();
  });
  root.addEventListener("pointerup", (e) => {
    if (done || !frame || !from) return;
    locate(e);
    const drag = moved ? rectOf(from, at) : null;
    from = null;
    moved = false;
    if (tool === "color") return pick(false);
    if (!drag) {
      const target = takesWindow() ? windowAt() : null;
      if (target?.id != null) leave("finish", { window: target.id });
      return redraw();
    }
    // Smaller than 2 × 2 is a slip of the hand.
    if (drag.w < 2 || drag.h < 2) return redraw();
    if (tool === "ruler") {
      held = drag;
      void cx.call("copy_text", { text: `${drag.w} × ${drag.h}` });
      return redraw();
    }
    leave("finish", { rect: drag });
  });
  // The way out that needs no keyboard.
  root.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    cancel();
  });
  root.addEventListener(
    "wheel",
    (e) => {
      if (loupe.hidden) return;
      // Three steps of 15 % for one notch of the wheel.
      zoom = clamp(zoom * (e.deltaY < 0 ? 1.15 ** 3 : 1.15 ** -3), 0.5, BASE / LEAST);
      redraw();
    },
    { passive: true },
  );

  window.addEventListener("keydown", (e) => {
    if (done) return;
    if (e.key === "Escape") return cancel();
    if (!frame || e.ctrlKey || e.altKey || e.metaKey) return;
    if (e.key === "Enter") {
      e.preventDefault();
      // The whole display for a screenshot or a reading; the pixel for a colour; a ruler just closes.
      return tool === "color" ? pick(false) : tool === "ruler" ? cancel() : leave("finish");
    }
    const number = ["1", "2", "3", "4", "5"].indexOf(e.key);
    if (number >= 0) return setTool(tools[number], true);
    const letter = e.key.toLowerCase();
    if (letter === "z") {
      glassOn = !glassOn;
      return redraw();
    }
    if (letter === "c" && tool === "color") return pick(true);
    const step = e.shiftKey ? 10 : 1;
    const by = ({ ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] } as Record<string, number[]>)[e.key];
    if (by) {
      e.preventDefault();
      // Rust moves the real pointer by whole pixels; the move comes back as a pointer event.
      void cx.call("nudge", { dx: by[0], dy: by[1] });
    }
  });
  window.addEventListener("resize", redraw);

  // ── The picture ─────────────────────────────────────────────────────────────

  void cx.call<Frame>("frame", { label }).then((f) => {
    const context = canvas.getContext("2d");
    if (!f || !context) return cancel();
    try {
      pixels = bytes(f.rgba);
      canvas.width = f.w;
      canvas.height = f.h;
      context.putImageData(new ImageData(pixels as Uint8ClampedArray<ArrayBuffer>, f.w, f.h), 0, 0);
    } catch {
      return cancel();
    }
    f.rgba = "";
    frame = f;
    glass.width = glass.height = Math.round(LOUPE * window.devicePixelRatio);
    glassOn = f.settings.loupe !== false;
    format = String(f.settings.colorFormat ?? "hex");
    setTool(f.tool, false);
    bar.hidden = false;
    window.focus();
    // The recorder is another feature: its tool is on the bar only while it runs.
    void Bridge.featuresList().then((list) => {
      if (!list?.some((feature) => feature.id === "recorder" && feature.running)) return;
      tools = [...TOOLS, "record"];
      if (f.tool === "record") setTool("record", false);
      else drawBar();
    });
  });
}

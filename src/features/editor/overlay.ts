// The two pages that sit over a whole display: drawing on the live screen, and
// the selector and bar of the scrolling capture. Both windows are see-through
// and exactly as large as the display, so a pixel of the page is a pixel of
// the screen.

import type { FeatureCtx } from "../../core/feature";
import { iconBtn, kbd } from "../../ui/components";
import { icon } from "../../ui/icons";
import { h } from "../../views/dom";
import { Board, fontPx, ICONS, palette, sep, sizeButtons, swatches, toolRail, typeText, type Tool } from "./draw";

// ── Drawing on the screen ─────────────────────────────────────────────────────

/**
 * The marks of the editor on a see-through canvas over the screen. With a tool
 * in hand the window takes the pointer; with none (the pointer button, or the
 * shortcut again) clicks go through to what is under the drawing and only the
 * tool bar still takes them. Esc leaves.
 */
export function mountDraw(root: HTMLElement, cx: FeatureCtx) {
  const t = cx.t;
  const colors = palette();
  root.className = "f-editor-draw";

  /** Clicks go through to the screen. */
  let through = false;
  /** The tool to come back to. */
  let last: Tool = "pen";
  let typing = false;

  const view = h("canvas");
  const board = new Board(() => {
    undoBtn.disabled = !board.canUndo;
    clearBtn.disabled = board.marks.length === 0;
    paint();
  });
  // The screen's pixels are small: lines are twice as thick as on a picture.
  [board.color, board.size] = [colors[0], 4];

  const rail = toolRail(t, ["pen", "highlight", null, "arrow", "line", "rect", "ellipse", "text", null, "eraser"], setTool);
  const throughBtn = h("button", { class: "tab icon f-editor-tool", title: t("Use the screen under the drawing"), "aria-label": t("Use the screen under the drawing"), onclick: () => setThrough(true) }, icon(ICONS.select, 16));
  const colorRow = swatches(t, (i) => {
    board.color = colors[i];
    colorRow.show(i);
  });
  const sizeRow = sizeButtons(
    t,
    (size) => {
      board.size = size;
      sizeRow.show(size);
    },
    2,
  );
  const undoBtn = iconBtn(ICONS.undo, `${t("Undo")} (Ctrl+Z)`, () => board.undo());
  const clearBtn = iconBtn("trash", t("Clear the drawing"), () => board.marks.length && board.set([]));
  const closeBtn = iconBtn("close", `${t("Stop drawing")} (Esc)`, () => void cx.call("draw_close"));
  const bar = h("div", { class: "f-editor-bar fx-in" }, throughBtn, sep(), ...rail.els, sep(), colorRow.el, sep(), ...sizeRow.els, sep(), undoBtn, clearBtn, closeBtn);
  root.append(view, bar);
  colorRow.show(0);
  sizeRow.show(4);

  function paint() {
    const c = view.getContext("2d")!;
    c.clearRect(0, 0, view.width, view.height);
    board.paint(c);
  }

  /** Pixels of the display per CSS pixel. */
  const scale = () => view.width / view.getBoundingClientRect().width || 1;

  function setThrough(on: boolean) {
    through = on;
    root.classList.toggle("through", on);
    throughBtn.classList.toggle("on", on);
    rail.show(on ? null : board.tool);
    // Where the tool bar is on the display: the one place that still takes clicks.
    const [k, r] = [scale(), bar.getBoundingClientRect()];
    void cx.call("draw_through", { on, bar: { x: r.left * k, y: r.top * k, w: r.width * k, h: r.height * k } });
  }

  function setTool(tool: Tool) {
    [board.tool, last] = [tool, tool];
    view.style.cursor = tool === "text" ? "text" : "crosshair";
    if (through) setThrough(false);
    rail.show(tool);
  }

  new ResizeObserver(([entry]) => {
    const box = entry.devicePixelContentBoxSize?.[0];
    const k = window.devicePixelRatio || 1;
    const [w, h] = box ? [box.inlineSize, box.blockSize] : [Math.round(entry.contentRect.width * k), Math.round(entry.contentRect.height * k)];
    if (view.width === w && view.height === h) return;
    [view.width, view.height] = [w, h];
    paint();
  }).observe(view, { box: "device-pixel-content-box" });

  const at = (e: PointerEvent): [number, number] => {
    const r = view.getBoundingClientRect();
    const k = view.width / r.width;
    return [(e.clientX - r.left) * k, (e.clientY - r.top) * k];
  };
  /** How near a mark the eraser has to come, in pixels of the display. */
  const REACH = 8;
  view.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || typing) return;
    const [x, y] = at(e);
    if (board.tool === "text") {
      e.preventDefault();
      const [k, color, size] = [scale(), board.color, board.size];
      typing = true;
      return typeText(root, { left: x / k, top: y / k, px: fontPx(size) / k, color }, (text) => {
        typing = false;
        if (text) board.set([...board.marks, { tool: "text", color, size, pts: [Math.round(x), Math.round(y)], text }]);
      });
    }
    view.setPointerCapture(e.pointerId);
    board.down(x, y, REACH);
  });
  view.addEventListener("pointermove", (e) => board.move(...at(e), REACH));
  view.addEventListener("pointerup", () => board.up());
  view.addEventListener("pointercancel", () => board.up());

  window.addEventListener("keydown", (e) => {
    const key = e.key.toLowerCase();
    const tool = rail.byKey(key);
    if (key === "escape") void cx.call("draw_close");
    else if ((e.ctrlKey && key === "y") || (e.ctrlKey && e.shiftKey && key === "z")) board.redo();
    else if (e.ctrlKey && key === "z") board.undo();
    else if (tool && !e.ctrlKey && !e.altKey) setTool(tool);
    else return;
    e.preventDefault();
  });

  // The shortcut again: between drawing and using what is under the drawing.
  cx.on("draw-toggle", () => (through ? setTool(last) : setThrough(true)));
  setTool("pen");
  undoBtn.disabled = clearBtn.disabled = true;
  // Rust closes a window over the display whose page never said this.
  void cx.call("ready", { page: "draw" });
}

// ── Scrolling capture ─────────────────────────────────────────────────────────

/**
 * First the selector over the display: the user drags the part of the page
 * that scrolls. Rust then makes this same window the small bar beside that
 * region, and the page shows how tall the picture has grown, with Done.
 */
export function mountScroll(root: HTMLElement, cx: FeatureCtx) {
  const t = cx.t;
  root.className = "f-editor-scroll";
  const box = h("div", { class: "f-editor-pick", hidden: true });
  const hint = h("div", { class: "f-editor-hint" }, icon(ICONS.scroll, 16), h("span", { text: t("Drag over the part of the page that scrolls") }), kbd("Esc"));
  root.append(box, hint);

  const cancel = () => void cx.call("scroll_cancel");
  const done = () => void cx.call("scroll_done");
  let running = false;
  let from: [number, number] | null = null;

  const rect = (e: PointerEvent) => {
    const [x, y] = from ?? [e.clientX, e.clientY];
    return { x: Math.min(x, e.clientX), y: Math.min(y, e.clientY), w: Math.abs(e.clientX - x), h: Math.abs(e.clientY - y) };
  };
  const place = (e: PointerEvent) => {
    const r = rect(e);
    box.hidden = false;
    box.style.cssText = `left:${r.x}px;top:${r.y}px;width:${r.w}px;height:${r.h}px`;
  };

  root.addEventListener("pointerdown", (e) => {
    if (running) return;
    // Any other button backs out.
    if (e.button !== 0) return cancel();
    from = [e.clientX, e.clientY];
    root.setPointerCapture(e.pointerId);
    root.classList.add("picking");
    place(e);
  });
  root.addEventListener("pointermove", (e) => from && place(e));
  root.addEventListener("pointerup", (e) => {
    if (!from) return;
    const r = rect(e);
    from = null;
    // Too small to be a page: the drag starts over.
    if (r.w < 40 || r.h < 40) {
      box.hidden = true;
      return root.classList.remove("picking");
    }
    void start(r);
  });
  root.addEventListener("contextmenu", (e) => e.preventDefault());
  void cx.call("ready", { page: "scroll" });
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") cancel();
    else if (e.key === "Enter" && running) done();
  });

  async function start(r: { x: number; y: number; w: number; h: number }) {
    running = true;
    const k = window.devicePixelRatio || 1;
    const rows = h("span", { class: "f-editor-rows" });
    const doneBtn = h("button", { class: "btn sm primary", onclick: done }, h("span", { text: t("Done") }), h("span", { class: "kbd", text: "Enter" }));
    const hud = h("div", { class: "f-editor-hud" }, icon(ICONS.scroll, 14), h("b", { text: t("Scroll the page") }), rows, h("span", { class: "isl-gap" }), h("button", { class: "btn sm ghost", text: t("Cancel"), onclick: cancel }), doneBtn);
    // Nothing shows while the window is still as large as the display.
    root.className = "f-editor-scroll running";
    root.replaceChildren();
    cx.on<{ rows: number }>("scroll", (grown) => (rows.textContent = t("{n} px", { n: grown.rows.toLocaleString() })));
    // The window becomes the bar; the region is in pixels of the display.
    const began = await cx.call<boolean>("scroll_start", { x: r.x * k, y: r.y * k, w: r.w * k, h: r.h * k });
    if (began) root.append(hud);
    else cancel();
  }
}

// What the strip and the open island both show travels from one to the other
// (docs/ARCHITECTURE.md §4, "Opening and folding"): a title, a cover. The strip
// and the views mark such elements with `data-shared="<name>"` ("title", "mark");
// when the island opens or folds, each one that is in both and says the same
// thing gets a copy of where it is going, placed over where it comes from and
// moved there with the shell, by transform only. The two real elements stay out
// of sight meanwhile, and the copy goes the moment it has arrived on the real one.

import { Tracked, lerp } from "../core/anim";
import { h } from "../views/dom";

interface Box {
  /** Left edge from the island's centre line, and the middle from its top edge. */
  x: number;
  cy: number;
  w: number;
  h: number;
}

interface Item {
  copy: HTMLElement;
  from: Box;
  to: Box;
  /** The size the copy starts at, as a factor of the size it ends at. */
  scale: number;
}

/** A progress that starts slowly; past its end (the spring overshoots) it goes on as it came. */
const late = (p: number) => (p < 1 ? p * p : p);
/** A progress that is nearly there early. */
const early = (p: number) => (p < 1 ? 1 - (1 - p) * (1 - p) : p);

/** More than this inside one shared element and it is not a title or a cover: it just cross-fades. */
const MAX_PARTS = 12;

/** Where `el` is laid out inside `root`: its layout box, which no transform or fade above it changes. */
function box(el: HTMLElement, root: HTMLElement, half: number): Box | null {
  let x = 0;
  let y = 0;
  let node: HTMLElement | null = el;
  while (node && node !== root) {
    x += node.offsetLeft;
    y += node.offsetTop;
    node = node.offsetParent as HTMLElement | null;
  }
  const [w, hh] = [el.offsetWidth, el.offsetHeight];
  if (node !== root || w === 0 || hh === 0) return null;
  return { x: x - half, cy: y + hh / 2, w, h: hh };
}

const isText = (el: HTMLElement) => el.childElementCount === 0 && (el.textContent ?? "").trim() !== "";

/** Both ends show the same thing: the same words, or the same picture. */
function same(a: HTMLElement, b: HTMLElement): boolean {
  if (isText(a) || isText(b)) return (a.textContent ?? "").trim() === (b.textContent ?? "").trim();
  const picture = (el: HTMLElement) => (el instanceof HTMLImageElement ? el.currentSrc : getComputedStyle(el).backgroundImage);
  const [pa, pb] = [picture(a), picture(b)];
  return pa !== "" && pa !== "none" && pa === pb;
}

/** `copy` looks like `real` wherever it is put: everything the page's rules gave it, written on it. */
function dress(real: Element, copy: HTMLElement) {
  const style = getComputedStyle(real);
  for (const name of style) copy.style.setProperty(name, style.getPropertyValue(name));
  copy.removeAttribute("id");
  copy.removeAttribute("data-shared");
}

export class Travel {
  readonly el = h("div", { id: "travel", "aria-hidden": "true" });

  private items: Item[] = [];
  private hidden: HTMLElement[] = [];
  private progress = new Tracked(1);
  private opening = true;
  private until = 0;

  /**
   * Sends what `from` and `to` both show on its way. `root` is the island and
   * `half` half its width right now (both ends are centred in it). Opening, the
   * copies move on the shell's spring; folding, on its curve. They stay until
   * `stay` ms have passed, the time the real content needs to be fully there.
   */
  start(root: HTMLElement, from: ParentNode | null, to: ParentNode | null, half: number, opening: boolean, now: number, stay: number) {
    this.finish();
    if (!from || !to) return;
    for (const dest of to.querySelectorAll<HTMLElement>("[data-shared]")) {
      const name = dest.dataset.shared ?? "";
      const src = [...from.querySelectorAll<HTMLElement>("[data-shared]")].find((el) => el.dataset.shared === name);
      if (!src || !same(src, dest)) continue;
      const parts = dest.querySelectorAll("*");
      const [a, b] = [box(src, root, half), box(dest, root, half)];
      if (!a || !b || parts.length > MAX_PARTS || dest.querySelector("canvas")) continue;

      const copy = dest.cloneNode(true) as HTMLElement;
      dress(dest, copy);
      copy.querySelectorAll<HTMLElement>("*").forEach((part, i) => dress(parts[i], part));
      // Text keeps its shape: it starts at the size of the letters it replaces. A picture starts at the size of the one it replaces.
      const letters = (el: HTMLElement) => parseFloat(getComputedStyle(el).fontSize) || 1;
      const scale = isText(dest) ? letters(src) / letters(dest) : a.w / b.w;
      // Its own size, whatever the place it was copied from allowed it (a share of a column that is not here).
      Object.assign(copy.style, {
        width: `${b.w}px`, height: `${b.h}px`, maxWidth: "none", maxHeight: "none", minWidth: "0", minHeight: "0",
        transition: "none", animation: "none", visibility: "visible", opacity: "1", filter: "none",
        // It starts where the words it replaces start, and grows from there.
        transformOrigin: "0 50%",
      });
      // Words that fit are never cut short by a box measured to the whole pixel.
      if (isText(dest) && dest.scrollWidth <= dest.clientWidth) Object.assign(copy.style, { overflow: "visible", textOverflow: "clip" });
      this.el.append(copy);
      this.items.push({ copy, from: a, to: b, scale });
      for (const end of [src, dest]) {
        end.classList.add("travelling");
        this.hidden.push(end);
      }
    }
    if (this.items.length === 0) return;
    this.opening = opening;
    this.progress.jump(0);
    if (opening) this.progress.springTo(1);
    else this.progress.curveTowards(1, undefined, now);
    this.until = now + stay;
    this.place();
  }

  /** One frame. True while a copy is on its way or waiting for the real thing. */
  step(dt: number, now: number): boolean {
    if (this.items.length === 0) return false;
    this.progress.step(dt, now);
    if (!this.progress.animating && now >= this.until) {
      this.finish();
      return false;
    }
    this.place();
    return true;
  }

  /** Arrived, or cut short: the copies go and the real elements are seen again. */
  finish() {
    for (const { copy } of this.items) copy.remove();
    for (const el of this.hidden) el.classList.remove("travelling");
    this.items = [];
    this.hidden = [];
  }

  private place() {
    const p = this.progress.value;
    // On the strip things stand in a row, in the open island often one under
    // another: opening, a copy first drops out of the row and then slides to its
    // column; folding, it slides out first and then rises. A straight line would
    // take a title through the mascot that is on the same journey.
    const [px, py] = this.opening ? [late(p), early(p)] : [early(p), late(p)];
    for (const { copy, from, to, scale } of this.items) {
      const x = lerp(from.x, to.x, px);
      const y = lerp(from.cy, to.cy, py) - to.h / 2;
      copy.style.transform = `translate(${x.toFixed(2)}px, ${y.toFixed(2)}px) scale(${lerp(scale, 1, p).toFixed(4)})`;
    }
  }
}

// Island geometry (docs/ARCHITECTURE.md §4, §7). All values are logical pixels.

export type IslandMode = "hidden" | "compact" | "expanded";

export type IslandViewName =
  | "overview"
  | "empty"
  | "approval"
  | "question"
  | "error"
  | "finished"
  | "confused"
  | "upload"
  | "uploading"
  | "choose"
  | "mail"
  | "prompt"
  | "searching"
  | "result"
  | "note"
  | "settings"
  | "greeting"
  | "recap"
  /** A feature's section (src/island/sections.ts says which). */
  | "section"
  /** The grid of every section (Ctrl+K). */
  | "gallery";

export type BotStateName =
  | "idle"
  | "working"
  | "thinking"
  | "searching"
  | "approval"
  | "question"
  | "error"
  | "finished"
  | "ratelimit"
  | "sleeping"
  | "dizzy";

export type BotEmoteName = "love" | "surprised" | "proud" | "wink" | "yawn" | "happy" | "annoyed";

export interface ViewLayout {
  height: number;
  /** Centre of the mascot from the island's left edge. */
  botX: number;
  /** From the island's top edge; null = centred in the content area; negative = from the bottom edge. */
  botY: number | null;
  /** 0 = no mascot on this view. */
  botDiameter: number;
}

// The window is a fixed 720×360, the largest island (320) and room for what
// overhangs it; the island is drawn inside, glued to the top edge and centred.
// Same constants in src-tauri/src/island.rs.
export const PANEL_W = 720;
export const PANEL_H = 360;

/** Hidden: the island retracts to nothing behind the wake strip. */
export const NOTCH_W = 240;
/** The compact strip. */
export const NOTCH_H = 32;
/** Peek on the pills; a live activity and a notice are wider (§7). */
export const COMPACT_W = 288;
export const ACTIVITY_W = 340;
export const NOTICE_W = 380;
export const EXPANDED_W = 640;

/**
 * `w` as the nearest width whose half is a whole number of device pixels. The
 * island is centred with translateX(-50%) and what it shows is centred inside
 * it: with any other width both sit between two pixels, and text and icons are
 * drawn soft for as long as the width animates (what was painted then in a
 * layer of its own stays so once it has settled) and, on a display scaled to
 * 125 %, at rest too under the 340 and 380 px strips. Upstream rounded the
 * offset instead (`windows/src/island/island.ts`, pull request 55); here the
 * content is centred as well, so it is the width that is rounded.
 */
export function pixelWidth(w: number, dpr = typeof devicePixelRatio === "number" ? devicePixelRatio : 1): number {
  return (Math.round((w / 2) * dpr) * 2) / dpr;
}

export const ROUNDED_CORNER = 14; // --r-md: hidden and compact
export const EXPANDED_CORNER = 22; // --r-lg

/** --island-header and --island-inset: a view's content is height − 40 − 16 tall, 608 wide. */
export const HEADER_H = 40;
export const INSET = 16;
/** What a section may declare as its height. */
export const SECTION_MIN_H = 160;
export const SECTION_MAX_H = 320;

/** Invisible hover strip that wakes the island when hidden. */
export const WAKE_STRIP_W = 240;
export const WAKE_STRIP_H = 6;

// The mascot is one canvas that glides between these places. In an alert card
// it is the 22 px mark of the card's first row (inset 16 + card padding 12);
// on the overview it heads the focus column (184 wide, so its centre is at 108).
const MARK = { botX: INSET + 12 + 11, botY: HEADER_H + 12 + 11, botDiameter: 22 };

export const VIEW_LAYOUTS: Record<IslandViewName, ViewLayout> = {
  overview: { height: 188, botX: 108, botY: 73, botDiameter: 58 },
  empty: { height: 160, botX: 64, botY: null, botDiameter: 44 },
  approval: { height: 204, ...MARK },
  question: { height: 204, ...MARK },
  error: { height: 188, ...MARK },
  finished: { height: 180, ...MARK },
  confused: { height: 160, botX: 72, botY: null, botDiameter: 58 },
  // The drop sequence draws these three itself (src/upload); this is only where
  // the island's own mascot waits underneath.
  upload: { height: 176, botX: 146, botY: 100, botDiameter: 62 },
  uploading: { height: 176, botX: 52, botY: 99, botDiameter: 20 },
  choose: { height: 176, botX: 64, botY: null, botDiameter: 44 },
  mail: { height: 160, botX: 64, botY: null, botDiameter: 44 },
  // Left of the composer, on the island's bottom inset.
  prompt: { height: 216, botX: INSET + 11, botY: -(INSET + 14), botDiameter: 22 },
  searching: { height: 160, botX: 64, botY: null, botDiameter: 44 },
  result: { height: 160, botX: 64, botY: null, botDiameter: 44 },
  note: { height: 160, botX: 64, botY: null, botDiameter: 44 },
  settings: { height: 188, botX: 0, botY: null, botDiameter: 0 },
  greeting: { height: 150, botX: 320, botY: 90, botDiameter: 0 },
  recap: { height: 224, botX: 0, botY: null, botDiameter: 0 },
  // A section declares its own height; the gallery sizes itself to its rows.
  section: { height: SECTION_MIN_H, botX: 0, botY: null, botDiameter: 0 },
  gallery: { height: 200, botX: 0, botY: null, botDiameter: 0 },
};

/** The question view with options to pick from: room for two rows of them. */
export const QUESTION_PICKER_H = 244;

/** The chat grows with the conversation. */
export function chatPromptHeight(messageCount: number): number {
  return Math.min(296, VIEW_LAYOUTS.prompt.height + messageCount * 40);
}

export function islandSize(
  mode: IslandMode,
  view: IslandViewName,
  chatCount = 0,
): { w: number; h: number } {
  switch (mode) {
    case "hidden":
      return { w: NOTCH_W, h: 0 };
    case "compact":
      return { w: COMPACT_W, h: NOTCH_H };
    case "expanded": {
      const h = view === "prompt" ? chatPromptHeight(chatCount) : VIEW_LAYOUTS[view].height;
      return { w: EXPANDED_W, h };
    }
  }
}

export interface BotPlacement {
  cx: number;
  cy: number;
  diameter: number;
  opacity: number;
}

/** Where the mascot sits; cy is measured from the island's top edge. */
export function botPosition(mode: IslandMode, view: IslandViewName, islandH: number, uploadProgress = 0): BotPlacement {
  switch (mode) {
    case "hidden":
      return { cx: 22, cy: 16, diameter: 12, opacity: 0 };
    case "compact":
      // The mark of the strip: 28 px, 8 px from the edge.
      return { cx: 22, cy: NOTCH_H / 2, diameter: 28, opacity: 1 };
    case "expanded": {
      const layout = VIEW_LAYOUTS[view];
      const diameter = layout.botDiameter;
      const opacity = diameter > 0 ? 1 : 0;
      if (view === "uploading") return { cx: 36 + uploadProgress * 526, cy: layout.botY ?? 103, diameter, opacity };
      const cy =
        layout.botY == null
          ? HEADER_H + (islandH - HEADER_H - INSET) / 2
          : layout.botY < 0
            ? islandH + layout.botY
            : layout.botY;
      return { cx: layout.botX, cy, diameter, opacity };
    }
  }
}

// Fonts that carry the scripts of the ten interface languages, appended to the
// existing stacks before the generic family. Latin and Cyrillic text keeps
// rendering in the fonts it always had; these only fill in the characters
// those lack: Devanagari and Bengali (Nirmala UI), Simplified Chinese
// (Microsoft YaHei) and Arabic (Segoe UI, already first) on Windows, the Noto
// families on Linux.
export const SCRIPT_FONTS =
  `"Nirmala UI", "Microsoft YaHei UI", "Microsoft YaHei", "Noto Sans", "Noto Sans Devanagari", ` +
  `"Noto Sans Bengali", "Noto Sans Arabic", "Noto Sans CJK SC", "Noto Sans SC", "Droid Sans Fallback"`;

/** The interface font, bundled through src/ui/tokens.css. Canvas text names it first. */
export const UI_FONT = `"Poppins", "Hind Siliguri", "Montserrat", "Cairo"`;
// Canvas drawing does not wait for a web font by itself: ask for the weights it uses up front.
if (typeof document !== "undefined") for (const w of [400, 500, 600]) {
  void document.fonts?.load(`${w} 13px Poppins`);
  void document.fonts?.load(`${w} 13px "Hind Siliguri"`, "ক");
  void document.fonts?.load(`${w} 13px Montserrat`, "Ж");
  void document.fonts?.load(`${w} 13px Cairo`, "ا");
}

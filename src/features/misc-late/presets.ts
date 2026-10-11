// Start with a bundle: what each preset turns on, what that touches, and the
// chooser both the first-run page and Settings draw. Applying makes the preset
// the whole set of features that are on (misc-late itself is never turned off)
// and only ever happens after a click on Apply.

import { Bridge, type FeatureInfo } from "../../core/bridge";
import { N_, t, tn } from "../../i18n/i18n";
import { btn } from "../../ui/components";
import { icon } from "../../ui/icons";
import { h } from "../../views/dom";

export interface Preset {
  id: string;
  name: string;
  caption: string;
  icon: string;
  /** Feature ids; "all" is every feature that never needs administrator rights. */
  features: string[] | "all";
}

/** The three bundles. Names and captions go through `t()` where they are drawn. */
export const PRESETS: Preset[] = [
  {
    id: "agents",
    name: N_("Agents only"),
    caption: N_("Your coding agents on the island and nothing else. Nothing listens to the keyboard."),
    icon: "terminal",
    features: ["agents", "agent-usage", "recap", "integrations", "drop"],
  },
  {
    id: "essentials",
    name: N_("Essentials"),
    caption: N_("Agents, chat, music, clipboard, shelf, timers and the system heads-up displays."),
    icon: "island",
    features: [
      "agents", "agent-usage", "recap", "integrations", "drop", "chat", "media", "clipboard", "shelf",
      "timers-downloads", "huds-power", "calendar", "command-bar",
    ],
  },
  {
    id: "everything",
    name: N_("Everything"),
    caption: N_("Every tool, except what needs administrator rights."),
    icon: "grid",
    features: "all",
  },
];

/** What a feature does that you should know about, in plain words. */
export const USE_TEXT: Record<FeatureInfo["uses"][number], { name: string; icon: string; about: string }> = {
  network: { name: N_("Network"), icon: "wifi", about: N_("Reaches the internet, only to services you set up yourself.") },
  microphone: { name: N_("Microphone"), icon: "mic", about: N_("Listens through the microphone, only while you use it.") },
  camera: { name: N_("Camera"), icon: "camera", about: N_("Uses the camera, only while its preview is open.") },
  clipboard: { name: N_("Clipboard"), icon: "clipboard", about: N_("Reads and writes what you copy.") },
  inputHook: { name: N_("Keyboard and mouse"), icon: "keyboard", about: N_("Sees every key and click as it happens, so it can react to your shortcuts.") },
  capture: { name: N_("Screen capture"), icon: "shot", about: N_("Reads what is on the screen when you ask it to.") },
  elevation: { name: N_("Administrator rights"), icon: "shield", about: N_("Asks for administrator rights, only after you agree.") },
  fileDelete: { name: N_("Deleting files"), icon: "trash", about: N_("Can delete files, only the ones you choose.") },
};

/** The features a preset names that this build has. */
export function membersOf(p: Preset, infos: readonly FeatureInfo[]): FeatureInfo[] {
  if (p.features === "all") return infos.filter((f) => !f.uses.includes("elevation"));
  return p.features.map((id) => infos.find((f) => f.id === id)).filter((f): f is FeatureInfo => !!f);
}

/** What applying would change: switches that would flip. */
export function diffOf(p: Preset, infos: readonly FeatureInfo[]) {
  const keep = new Set(membersOf(p, infos).map((f) => f.id));
  keep.add("misc-late");
  return {
    on: infos.filter((f) => keep.has(f.id) && !f.on),
    off: infos.filter((f) => !keep.has(f.id) && f.on),
    stay: infos.filter((f) => keep.has(f.id) && f.on),
  };
}

/** Flips the switches. Resolves with the names of the ones that refused to start. */
export async function applyPreset(p: Preset, infos: readonly FeatureInfo[]): Promise<string[]> {
  const { on, off } = diffOf(p, infos);
  const failed: string[] = [];
  for (const f of off) await Bridge.featureSetEnabled(f.id, false).catch(() => failed.push(f.name));
  for (const f of on) await Bridge.featureSetEnabled(f.id, true).catch(() => failed.push(f.name));
  return failed;
}

const chips = (list: readonly FeatureInfo[]) =>
  h("div", { class: "f-misc-late-chips" }, ...list.map((f) => h("span", { class: "f-misc-late-chip", text: t(f.name) })));

/** What a preset hooks into, as a short list of words. */
function usesOf(members: readonly FeatureInfo[]): HTMLElement {
  const kinds = [...new Set(members.flatMap((f) => f.uses))];
  if (!kinds.length) return h("p", { class: "hint", text: t("Nothing here listens to the keyboard, reads the screen or the clipboard.") });
  return h(
    "div",
    { class: "f-misc-late-uses" },
    ...kinds.map((k) =>
      h("div", { class: "f-misc-late-use" }, icon(USE_TEXT[k].icon, 14), h("b", { text: t(USE_TEXT[k].name) }), h("span", { text: t(USE_TEXT[k].about) })),
    ),
  );
}

/**
 * Three cards to pick from; the one picked shows what it turns on and what that
 * touches. `onApply` gets the preset once the user has confirmed.
 */
export function presetChooser(infos: readonly FeatureInfo[], onApply: (p: Preset) => void | Promise<void>, applyLabel: string): HTMLElement {
  let picked: Preset | null = null;
  let confirming = false;
  const root = h("div", { class: "f-misc-late-presets" });

  const draw = () => {
    root.replaceChildren();
    const cards = h("div", { class: "f-misc-late-cards" });
    for (const p of PRESETS) {
      const n = membersOf(p, infos).length;
      cards.append(
        h(
          "button",
          { class: `f-misc-late-pcard${picked === p ? " on" : ""}`, "aria-pressed": String(picked === p), onclick: () => { picked = p; confirming = false; draw(); } },
          h("span", { class: "f-misc-late-pico" }, icon(p.icon, 20)),
          h("b", { text: t(p.name) }),
          h("span", { class: "f-misc-late-pcap", text: t(p.caption) }),
          h("span", { class: "f-misc-late-pn", text: tn("{count} feature", "{count} features", n, { count: n }) }),
        ),
      );
    }
    root.append(cards);
    if (!picked) {
      root.append(h("p", { class: "hint", text: t("Pick a bundle to see what it turns on.") }));
      return;
    }
    const d = diffOf(picked, infos);
    const members = membersOf(picked, infos);
    root.append(
      h("div", { class: "f-misc-late-detail" },
        h("div", { class: "section-title", text: t("Turns on") }),
        members.length ? chips(members) : h("p", { class: "hint", text: t("Nothing.") }),
        h("div", { class: "section-title", text: t("What it touches") }),
        usesOf(members),
        d.off.length
          ? h("p", { class: "hint", text: tn("Applying also turns off {count} feature that is on now.", "Applying also turns off {count} features that are on now.", d.off.length, { count: d.off.length }) })
          : null,
      ),
    );
    const go = btn(applyLabel, "primary", () => {
      if (!confirming) {
        confirming = true;
        go.replaceChildren(h("span", { text: t("Turn on {count}, turn off {off}", { count: d.on.length, off: d.off.length }) }));
        return;
      }
      go.disabled = true;
      void Promise.resolve(onApply(picked!));
    });
    root.append(h("div", { class: "f-misc-late-actions" }, go));
  };
  draw();
  return root;
}

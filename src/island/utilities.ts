// Utilities: every tool the running features offer as a global shortcut, one
// click away in the island (docs/ARCHITECTURE.md §5.5). A tool here does what
// its key does; nothing is listed for a feature that is off. The list is read
// from the registry each time the page comes up.

import { Bridge } from "../core/bridge";
import type { SectionHost } from "../core/feature";
import { featureById, featureInfos } from "../core/registry";
import { displayKeys } from "../core/shortcuts";
import { State } from "../core/state";
import { t } from "../i18n/i18n";
import { list, type ListItem } from "../ui/components";
import { h } from "../views/dom";

/**
 * `run` is the island's: it folds first, so the window the user was in is in
 * front again by the time the tool acts (a layout or a paste is for that window).
 */
export function buildUtilities(run: (feature: string, id: string) => void): SectionHost {
  const el = h("div", { class: "utilities" });

  function draw() {
    const items: ListItem[] = featureInfos()
      .filter((f) => f.on && f.running)
      .flatMap((f) =>
        f.shortcuts.map((sc) => {
          const keys = State.settings.shortcuts?.[`${f.id}:${sc.id}`]?.keys ?? sc.default ?? "";
          return {
            icon: featureById(f.id)?.sections?.[0]?.icon ?? "sparkle",
            label: t(sc.label),
            meta: t(f.name),
            kbd: keys ? displayKeys(keys) : undefined,
            onClick: () => run(f.id, sc.id),
          };
        }),
      );
    el.replaceChildren(
      items.length > 0 ? list(items) : h("div", { class: "sub", text: t("No tools yet. Features you turn on in Settings add theirs.") }),
    );
  }

  return { el, show: draw, hide() {}, sync() {} };
}

/** What a click on a tool asks the host for: the same as its key being pressed. */
export const runTool = (feature: string, id: string) => void Bridge.featureAction(feature, id);

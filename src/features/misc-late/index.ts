// Tools and setup — the page side (docs/ARCHITECTURE.md §2.3). Nothing runs at
// import. On the island page: the watch's reading in the strip, the notices for
// a pasted picture and the Explorer selection (handed to the shelf), and, on the
// first run only, the Settings window with the first-run page. The cleaning
// window (the way out of the lock) is a page of its own.

import "./style.css";
import { Bridge } from "../../core/bridge";
import { defineFeature, type FeatureCtx } from "../../core/feature";
import { btn, stripRow } from "../../ui/components";
import { h } from "../../views/dom";
import { needsOnboarding } from "./onboarding";
import { settingsBody } from "./page";

interface WatchEvent { on: boolean; headline?: string | null; matched?: boolean; error?: string | null; title?: string }
interface CleaningEvent { on: boolean; count?: number; need?: number; remaining?: number; limit?: number }

let watching: { headline: string; title: string } | null = null;
let tr: FeatureCtx["t"] = (k) => k;

const clock = (s: number) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

/** The window that holds the lock's way out: always up while the keyboard is locked. */
function mountCleaning(root: HTMLElement, cx: FeatureCtx) {
  const t = cx.t;
  const need = 5;
  const dots = Array.from({ length: need }, () => h("span", { class: "f-misc-late-dot" }));
  const left = h("b", { class: "f-misc-late-count", text: "" });
  const sub = h("p", { class: "f-misc-late-clean-sub", text: t("Press Esc {n} times in a row to unlock.", { n: need, count: need }) });
  const paint = (e: CleaningEvent) => {
    dots.forEach((d, i) => d.classList.toggle("on", i < (e.count ?? 0)));
    if (e.remaining != null) left.textContent = clock(e.remaining);
  };
  root.replaceChildren(
    h("div", { class: "win f-misc-late-clean" },
      h("div", { class: "f-misc-late-clean-body" },
        h("h1", { text: t("Keyboard locked for cleaning") }),
        sub,
        h("div", { class: "f-misc-late-dots" }, ...dots),
        h("p", { class: "f-misc-late-clean-sub" }, t("Unlocks by itself in "), left),
        btn(t("Unlock now"), "primary", () => void cx.call("cleaning_stop")),
        h("p", { class: "f-misc-late-clean-note", text: t("The mouse still works. Ctrl+Alt+Del and Win+L are Windows' own and still work too.") }),
      ),
    ),
  );
  paint({ on: true, count: 0 });
  cx.on<CleaningEvent>("cleaning", paint);
}

export default defineFeature({
  id: "misc-late",

  settings: settingsBody,

  windows: { cleaning: mountCleaning },

  activities: [
    {
      id: "watch",
      order: 1,
      isActive: () => watching != null,
      render: () => stripRow({ icon: "eye", tone: "info", title: watching?.headline || "…", detail: watching?.title ?? "" }),
    },
  ],

  start(cx) {
    tr = cx.t;
    const offs = [
      cx.on<WatchEvent>("watch", (e) => {
        const was = watching;
        if (e.matched || e.error) {
          watching = null;
          cx.island.activityChanged("watch");
          cx.island.notice({
            kind: "watch",
            title: e.matched ? tr("It happened: {headline}", { headline: e.headline ?? "" }) : tr("The watch stopped"),
            detail: e.matched ? e.title : (e.error ?? ""),
            icon: "eye",
            tone: e.matched ? "ok" : "warn",
          });
        } else if (e.on) {
          watching = { headline: e.headline ?? "", title: e.title ?? "" };
          // Reveal only when the watch starts; later readings redraw where they stand.
          if (!was) cx.island.activityChanged("watch");
          else cx.island.redraw("watch");
        } else {
          watching = null;
          cx.island.activityChanged("watch");
        }
      }),
      cx.on<{ path: string; name: string | null }>("pasted", (p) =>
        cx.island.notice({ kind: "clipboard", title: tr("Saved as a file"), detail: p.name ?? p.path, icon: "image" })),
      // The shelf is another feature: it is asked through its own command, whatever its page is doing.
      cx.on<{ paths: string[] }>("selection", async ({ paths }) => {
        const added = await Bridge.featureCall<number>("shelf", "added", { paths });
        cx.island.notice({
          kind: "clipboard",
          title: added == null ? tr("The shelf is turned off") : tr("Added to the shelf"),
          detail: added == null ? "" : tr("{count} from Explorer", { count: added }),
          icon: "shelf",
        });
      }),
    ];
    // First run: bring up Settings, which shows the welcome page until it is dismissed.
    void needsOnboarding().then((first) => {
      if (first) void Bridge.openSettingsWindow();
    });
    return () => {
      offs.forEach((off) => void Promise.resolve(off).then((f) => f?.()));
      watching = null;
    };
  },
});

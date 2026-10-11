// The Dashboard: the island's Home page of smart cards (docs/ARCHITECTURE.md §5.5).
// Two cards, each with faces that swap by what matters now (the player and
// the agents; the levels and the next event), and a row under them: the tool
// last opened and a system readout. Everything comes from what the features
// that are on already answer to; a face whose feature is off is not there.
// Nothing runs unless the page is on screen: `show` starts it, `hide` ends it.

import { Bridge, onEvent } from "../core/bridge";
import type { SectionHost } from "../core/feature";
import { featureOn } from "../core/registry";
import { State } from "../core/state";
import { N_, t } from "../i18n/i18n";
import { bars, card, meter } from "../ui/components";
import { icon } from "../ui/icons";
import { clear, h } from "../views/dom";
import { taskLine } from "../views/views";

/** What the page needs from the island. */
export interface HomeHost {
  openSection(key: string): void;
  /** The agents' own page: the overview. */
  openAgents(): void;
  /** The section last opened, other than this one. */
  lastTool(): { key: string; icon: string; title: string } | null;
  /** The section on screen right now, if any. */
  section(): string | null;
}

interface Face {
  icon: string;
  label: string;
  /** Its feature is on. */
  there(): boolean;
  /** It is what matters now: it leads its card when the page comes up. */
  leads(): boolean;
  draw(): Array<Node | null>;
  open(): void;
}

interface Track { title: string; artist: string; playing: boolean; source?: { name?: string } }
interface Level { level: number; mute: boolean }
interface Power { ac: boolean; percent: number | null; charging: boolean }
interface Coming { start: number; inProgress: boolean; events: Array<{ title: string }> }
interface Snap { cpu: number | null; mem: { total: number; used: number } | null }

/** How often what has no event of its own (the track, the next event, the battery) is asked again while the page is up. */
const REFRESH_MS = 5000;

const ask = <T>(feature: string, method: string, args?: object): Promise<T | null> =>
  featureOn(feature) ? Bridge.featureCall<T>(feature, method, args).catch(() => null) : Promise.resolve(null);

const working = () => State.tasks.some((task) => task.state !== "idle" && task.state !== "sleeping");

const line = (name: string, label: string, fraction: number, tone?: "warn") =>
  h("div", { class: "home-line" }, icon(name, 14), h("span", { text: label }), meter(fraction, tone), h("span", { class: "isl-num", text: String(Math.round(fraction * 100)) }));

export function buildHome(host: HomeHost): SectionHost {
  let track: Track | null = null;
  let out: Level | null = null;
  let power: Power | null = null;
  let coming: Coming | null = null;
  let snap: Snap | null = null;
  let timer: number | null = null;
  let offs: Array<() => void> = [];
  let up = false;
  /** The face each card is on; chosen when the page comes up, then the user's. */
  const at = [0, 0];
  let drawn = "";

  const text = (main: string, sub: string) => [h("b", { class: "home-main", text: main }), h("span", { class: "home-sub", text: sub })];

  const player: Face = {
    icon: "music",
    label: N_("Now playing"),
    there: () => featureOn("media"),
    leads: () => track?.playing === true,
    draw: () =>
      track?.title
        ? [...text(track.title, [track.artist, track.source?.name].filter(Boolean).join(" · ")), h("span", { class: "home-foot" }, bars(!track.playing))]
        : text(t("Nothing playing"), t("Music")),
    open: () => host.openSection("media.music"),
  };
  const agents: Face = {
    icon: "sparkle",
    label: N_("Agents"),
    there: () => featureOn("agents"),
    leads: () => working(),
    draw: () => {
      const task = working() ? State.focusTask : null;
      const others = State.tasks.filter((o) => o !== task && o.state !== "idle" && o.state !== "sleeping").slice(0, 4);
      const dots = h("span", { class: "home-foot isl-grid2" });
      for (const o of others) dots.append(h("i", { style: `--c:${o.color}` }));
      return task ? [...text(task.name, taskLine(task).text), others.length > 0 ? dots : null] : text(t("No agent at work"), t("Agents"));
    },
    open: () => host.openAgents(),
  };
  const levels: Face = {
    icon: "sliders",
    label: N_("Levels"),
    there: () => featureOn("audio-mixer") || featureOn("huds-power"),
    leads: () => false,
    draw: () => [
      out ? line(out.mute ? "mute" : "volume", t("Volume"), out.mute ? 0 : out.level) : null,
      power?.percent != null && power.percent <= 100 ? line("battery", t("Battery"), power.percent / 100, !power.ac && power.percent <= 20 ? "warn" : undefined) : null,
      !out && !(power?.percent != null && power.percent <= 100) ? h("span", { class: "home-sub", text: t("Levels") }) : null,
    ],
    open: () => host.openSection(featureOn("audio-mixer") ? "audio-mixer.mixer" : "huds-power.battery"),
  };
  const next: Face = {
    icon: "calendar",
    label: N_("Next event"),
    there: () => featureOn("calendar"),
    // An event of the coming hour, or one that is on, leads before the levels.
    leads: () => coming != null && (coming.inProgress || coming.start - Date.now() < 60 * 60_000),
    draw: () => {
      if (!coming?.events.length) return text(t("Nothing coming up"), t("Calendar"));
      const when = new Date(coming.start).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
      const more = coming.events.length - 1;
      return text(coming.events[0].title + (more > 0 ? ` +${more}` : ""), coming.inProgress ? t("Now") : when);
    },
    open: () => host.openSection("calendar.calendar"),
  };
  const CARDS: Face[][] = [[player, agents], [levels, next]];

  const cardsEl = h("div", { class: "home-cards" });
  const rowEl = h("div", { class: "home-row" });
  const el = h("div", { class: "home" }, cardsEl, rowEl);

  /** The cards the user chose (Settings › Island "Dashboard cards"), each with the faces whose features are on. */
  const shown = () =>
    CARDS.map((faces, i) => ({ i, faces: faces.filter((f) => f.there()) })).filter(
      ({ i, faces }) => faces.length > 0 && (State.settings.homeCards === "both" || State.settings.homeCards === (i === 0 ? "now" : "levels")),
    );

  function draw() {
    const cards = shown();
    clear(cardsEl);
    for (const { i, faces } of cards) {
      const face = faces[Math.min(at[i], faces.length - 1)];
      const dots = h("span", { class: "home-dots" });
      if (faces.length > 1) {
        faces.forEach((f, n) =>
          dots.append(
            h("button", {
              class: f === face ? "on" : "",
              title: t(f.label),
              onclick: (e: Event) => {
                e.stopPropagation();
                at[i] = n;
                draw();
              },
            }),
          ),
        );
      }
      const box = card([h("div", { class: "home-head" }, icon(face.icon, 14), h("span", { text: t(face.label) }), h("i"), dots), ...face.draw()]);
      box.classList.add("home-card");
      box.addEventListener("click", () => face.open());
      cardsEl.append(box);
    }
    if (cards.length === 0) cardsEl.append(h("div", { class: "sub", text: t("Nothing to show here yet. Features you turn on in Settings add their cards.") }));

    clear(rowEl);
    const last = host.lastTool();
    if (last) {
      rowEl.append(h("button", { class: "capsule home-chip", title: t("Last opened"), onclick: () => host.openSection(last.key) }, icon(last.icon, 14), h("span", { text: t(last.title) })));
    }
    rowEl.append(h("span", { class: "isl-gap" }));
    if (featureOn("sysmon") && snap) {
      const chip = h("button", { class: "capsule home-chip", title: t("System"), onclick: () => host.openSection("sysmon.system") }, icon("cpu", 14));
      if (snap.cpu != null) chip.append(h("span", { text: `${t("CPU")} ${Math.round(snap.cpu * 100)}%` }));
      if (snap.mem) chip.append(h("span", { class: "home-dim", text: `${t("Memory")} ${Math.round((snap.mem.used / snap.mem.total) * 100)}%` }));
      rowEl.append(chip);
    }
  }

  /** Draws again only when something it shows has changed. */
  function sync() {
    if (!up) return;
    const task = State.focusTask;
    const key = JSON.stringify([
      State.settings.homeCards, at, track, out, power, coming, host.lastTool()?.key,
      snap && [snap.cpu != null && Math.round(snap.cpu * 100), snap.mem && Math.round((snap.mem.used / snap.mem.total) * 100)],
      working(), task?.id, task && taskLine(task).text, State.tasks.map((o) => o.state + o.color),
      CARDS.flat().map((f) => f.there()), featureOn("sysmon"),
    ]);
    if (key === drawn) return;
    drawn = key;
    draw();
  }

  async function refresh() {
    const [media, mixer, hud, cards] = await Promise.all([
      ask<{ track: Track | null }>("media", "state"),
      // The level is told as it changes (below): the devices are listed once.
      out ? null : ask<{ out: Level | null }>("audio-mixer", "devices"),
      ask<{ power: Power }>("huds-power", "state"),
      ask<Coming[]>("calendar", "cards", { lead: 240, extra: [] }),
    ]);
    if (!up) return;
    // No reading yet: the monitor's own page may have ended the watch as it left. Asked again.
    if (featureOn("sysmon") && !snap) void ask("sysmon", "watch", { on: true, detail: "" });
    track = media?.track ?? null;
    if (mixer?.out) out = mixer.out;
    power = hud?.power ?? null;
    coming = Array.isArray(cards) ? cards[0] ?? null : null;
    sync();
  }

  return {
    el,
    show() {
      up = true;
      drawn = "";
      void refresh().then(() => {
        // The face that matters now leads; from here on the card is the user's.
        shown().forEach(({ i, faces }) => (at[i] = Math.max(0, faces.findIndex((f) => f.leads()))));
        drawn = "";
        sync();
      });
      timer = window.setInterval(() => void refresh(), REFRESH_MS);
      const hear = <T>(name: string, fn: (p: T) => void) =>
        void onEvent<T>(name, (p) => {
          if (!up) return;
          fn(p);
          sync();
        }).then((off) => (up ? offs.push(off) : off()));
      hear<Level>("huds-power:volume", (v) => (out = { level: v.level, mute: v.mute }));
      // The watch itself is asked for by the first refresh, and again by a later one while no reading has come.
      if (featureOn("sysmon")) hear<{ snap: Snap }>("sysmon:tick", (said) => (snap = said.snap));
      sync();
    },
    hide() {
      up = false;
      if (timer != null) window.clearInterval(timer);
      timer = null;
      for (const off of offs) off();
      offs = [];
      out = null;
      snap = null;
      // Left for the monitor's own page: that one watches now, and ends the watch itself.
      if (host.section() !== "sysmon.system") void ask("sysmon", "watch", { on: false });
    },
    sync,
  };
}

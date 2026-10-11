// Weather — the page side of the feature (docs/ARCHITECTURE.md §2.3).
// Rust fetches Open-Meteo every 30 minutes and keeps Celsius; this file draws the section and the
// settings, converts to Fahrenheit and picks the icons (inline paths, same style as src/ui/icons.ts).

import "./style.css";
import { defineFeature, type FeatureCtx } from "../../core/feature";
import { h, svg } from "../../views/dom";
import { tn } from "../../i18n/i18n";
import { btn, card, field, row, toggle } from "../../ui/components";

interface Place { name: string; region: string; country: string; lat: number; lon: number }
interface Hour { at: string; temp: number; code: number; day: boolean }
interface Forecast { temp: number; code: number; day: boolean; high: number | null; low: number | null; hours: Hour[] }
interface State { place: Place | null; forecast: Forecast | null; at: number; failed: boolean }

// ── icons ────────────────────────────────────────────────────────────────────

const CLOUD = "M7.5 18.5a4.2 4.2 0 0 1-.6-8.4 5.6 5.6 0 0 1 10.7 1.2 3.7 3.7 0 0 1-.7 7.2z";
const CLOUD_UP = "M7.5 15a4 4 0 0 1-.5-7.9 5.4 5.4 0 0 1 10.3 1.1A3.5 3.5 0 0 1 17 15z";
const ICONS: Record<string, string> = {
  sun: "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8zM12 2.5v2M12 19.5v2M2.5 12h2M19.5 12h2M5.3 5.3l1.4 1.4M17.3 17.3l1.4 1.4M5.3 18.7l1.4-1.4M17.3 6.7l1.4-1.4",
  moon: "M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z",
  partly: "M8 3v1.5M3 8h1.5M4.6 4.6l1 1M11.4 4.6l-1 1M8 6.2a2.3 2.3 0 0 0 0 4.6M10 20h7.5a3.2 3.2 0 0 0 .3-6.4 4.4 4.4 0 0 0-8.3 1A2.7 2.7 0 0 0 10 20z",
  partlyNight: "M9.5 3.5a3.8 3.8 0 0 0 4.2 5.6 3.8 3.8 0 0 1-4.2-5.6zM10 20h7.5a3.2 3.2 0 0 0 .3-6.4 4.4 4.4 0 0 0-8.3 1A2.7 2.7 0 0 0 10 20z",
  cloud: CLOUD,
  fog: "M5 8h14M3 12h18M6 16h12M9 20h6",
  rain: `${CLOUD_UP}M8 18l-1 2.5M12 18l-1 2.5M16 18l-1 2.5`,
  snow: `${CLOUD_UP}M8 18.5h.01M12 18.5h.01M16 18.5h.01M10 21h.01M14 21h.01`,
  storm: `${CLOUD_UP}M12.5 15.5l-2.5 3h3l-2 3`,
};

/** The condition for a WMO weather code: its English label and the icon key. */
function sky(code: number, day: boolean): { label: string; icon: string } {
  if (code === 0) return { label: "Clear", icon: day ? "sun" : "moon" };
  if (code === 1) return { label: "Mostly clear", icon: day ? "sun" : "moon" };
  if (code === 2) return { label: "Partly cloudy", icon: day ? "partly" : "partlyNight" };
  if (code === 3) return { label: "Overcast", icon: "cloud" };
  if (code === 45 || code === 48) return { label: "Fog", icon: "fog" };
  if (code >= 51 && code <= 55) return { label: "Drizzle", icon: "rain" };
  if (code === 56 || code === 57 || code === 66 || code === 67) return { label: "Freezing rain", icon: "rain" };
  if (code >= 61 && code <= 65) return { label: "Rain", icon: "rain" };
  if (code >= 71 && code <= 77) return { label: "Snow", icon: "snow" };
  if (code >= 80 && code <= 82) return { label: "Showers", icon: "rain" };
  if (code === 85 || code === 86) return { label: "Snow showers", icon: "snow" };
  if (code >= 95) return { label: "Thunderstorm", icon: "storm" };
  return { label: "Overcast", icon: "cloud" };
}
const wicon = (key: string, size: number) => svg(ICONS[key] ?? ICONS.cloud, size, { stroke: 1.6 });

// ── state shared by the section and the feature ──────────────────────────────

const EMPTY: State = { place: null, forecast: null, at: 0, failed: false };
let S: State = EMPTY;
let fahrenheit = false;
const watchers = new Set<() => void>();

const num = (c: number | null | undefined) => (c == null ? "–" : String(Math.round(fahrenheit ? (c * 9) / 5 + 32 : c)));
const deg = (c: number | null | undefined) => `${num(c)}°`;

function ageText(cx: FeatureCtx, ms: number): string {
  const min = Math.floor((Date.now() - ms) / 60000);
  if (min < 1) return cx.t("Updated just now");
  const age = min < 60 ? tn("{count} minute ago", "{count} minutes ago", min) : tn("{count} hour ago", "{count} hours ago", Math.floor(min / 60));
  return cx.t("Updated {age}", { age });
}

export default defineFeature({
  id: "weather",

  sections: [{ id: "weather", title: "Weather", icon: ICONS.sun, letter: "f", height: 230, build: (cx) => page(cx) }],

  settings: (cx) => settingsPage(cx),

  start(cx) {
    const read = (s: Record<string, unknown>) => {
      fahrenheit = s.fahrenheit === true;
      watchers.forEach((w) => w());
    };
    cx.settings.get().then(read);
    cx.call<State>("state").then((s) => {
      if (s) S = s;
      watchers.forEach((w) => w());
    });
    const offs = [
      cx.settings.onChange(read),
      cx.on<State>("changed", (s) => {
        S = s;
        watchers.forEach((w) => w());
      }),
    ];
    return () => {
      offs.forEach((f) => f());
      watchers.clear();
      S = EMPTY;
    };
  },
});

// ── the section ──────────────────────────────────────────────────────────────

function page(cx: FeatureCtx) {
  const el = h("div", { class: "f-weather-page" });
  let shown = false;

  function draw() {
    const f = S.forecast;
    if (!S.place || !f) {
      el.replaceChildren(h("p", { class: "f-weather-msg", text: cx.t(S.place ? "Waiting for the first reading." : "Choose a city in Settings.") }));
      return;
    }
    const now = sky(f.code, f.day);
    el.replaceChildren(
      h("div", { class: "f-weather-main" },
        wicon(now.icon, 44),
        h("span", { class: "f-weather-temp", text: deg(f.temp) }),
        h("div", { class: "f-weather-cond" },
          h("span", { class: "f-weather-city", text: S.place.name }),
          h("span", { class: "f-weather-sub", text: cx.t(now.label) }),
          h("span", { class: "f-weather-sub", text: cx.t("High {high}° · Low {low}°", { high: num(f.high), low: num(f.low) }) })),
        h("div", { class: `f-weather-age${S.failed ? " bad" : ""}` },
          h("div", { text: ageText(cx, S.at) }),
          S.failed ? h("div", { text: cx.t("Could not refresh. Showing the last reading.") }) : null)),
      card(h("div", { class: "f-weather-hours" },
        ...f.hours.map((x) =>
          h("div", { class: "f-weather-hour", title: cx.t(sky(x.code, x.day).label) },
            h("span", { text: x.at }), wicon(sky(x.code, x.day).icon, 18), h("b", { text: deg(x.temp) }))))),
    );
  }

  return {
    el,
    show() {
      shown = true;
      watchers.add(draw);
      draw();
      // Rust fetches only when its data is older than 30 minutes.
      void cx.call<State>("seen").then((s) => {
        if (s && shown) {
          S = s;
          draw();
        }
      });
    },
    hide() {
      shown = false;
      watchers.delete(draw);
    },
    sync() {},
  };
}

// ── settings ─────────────────────────────────────────────────────────────────

function settingsPage(cx: FeatureCtx) {
  const results = h("div", { class: "f-weather-results" });
  const city = h("div", {});
  const unit = h("div", { class: "f-weather-unit" });
  const label = (p: Place) => [p.name, p.region, p.country].filter(Boolean).join(", ");

  const draw = () =>
    cx.settings.get().then((s) => {
      const p = s.place as Place | null;
      city.replaceChildren(
        // The value sits in the control column: "row-hint" is the cell of the hint below the label.
        row(cx.t("City"), h("span", { class: "f-weather-place", text: p ? label(p) : cx.t("No city chosen") }),
          cx.t("Choose a city. Forecasts come from a free public service; nothing is requested until a city is set.")),
      );
      unit.replaceChildren(
        row(cx.t("Fahrenheit"), toggle(s.fahrenheit === true, (on) => void cx.settings.patch({ fahrenheit: on })),
          cx.t("Show temperatures in °F instead of °C.")),
      );
    });

  async function search(q: string) {
    if (!q.trim()) return;
    const r = await cx.call<Place[]>("search", { query: q });
    results.replaceChildren(
      ...(r === null
        ? [h("p", { class: "f-weather-msg", text: cx.t("The search failed. Check the connection and try again.") })]
        : r.length === 0
          ? [h("p", { class: "f-weather-msg", text: cx.t("No city found.") })]
          : r.map((p) =>
              btn(label(p), "secondary", () => {
                void cx.settings.patch({ place: p }).then(() => {
                  results.replaceChildren();
                  void draw();
                });
              }))),
    );
  }

  const input = field({ placeholder: cx.t("Search for a city"), onEnter: (v) => void search(v) });
  void draw();
  return card([city, h("div", { class: "f-weather-search" }, input, btn(cx.t("Search"), "primary", () => void search(input.input.value))), results, unit]);
}

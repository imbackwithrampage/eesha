// Lyrics — the page side of the feature (docs/ARCHITECTURE.md §2.3).
// Rust sends the song's lines with the player's position. The line being sung is found here
// (clock.ts: one timeout per line, and only while a lyric is on screen) and shown through the
// media feature: a three-line column in the Music section, and the line on its strip row.
// The fade between lines and the column browsed with the wheel: after boring.notch pull
// requests 977 and 1050 (no code of theirs).

import "./style.css";
import { defineFeature, type FeatureCtx } from "../../core/feature";
import { btn, card, row, toggle } from "../../ui/components";
import { h } from "../../views/dom";
import { lyricSeen, mountLyric, setLyric, watchLyric } from "../media/index";
import { follower } from "./clock";

interface State {
  track: boolean;
  fetch: boolean;
  /** "error": the service did not answer, which is not "none". */
  status: "off" | "loading" | "found" | "none" | "error";
  /** Changes with the text: the lines are built again only then. */
  rev: number;
  /** A text without times is only read: nothing follows the position. */
  synced: boolean;
  lines: [number, string][];
  playing: boolean;
  position: number;
  at: number;
  rate: number;
}

/** The column goes back to the line being sung this long after the last wheel step. */
const SNAP_MS = 2000;
/** One wheel step per this long: the burst of a touchpad does not run through the song. */
const STEP_MS = 60;

export default defineFeature({
  id: "lyrics",
  settings: (cx) => {
    const box = card([]);
    void cx.settings.get().then((v) =>
      box.append(
        row(
          cx.t("Fetch lyrics from LRCLIB"),
          toggle(v.fetch === true, (on) => void cx.settings.patch({ fetch: on })),
          cx.t("Sends the title, artist and album of the playing track to lrclib.net. Nothing is sent while this is off."),
        ),
      ),
    );
    return box;
  },
  start: (cx: FeatureCtx) => {
    let st: State | null = null;
    let rev = -1;
    let noted = "";
    let times: number[] = [];
    let texts: string[] = [];
    let rows: HTMLElement[] = [];
    /** The line being sung (-1: none), and the one the wheel has moved to. */
    let current = -1;
    let browse: number | null = null;
    let snapTimer = 0;
    let lastStep = 0;
    /** Where the lyric was last seen ("": nowhere), and whether the column holds lines it has not been drawn with yet. */
    let seen = "";
    let still = true;

    // Every line of the song in one column, laid out once: a line change moves
    // the column and repaints three lines, and nothing is measured anew.
    const track = h("div", { class: "f-lyrics-track" });
    const note = h("div", { class: "f-lyrics-note" });
    const col = h("div", { class: "f-lyrics" }, track, note);
    const retry = btn(cx.t("Try again"), "ghost", () => void cx.call<State>("retry").then((s) => s && apply(s)));
    retry.classList.add("sm");

    /** Marks the line being sung and its neighbours, and brings the middle one to the middle. */
    const paint = (jump = false) => {
      const view = browse ?? current;
      rows.forEach((el, i) => {
        el.classList.toggle("on", i === current);
        el.classList.toggle("near", i !== current && Math.abs(i - view) <= 1);
      });
      const mid = rows[view];
      if (jump) col.classList.add("still");
      track.style.transform = `translateY(${mid ? -Math.round(mid.offsetTop + mid.offsetHeight / 2) : 0}px)`;
      if (!jump) return;
      // The jump is taken before the transitions come back.
      void track.offsetHeight;
      col.classList.remove("still");
    };

    /** Nobody is looking: no timer is left, and the next line seen is drawn as it stands. */
    const rest = () => {
      follow(null);
      window.clearTimeout(snapTimer);
      if (st?.synced) browse = null;
      seen = "";
      return false;
    };

    const follow = follower((index) => {
      // The strip went to something else, or the island away: nothing runs until a lyric is seen again.
      const where = lyricSeen();
      if (!where) return rest();
      // Just come on screen: nobody saw the line before, so nothing fades or glides.
      const came = where !== seen;
      seen = where;
      if (index === current && !still && !came) return;
      current = index;
      setLyric(texts[index] ?? "", !came);
      if (where !== "section") return;
      paint(still || came);
      still = false;
    });

    /** Plans the following again from what is known now. */
    const plan = () => {
      if (st?.status === "found" && st.synced) return lyricSeen() ? follow(times, st) : void rest();
      follow(null);
      setLyric("");
      const where = lyricSeen();
      // A text without times: the column shows it where the wheel left it.
      if (st?.status === "found" && where === "section" && (still || where !== seen)) {
        paint(true);
        still = false;
      }
      seen = where;
    };

    /** The lyric came on screen, moved between the strip and the section, or left. */
    const look = () => {
      if (lyricSeen() !== seen) plan();
    };

    const apply = (s: State) => {
      st = s;
      // The column is there for every song while lyrics are fetched: the player beside it keeps its width.
      mountLyric(s.track && s.fetch ? col : null);
      if (s.rev !== rev) {
        rev = s.rev;
        // Before the first line the song has one without words: "nothing yet" has a place in the column.
        const lines: [number, string][] = s.synced && s.lines.length > 0 && s.lines[0][0] > 0 ? [[0, ""], ...s.lines] : s.lines;
        times = lines.map(([ms]) => ms);
        texts = lines.map(([, text]) => text.trim());
        rows = texts.map((text) => h("div", { text: text || " " }));
        track.replaceChildren(...rows);
        current = -1;
        browse = s.synced || rows.length === 0 ? null : Math.min(1, rows.length - 1);
        still = true;
      }
      if (s.status !== noted) {
        noted = s.status;
        note.replaceChildren(
          ...(s.status === "none"
            ? [cx.t("No lyrics")]
            : s.status === "error"
              ? [h("span", { text: cx.t("The lyrics could not be loaded.") }), retry]
              : []),
        );
      }
      plan();
    };

    // The wheel browses through the song; left alone, the column goes back to the line being sung.
    col.addEventListener(
      "wheel",
      (e) => {
        if (rows.length === 0) return;
        e.preventDefault();
        const now = performance.now();
        if (e.deltaY === 0 || now - lastStep < STEP_MS) return;
        lastStep = now;
        browse = Math.max(0, Math.min(rows.length - 1, (browse ?? current) + Math.sign(e.deltaY)));
        paint();
        window.clearTimeout(snapTimer);
        if (!st?.synced) return;
        snapTimer = window.setTimeout(() => {
          browse = null;
          paint();
        }, SNAP_MS);
      },
      { passive: false },
    );

    watchLyric(look);
    const off = cx.on<State>("state", apply);
    void cx.call<State>("state").then((s) => s && apply(s));
    return () => {
      off();
      watchLyric(null);
      rest();
      st = null;
      mountLyric(null);
      setLyric("");
    };
  },
});

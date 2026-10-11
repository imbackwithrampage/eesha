// The wake phrase, page side (docs/VOICE.md §6, §7.5, §8): the microphone indicator. The bar of light
// (`cx.island.ear`) is on exactly while the microphone is open and bright for a moment when a voice is
// heard; matching the phrase and the guards are Rust's (crates/voice/src/wake.rs). Package voice-wake.
//
// "Exactly" is kept on both edges. Lit first: a turn's state and `voice:wake {wanted}` both arrive
// before Rust opens the device. Out last: when neither holds any more the bar stays until `ears.status`
// says the device is closed (Rust closes it at the capture's next poll, or after an engine run).
//
// This page is also how Rust knows the indicator can be shown at all (`wake.sync {page}`: without it
// the wake's microphone is never opened), and what tells it that the tray's Pause or its own speech may
// have changed what is wanted.

import { onEvent } from "../../core/bridge";
import type { FeatureCtx } from "../../core/feature";
import type { VoiceEvents } from "./index";

/** How often the closing microphone is looked at until it is closed. */
export const CLOSE_CHECK_MS = 100;

/**
 * What the ear bar shows for a state event: `on` while the microphone is open, `heard` when a voice has
 * just been heard. `wanted`: Rust keeps the microphone open for the wake phrase (`voice:wake`), which
 * it does not on battery under "only while plugged in", nor under any other guard. A turn opens it
 * whatever the wake's switch says, and so does its own speech (for the one word "stop"); with the wake
 * wanted it stays open through thinking and closing too.
 */
export function earFor(event: VoiceEvents["state"], wanted = false): { on: boolean; heard: boolean } {
  const s = event.state;
  const on = s !== "off" && s !== "paused" && (wanted || s === "listening" || s === "asking" || s === "speaking" || s === "waiting" || s === "hearing");
  return { on, heard: on && s === "hearing" };
}

/** Keeps the ear bar true to the microphone, from the feature's `start` to the dispose it returns (which puts it out). */
export function startWake(cx: FeatureCtx): () => void {
  let state: VoiceEvents["state"] = { state: "idle", gen: 0 };
  let wanted = false;
  /** The turn in hand began with the wake phrase. */
  let woke = false;
  let speaking = false;
  /** What the rule last asked of the bar, and the round of checks that may still put it out. */
  let asked: "off" | "on" | "heard" = "off";
  let round = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let over = false;

  const sync = (args?: object) => void cx.call("wake.sync", args);

  /** The bar goes out only once the microphone is closed. */
  const closed = async (mine: number) => {
    const status = await cx.call<{ mic: boolean }>("ears.status");
    if (mine !== round || over) return;
    if (status?.mic) timer = setTimeout(() => void closed(mine), CLOSE_CHECK_MS);
    else cx.island.ear(false);
  };

  const show = () => {
    const ear = earFor(state, wanted);
    const look = !ear.on ? "off" : ear.heard ? "heard" : "on";
    if (look === asked) return;
    asked = look;
    clearTimeout(timer);
    round++;
    if (look === "off") void closed(round);
    else cx.island.ear(true, ear.heard);
  };

  // debt: the tray's Pause has no seam for a feature (asked for in the package's report). The two
  // events that can change it are watched, and Rust reads the flag once the island has set it.
  const later = () => void setTimeout(sync, 0);
  const host = [onEvent("tray", later), onEvent("shortcut", later)];

  const stops = [
    cx.on<VoiceEvents["state"]>("state", (e) => {
      state = e;
      // Its own speech changes what the wake wants (the phrase interrupts only if voice may): Rust decides.
      if ((e.state === "speaking") !== speaking) {
        speaking = !speaking;
        sync();
      }
      // Ended from outside (Esc): no word on whether a command came.
      if (e.state === "closing") woke = false;
      show();
    }),
    cx.on<{ wanted: boolean; woke: boolean }>("wake", (e) => {
      wanted = e.wanted;
      woke ||= e.woke;
      show();
    }),
    // A wake that no command followed counts towards the stricter gate (§6, "False wakes").
    cx.on<VoiceEvents["heard"]>("heard", (e) => {
      if (!e.final) return;
      if (woke && !e.text.trim()) void cx.call("wake.empty");
      woke = false;
    }),
    // The speech model arrived: the wake can be listened for.
    cx.on<VoiceEvents["download"]>("download", (e) => {
      if (e.done) sync();
    }),
  ];
  sync({ page: true });

  return () => {
    over = true;
    clearTimeout(timer);
    for (const stop of stops) stop();
    for (const off of host) void off.then((stop) => stop());
    cx.island.ear(false);
  };
}

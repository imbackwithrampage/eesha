// What the compact strip shows, and when the island comes out to show it
// (docs/ARCHITECTURE.md §5). Pure TypeScript: no DOM, no Tauri.
//
// Three candidates, in this order: a notice (transient), a live activity, the
// pills. A card that waits for an answer beats all three: nothing is shown over
// it and notices wait behind it.
//
// The notice table, `shouldReplace` and the activity order and pairs are ported
// from reference/vorssaint-utils/Sources/Vorssaint/Services/Notch/NotchSupport.swift
// (`NotchEvent`, `shouldReplace`, `compactActivities`, `compactCompanions`).

import type { ActivityDef, Notice, NoticeKind } from "../core/feature";

/** Priority (higher replaces lower, equal replaces) and time on screen, by kind. */
export const NOTICES: Record<NoticeKind, { priority: number; ms: number }> = {
  volume: { priority: 3, ms: 1600 },
  brightness: { priority: 3, ms: 1600 },
  mic: { priority: 3, ms: 1600 },
  capture: { priority: 2, ms: 12_000 },
  timer: { priority: 2, ms: 6000 },
  watch: { priority: 2, ms: 6000 },
  battery: { priority: 1, ms: 4000 },
  notification: { priority: 1, ms: 3000 },
  accessory: { priority: 1, ms: 4000 },
  agent: { priority: 1, ms: 5000 },
  clipboard: { priority: 0, ms: 2500 },
  download: { priority: 0, ms: 6000 },
  track: { priority: 0, ms: 3000 },
};

/**
 * A notice the pointer holds is being read: only the same kind of message, or
 * something more urgent, may take its place.
 */
export function shouldReplace(current: NoticeKind | null, incoming: NoticeKind, held = false): boolean {
  if (!current) return true;
  if (held) return incoming === current || NOTICES[incoming].priority > NOTICES[current].priority;
  return NOTICES[incoming].priority >= NOTICES[current].priority;
}

// The fixed order of the live activities (ActivityDef.order).
// `agentWork` is the agent-usage feature's: turns of sessions the hooks do not report, just after the ones they do.
// `micsMuted` (audio-mixer) is a state the user must not forget: above the calendar and music, and it rides with nothing.
// `micInUse` (huds-power) is a privacy indicator: before the agents, whose dot rides beside it.
// `copies` (clipboard) is a half-minute pointer to the section: last but the battery.
// `recording` (recorder) comes before a timer: while a recording runs the strip holds its time, Pause and Stop.
export const ORDER = {
  recording: -0.5, timer: 0, watch: 1, micInUse: 2.5, agents: 3, agentWork: 3.5, micsMuted: 3.7, calendar: 4, music: 5, keepAwake: 6, copies: 6.5,
} as const;

/**
 * Which activities may ride beside the one that owns the strip, by `order`.
 * Keep awake can run all day, so it is the last mark beside a timer or an event.
 */
const PAIRS: Record<number, readonly number[]> = {
  [ORDER.micInUse]: [ORDER.agents, ORDER.agentWork],
  [ORDER.timer]: [ORDER.agents, ORDER.agentWork, ORDER.calendar, ORDER.music, ORDER.keepAwake],
  [ORDER.calendar]: [ORDER.agents, ORDER.agentWork, ORDER.music, ORDER.keepAwake],
};

/** The first active activity owns the strip; its supported companions follow. */
export function pickActivities<A extends Pick<ActivityDef, "order" | "isActive">>(
  all: readonly A[],
): { primary: A | null; companions: A[] } {
  const active = all.filter((a) => a.isActive()).sort((a, b) => a.order - b.order);
  const primary = active[0] ?? null;
  const allowed = primary ? (PAIRS[primary.order] ?? []) : [];
  return { primary, companions: active.slice(1).filter((a) => allowed.includes(a.order)) };
}

/** One volume key press on Windows: a smaller change within the quiet window is not reported. */
const KEY_STEP = 1 / 64; // the fine volume steps of audio-extras are a 64th
const QUIET_MS = 2000;
/** A notice that waited longer than this behind a card or an open island is old news. */
const QUEUE_MS = 30_000;
/** How long the strip comes out for an activity that started or has news. */
export const ACTIVITY_REVEAL_MS = 3000;

/** A notice with what the island knows about it. */
export interface Shown<N extends Notice = Notice> {
  notice: N;
  /** Which feature sent it: its `section` is one of that feature's. */
  feature: string;
}

export interface ArbiterHost {
  /** A card waits for an answer: nothing covers it, notices queue. */
  blocked(): boolean;
  /** The island is open: a level goes to the header, other notices queue. */
  open(): boolean;
  /** The pointer rests on the strip. */
  hovered(): boolean;
  /** Slide the strip out for this long; `exact` = for a notice, whose time replaces what was left. */
  reveal(ms: number, exact?: boolean): void;
  /** The strip or the header level changed: draw again. */
  changed(): void;
}

export class Arbiter {
  /** The notice on the strip. */
  notice: Shown | null = null;
  /** The level in the open island's header. */
  level: Shown | null = null;
  /** True while a full-screen app is in front (§4): the strip stays away, notices queue. */
  quiet = false;

  private queued: (Shown & { at: number }) | null = null;
  private noticeTimer: number | null = null;
  private levelTimer: number | null = null;
  private lastLevel = new Map<NoticeKind, { level: number; title: string; at: number }>();

  // Written out, not a parameter property: Node runs this file as it is in the tests.
  private host: ArbiterHost;

  constructor(host: ArbiterHost) {
    this.host = host;
  }

  /** A feature has something to say. */
  post(notice: Notice, feature: string) {
    if (notice.level != null && this.silent(notice)) return;
    const shown: Shown = { notice, feature };
    if (this.host.open() && notice.level != null) {
      this.level = shown;
      window.clearTimeout(this.levelTimer ?? undefined);
      this.levelTimer = window.setTimeout(() => {
        this.levelTimer = null;
        this.level = null;
        this.host.changed();
      }, NOTICES[notice.kind].ms);
      this.host.changed();
      return;
    }
    if (this.host.blocked() || this.host.open() || this.quiet) {
      // A level is feedback for a key just pressed: late, it means nothing.
      if (notice.level == null && shouldReplace(this.queued?.notice.kind ?? null, notice.kind)) {
        this.queued = { ...shown, at: performance.now() };
      }
      return;
    }
    const up = this.notice;
    if (!shouldReplace(up?.notice.kind ?? null, notice.kind, this.host.hovered())) {
      // A banner that loses to a level (a device connected while the volume shows) waits its turn instead of being lost.
      if (up?.notice.level != null && notice.level == null) this.queued = { ...shown, at: performance.now() };
      return;
    }
    // And a level that takes the strip from a banner gives it back afterwards.
    if (up && up.notice.level == null && notice.level != null) this.queued = { ...up, at: performance.now() };
    this.show(shown);
  }

  /** The card was answered, the island folded, the full-screen app left: what waited may show. */
  flush() {
    const q = this.queued;
    if (!q || this.host.blocked() || this.host.open() || this.quiet) return;
    this.queued = null;
    if (performance.now() - q.at < QUEUE_MS) this.show(q);
  }

  /** Takes the notice down (it was clicked, or its feature went away). */
  dismiss() {
    window.clearTimeout(this.noticeTimer ?? undefined);
    this.noticeTimer = null;
    if (!this.notice) return;
    this.notice = null;
    this.host.changed();
  }

  /** An activity started, ended or has news: the strip comes out to show it. */
  activityChanged(activity: Pick<ActivityDef, "isActive"> | undefined) {
    if (activity?.isActive() && !this.host.blocked() && !this.quiet) this.host.reveal(ACTIVITY_REVEAL_MS);
    this.host.changed();
  }

  private show(shown: Shown) {
    const ms = NOTICES[shown.notice.kind].ms;
    this.notice = shown;
    window.clearTimeout(this.noticeTimer ?? undefined);
    this.noticeTimer = window.setTimeout(() => this.expire(), ms);
    this.host.reveal(ms, true);
    this.host.changed();
  }

  private expire() {
    // Being read: look again shortly.
    if (this.host.hovered()) {
      this.noticeTimer = window.setTimeout(() => this.expire(), 600);
      return;
    }
    this.noticeTimer = null;
    this.notice = null;
    this.host.changed();
    this.flush();
  }

  /** A level that barely moved since the last report (adaptive volume riding). Mute always reports. */
  private silent(n: Notice): boolean {
    const now = performance.now();
    const last = this.lastLevel.get(n.kind);
    const level = n.level ?? 0;
    const quiet =
      last != null &&
      now - last.at < QUIET_MS &&
      last.title === n.title &&
      level > 0 &&
      last.level > 0 &&
      Math.abs(level - last.level) < KEY_STEP - 1e-6;
    if (!quiet) this.lastLevel.set(n.kind, { level, title: n.title, at: now });
    return quiet;
  }
}

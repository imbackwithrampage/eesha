// Which line is being sung, and when the next one begins. No fast timer: one timeout per line,
// planned from the lines' own times and planned again whenever the player says something new.
// No DOM in here: `node src/features/lyrics/clock-probe.mjs` runs it on a made-up clock.

/** Where the player is: `position` seconds at `at` (Unix ms), moving on at `rate` while it plays. */
export interface Pos {
  playing: boolean;
  position: number;
  at: number;
  rate: number;
}

/** The position at `now`, in milliseconds. */
export function msAt(p: Pos, now: number): number {
  return (p.playing ? p.position + ((now - p.at) / 1000) * p.rate : p.position) * 1000;
}

/** The line being sung at `ms`: the last one that has begun; -1 before the first. `times` ascend. */
export function indexAt(times: readonly number[], ms: number): number {
  let lo = 0;
  let hi = times.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (times[mid] <= ms) lo = mid + 1;
    else hi = mid;
  }
  return lo - 1;
}

/** The clock and the timer: the page's own unless the probe brings its made-up ones. */
export interface Io {
  now(): number;
  later(fn: () => void, ms: number): number;
  cancel(id: number): void;
}

/**
 * Follows a synced text. The function it returns takes the lines' times and the
 * player's last report: `show(index)` is called at once and again as each line
 * begins. Call it again to plan anew (a position report, a seek, a pause, a
 * change of rate, another text) and with `null` to stop. Nothing is left
 * running once it is stopped, the song is paused, the last line has begun, or
 * `show` answers `false` (nobody is looking).
 */
export function follower(
  show: (index: number) => boolean | void,
  io: Io = { now: Date.now, later: (fn, ms) => window.setTimeout(fn, ms), cancel: (id) => window.clearTimeout(id) },
): (times: readonly number[] | null, pos?: Pos) => void {
  let timer = 0;
  const plan = (times: readonly number[], pos: Pos) => {
    timer = 0;
    const ms = msAt(pos, io.now());
    const index = indexAt(times, ms);
    const next = times[index + 1];
    if (show(index) === false || !pos.playing || next == null || !(pos.rate > 0)) return;
    // One millisecond past the line's time: a timer that fires a hair early must not find the old line again.
    timer = io.later(() => plan(times, pos), (next - ms) / pos.rate + 1);
  };
  return (times, pos) => {
    if (timer) io.cancel(timer);
    timer = 0;
    if (times && pos) plan(times, pos);
  };
}

// The line scheduling on a made-up clock: `node src/features/lyrics/clock-probe.mjs`.
// A made-up synced text, the reports a player would make (a correction, a seek, a pause, a
// change of rate, the island going away) and, at each moment, the line that shows.

import { follower, indexAt, msAt } from "./clock.ts";

const lines = [
  [0, ""],
  [2000, "Line one"],
  [5500, "Line two"],
  [9000, "Line three"],
  [12500, ""],
  [20000, "Line four"],
  [24000, "Line five"],
  [30000, "Line six"],
];
const times = lines.map(([ms]) => ms);

// The clock and the one timer there can be.
const start = 1_700_000_000_000;
let now = start;
let due = null;
let ids = 0;
let wakes = 0;
const io = {
  now: () => now,
  later: (fn, ms) => ((due = { at: now + ms, fn, id: ++ids }), due.id),
  cancel: (id) => {
    if (due?.id === id) due = null;
  },
};

let pos = { playing: false, position: 0, at: now, rate: 1 };
let shown = -1;
let worst = 0;
const secs = (ms) => (ms / 1000).toFixed(3).padStart(7);
const follow = follower((index) => {
  if (index === shown) return;
  shown = index;
  console.log(`  +${secs(now - start)} s  position ${secs(msAt(pos, now))} s  line ${index} ${JSON.stringify(lines[index][1])}`);
}, io);

/** Lets the clock run to `to` (ms from the start), firing the timer as it comes due, and checks every 50 ms. */
function runTo(to, watching = true) {
  const end = start + to;
  for (let tick = now; tick <= end; tick += 50) {
    while (due && due.at <= tick) {
      const d = due;
      due = null;
      now = d.at;
      wakes++;
      d.fn();
    }
    now = tick;
    if (!watching) continue;
    // How long the line on screen has been the wrong one, if it is.
    const right = indexAt(times, msAt(pos, now));
    if (right !== shown) worst = Math.max(worst, msAt(pos, now) - times[right]);
  }
  now = end;
}

function report(at, what, next) {
  runTo(at);
  pos = { ...pos, ...next, at: now };
  console.log(`+${secs(at)} s  ${what}`);
  follow(times, pos);
}

report(0, "the song starts", { playing: true, position: 0 });
report(7300, "a position report: the player is at 7.9 s, 0.6 s ahead of the clock here", { position: 7.9 });
report(10000, "a seek to 23.0 s", { position: 23 });
report(12000, "a pause at 25.0 s", { playing: false, position: 25 });
report(20000, "playing again", { playing: true, position: 25 });
report(22000, "twice the speed, at 27.0 s", { position: 27, rate: 2 });
report(24000, "a seek back to 1.0 s, at the usual speed", { position: 1, rate: 1 });

runTo(26000);
console.log(`+${secs(26000)} s  the lyric leaves the screen`);
follow(null);
console.log(`  timers left: ${due ? 1 : 0}`);
const asleep = wakes;
runTo(40000, false);
console.log(`  wakes while it was away: ${wakes - asleep}`);
console.log(`+${secs(40000)} s  it is back on screen`);
shown = -1;
follow(times, pos);
runTo(45000);
follow(null);

console.log(`\ntimer wakes in 31 s on screen: ${wakes} (a 250 ms poll: ${31 * 4})`);
console.log(`longest a wrong line stayed up, checked every 50 ms: ${worst.toFixed(0)} ms`);

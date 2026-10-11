// The ring's geometry as plain functions: no DOM in here, so the tests run them
// (tests/radial-ring.test.mjs). wheel.ts draws with them.
//
// The constants repeat crates/radial-menu/src/geometry.rs; change both together.
// Slice 0 is centred at 12 o'clock and the slices run clockwise.

export const SIZE = 400;
export const MID = SIZE / 2;
export const RADIUS = 150;
export const HUB = 40;
/** Where the slices start and end, before the gap and the rounding are taken off. */
export const INNER = 46;
/** The gap between two slices, and the half-width of the rounding stroke that also insets them. */
const GAP = 4;
const ROUND = 3;

/** The point at `angle` (radians, clockwise from 12 o'clock) and `r` from the centre. */
export const at = (angle: number, r: number): [number, number] => [MID + r * Math.sin(angle), MID - r * Math.cos(angle)];

/** Where slice `i` of `n` is centred, in degrees clockwise from 12 o'clock. */
export const sliceAngle = (i: number, n: number): number => (n > 0 ? (i * 360) / n : 0);

/**
 * How many slices lie between `from` and `to` the shorter way round: positive is clockwise,
 * negative the other way. Straight across (half the ring) goes clockwise.
 */
export function shorter(from: number, to: number, n: number): number {
  if (n <= 0) return 0;
  const ahead = (((to - from) % n) + n) % n;
  return ahead > n / 2 ? ahead - n : ahead;
}

/**
 * The highlight's angle once it has gone from slice `from` to slice `to`, in degrees. It is never
 * wrapped into 0…360: from the last slice on to the first it keeps growing, so a transition
 * between two such angles turns the way that was chosen here and not the long way back.
 */
export const travel = (angle: number, from: number, to: number, n: number): number => angle + shorter(from, to, n) * sliceAngle(1, n);

/** One slice: an annular sector, inset so that two neighbours leave a gap, rounded by a stroke of the same colour. */
export function wedge(i: number, n: number): string {
  const centre = (i * 2 * Math.PI) / n;
  const half = Math.PI / n;
  const inset = GAP / 2 + ROUND;
  const ri = INNER + ROUND;
  const ro = RADIUS - ROUND;
  const di = inset / ri;
  const dout = inset / ro;
  const start = Math.min(half - 0.01, di);
  const startOut = Math.min(half - 0.01, dout);
  const spanOut = 2 * (half - startOut);
  const spanIn = 2 * (half - start);
  const f = ([x, y]: [number, number]) => `${x.toFixed(2)} ${y.toFixed(2)}`;
  return (
    `M${f(at(centre - half + start, ri))}` +
    `L${f(at(centre - half + startOut, ro))}` +
    `A${ro} ${ro} 0 ${spanOut > Math.PI ? 1 : 0} 1 ${f(at(centre + half - startOut, ro))}` +
    `L${f(at(centre + half - start, ri))}` +
    `A${ri} ${ri} 0 ${spanIn > Math.PI ? 1 : 0} 0 ${f(at(centre - half + start, ri))}Z`
  );
}

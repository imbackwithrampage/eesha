// How the System section writes its numbers.
//
// `graphCeiling` is ported from the upstream file
// Sources/Vorssaint/Services/Metrics/MetricFormat.swift.

import { N_, t } from "../../i18n/i18n";

const UNITS = [N_("{n} B"), N_("{n} KB"), N_("{n} MB"), N_("{n} GB"), N_("{n} TB")];

/** "512 B", "1.4 MB", "118 GB": binary steps, as Windows counts, and never four digits. */
export function bytes(value: number): string {
  let n = Math.max(0, value);
  let unit = 0;
  while (n >= 1000 && unit < UNITS.length - 1) {
    n /= 1024;
    unit++;
  }
  return t(UNITS[unit], { n: unit === 0 || n >= 100 ? Math.round(n) : n.toFixed(1) });
}

export const rate = (bytesPerSecond: number) => t("{size}/s", { size: bytes(bytesPerSecond) });

export const percent = (fraction: number) => t("{percent}%", { percent: Math.round(Math.max(0, Math.min(1, fraction)) * 100) });

/** "3 h 12 min", "48 min". */
export function span(seconds: number): string {
  const minutes = Math.max(1, Math.round(seconds / 60));
  return minutes < 60 ? t("{m} min", { m: minutes }) : t("{h} h {m} min", { h: Math.floor(minutes / 60), m: minutes % 60 });
}

/**
 * The top of a graph that scales itself: the peak rounded up to 1, 2 or 5 of
 * a unit, so its label reads "2.0 MB/s" and it only moves when the traffic
 * crosses a step.
 */
export function graphCeiling(peak: number, unitStep = 1024): number {
  for (let power = 0; power < 6; power++) {
    for (const step of [1, 2, 5, 10, 20, 50, 100, 200, 500]) {
      const ceiling = step * unitStep ** power;
      if (ceiling >= peak) return ceiling;
    }
  }
  return peak;
}

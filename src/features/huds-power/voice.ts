// What can be said to the battery HUD (docs/VOICE.md §5): the charge, read and nothing else.

import { defineVoice } from "../../core/feature";

interface Battery { ac: boolean; percent: number | null; charging: boolean; minutes: number | null }

const span = (m: number) => (m >= 60 ? `${Math.floor(m / 60)} hour${m >= 120 ? "s" : ""}${m % 60 ? ` ${m % 60} minutes` : ""}` : `${m} minutes`);

export default defineVoice([
  {
    id: "battery",
    tool: "battery_level",
    describe: "Say the battery charge and whether it is charging.",
    forms: ["what is [my] [the] battery [level|at]", "how much battery [do i have|is left|have i got]", "battery level", "is [my] [the] battery charging"],
    safety: "run",
    run: async (_a, v) => {
      const b = await v.call<Battery>("battery");
      if (!b || b.percent == null || b.percent > 100) return { say: "This PC has no battery to read.", ok: false };
      const left = b.minutes ? (b.charging ? `, ${span(b.minutes)} to full` : `, about ${span(b.minutes)} left`) : "";
      return { say: `Battery is at ${b.percent} percent${b.charging ? ", charging" : b.ac ? ", plugged in" : ""}${left}.` };
    },
  },
]);

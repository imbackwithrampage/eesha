// What can be said to agent usage (docs/VOICE.md §5): the tokens used today and this week, read from the
// totals the feature keeps. The feature has no price per token, so "value" is not offered.

import { defineVoice } from "../../core/feature";

interface Period { total: number; requests: number }
interface Usage { providers: { name: string; today: Period; week: Period }[] }

const amount = (n: number) => (n < 1e3 ? `${n}` : n < 1e6 ? `${Math.round(n / 1e3)} thousand` : `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)} million`);

export default defineVoice([
  {
    id: "usage",
    tool: "agent_usage",
    describe: "Say how many tokens the coding agents used today and this week.",
    forms: ["how many tokens (have i|did i) (use|used) [today]", "how many tokens (today|this week)", "how much have my agents used [today]", "what is my agent usage"],
    safety: "run",
    run: async (_a, v) => {
      const u = await v.call<Usage>("state");
      if (!u?.providers.length) return { say: "I have no agent usage to read yet.", ok: false };
      const sum = (k: "today" | "week") => u.providers.reduce((n, p) => n + p[k].total, 0);
      return { say: `${amount(sum("today"))} tokens today, ${amount(sum("week"))} over the last seven days.` };
    },
  },
]);

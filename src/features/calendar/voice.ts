// What can be said to the calendar (docs/VOICE.md §5): add and tick a reminder, what is next, today.
// Deleting a reminder is only prepared (the runner refuses `delete_reminder`): its card opens.

import { defineVoice, type VoiceArg, type VoiceCx } from "../../core/feature";

interface Rem { id: number; text: string; done: boolean }
interface Ev { title: string; start: number; end: number; allDay: boolean }

const HOUR = 3600e3;
const DAY = 24 * HOUR;
const at = (ms: number) => new Date(ms).toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
const midnight = (ms: number) => new Date(new Date(ms).setHours(0, 0, 0, 0)).getTime();

const reminders = async (v: VoiceCx): Promise<Rem[]> => (await v.call<{ reminders: Rem[] }>("state"))?.reminders ?? [];
const events = async (v: VoiceCx, from: number, to: number): Promise<Ev[] | null> => {
  const r = await v.call<{ events: Ev[] }>("events", { from, to });
  return r ? r.events.sort((a, b) => a.start - b.start) : null;
};

const reminder: VoiceArg = {
  name: "reminder",
  kind: "choice",
  choices: async (v) => (await reminders(v)).filter((r) => !r.done).map((r) => ({ id: String(r.id), say: [r.text] })),
  ask: "Which reminder?",
};

export default defineVoice([
  {
    id: "add_reminder",
    tool: "reminder_add",
    describe: "Add a reminder to the list of reminders.",
    forms: ["remind me to {text}", "add a reminder to {text}", "add [a] reminder {text}"],
    args: [{ name: "text", kind: "text", ask: "What should I remind you of?" }],
    safety: "run",
    run: async (a, v) => ((await v.call("add_reminder", { text: String(a.text) })) ? { say: "Added to your reminders." } : { say: "I couldn't add it.", ok: false }),
  },
  {
    id: "tick_reminder",
    tool: "reminder_done",
    describe: "Tick off a reminder as done.",
    forms: ["tick off {reminder}", "check off {reminder}", "mark {reminder} as done", "mark {reminder} done", "complete {reminder}"],
    args: [reminder],
    safety: "run",
    run: async (a, v) => {
      const r = (await reminders(v)).find((x) => String(x.id) === String(a.reminder));
      if (!r || r.done) return { say: "That reminder is not open.", ok: false };
      return (await v.call("toggle_reminder", { id: r.id })) ? { say: "Ticked off." } : { say: "I couldn't tick it.", ok: false };
    },
    undo: async (a, v) => {
      const r = (await reminders(v)).find((x) => String(x.id) === String(a.reminder));
      if (r?.done) await v.call("toggle_reminder", { id: r.id });
    },
  },
  {
    id: "next",
    tool: "calendar_next",
    describe: "Say the next event on the calendar.",
    forms: ["what is next on my calendar", "what is my next (event|meeting|appointment)", "when is my next (event|meeting|appointment)", "what is next"],
    safety: "run",
    run: async (_a, v) => {
      const now = Date.now();
      const list = await events(v, now, now + 14 * DAY);
      if (!list) return { say: "I couldn't read the calendar.", ok: false };
      const next = list.find((e) => e.end > now);
      if (!next) return { say: "Nothing is coming up in the next two weeks." };
      const day = midnight(next.start) <= midnight(now) ? "today" : midnight(next.start) === midnight(now) + DAY ? "tomorrow" : new Date(next.start).toLocaleDateString("en-US", { weekday: "long" });
      return { say: next.allDay ? `${next.title}, ${day}, all day.` : `${next.title}, ${day} at ${at(next.start)}.` };
    },
  },
  {
    id: "today",
    tool: "calendar_today",
    describe: "Say today's events.",
    forms: ["what is on my calendar [today]", "what do i have today", "what is my day like", "read my calendar"],
    safety: "run",
    run: async (_a, v) => {
      const from = midnight(Date.now());
      const list = await events(v, from, from + DAY);
      if (!list) return { say: "I couldn't read the calendar.", ok: false };
      const left = list.filter((e) => e.end > Date.now());
      if (!left.length) return { say: "Nothing else today." };
      const said = left.slice(0, 4).map((e) => (e.allDay ? `${e.title}, all day` : `${e.title} at ${at(e.start)}`));
      return { say: `${said.join(". ")}.${left.length > 4 ? ` And ${left.length - 4} more.` : ""}` };
    },
  },
  {
    id: "delete_reminder",
    tool: "reminder_delete",
    describe: "Open the reminders so one can be deleted by a click.",
    forms: ["delete [a|the] reminder", "remove [a|the] reminder"],
    safety: "prepare",
    // Deleting is a click on the card; the runner refuses the method.
    run: async (_a, v) => {
      v.open("reminders");
      return { say: "Here are your reminders. Delete it with the button." };
    },
  },
]);

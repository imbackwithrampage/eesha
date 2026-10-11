// The sentence splitter (docs/VOICE.md §7.3, step 1): fixed replies and token deltas in, chunks for
// `mouth.say` out. Text is cleaned for speech (markdown, links, "[1]", bullets, code, emoji, a few
// symbols and units read as words) and cut as early as is safe: at . ! ? … or a newline, not after an
// abbreviation, inside a number, a time, a URL or an open code span or link; the first chunk is also
// cut at , ; : once it has six words; under 12 characters joins the next; over 220 is cut at a space.
// After the first, a chunk is at most about 1.2 times as long as the one before it (§7.2: a longer one
// leaves a silence): a sentence that runs on is cut at the last , ; : before that, else at a space.
// Every decision looks only at text already received (and one character after a stop), so the chunks
// are the same however the text was cut into pushes. Pure: no DOM, no Tauri.
// Filled by the package voice-generator (VOICE §10; read §7.3).

const MAX = 220;
const MIN = 12;
const FIRST_WORDS = 6;
// ponytail: characters stand in for audio seconds, and only the chunk before counts (a short one caps the next even when the mouth is ahead); time the voice, or carry the unused allowance, if answers sound chopped.
const GROW = 1.2;

const ABBR = new Set(["mr", "mrs", "ms", "dr", "prof", "sr", "jr", "st", "vs", "no", "fig", "approx", "inc", "ltd", "co", "vol", "mt", "gen", "sen", "rep", "gov", "lt", "col", "sgt", "capt", "jan", "feb", "mar", "apr", "jun", "jul", "aug", "sep", "sept", "oct", "nov", "dec"]);

/** A line's leading mark: heading, quote, bullet, number, checkbox. */
const MARK = /^[ \t]*(?:#{1,6}[ \t]+|>[ \t]*|[-*+•▪◦–—][ \t]+|\d{1,3}[.)][ \t]+)(?:\[[ xX]\][ \t]+)?/;
/** The start of a mark or a fence that cannot be told apart yet. */
const PART = /^(?:#{1,6}|\d{1,3}[.)]?|[-*+•▪◦–—]|`{1,2})?$/;

const CURRENCY: Record<string, string> = { $: "dollars", "€": "euros", "£": "pounds" };
const UNITS: Record<string, [string, string]> = {
  "km/h": ["kilometer per hour", "kilometers per hour"], mph: ["mile per hour", "miles per hour"],
  km: ["kilometer", "kilometers"], kg: ["kilogram", "kilograms"], cm: ["centimeter", "centimeters"], mm: ["millimeter", "millimeters"],
  kb: ["kilobyte", "kilobytes"], mb: ["megabyte", "megabytes"], gb: ["gigabyte", "gigabytes"], tb: ["terabyte", "terabytes"], ms: ["millisecond", "milliseconds"],
};

/** One line of written text as speech (its leading mark already taken off). */
function inline(s: string): string {
  return s
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[\d{1,3}\]/g, "")
    .replace(/<\/?[a-z][^>]*>/gi, "")
    .replace(/https?:\/\/[^\s<>()]*[^\s<>().,;:!?'"]|www\.[^\s<>()]*[^\s<>().,;:!?'"]/gi, "a link")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/(\*\*|__|~~)(.+?)\1/g, "$2")
    .replace(/(^|[\s(])([*_])(\S(?:[^*_]*?\S)?)\2(?=[\s).,!?:;]|$)/g, "$1$3")
    .replace(/(^|\s)[*_]+(?=\S)/g, "$1")
    .replace(/(?<=\S)[*_]+(?=\s|$)/g, "")
    .replace(/`/g, "")
    .replace(/([$€£])(\d[\d,]*(?:\.\d+)?)(?:\s?(million|billion|trillion|thousand))?/gi, (_m, c: string, n: string, scale?: string) => `${n}${scale ? ` ${scale}` : ""} ${CURRENCY[c]}`)
    .replace(/(\d)\s?%/g, "$1 percent")
    .replace(/%/g, " percent ")
    .replace(/(\d)\s?°\s?([CF])\b/g, (_m, n: string, u: string) => `${n} degrees ${u === "C" ? "Celsius" : "Fahrenheit"}`)
    .replace(/°/g, " degrees ")
    .replace(/(\d)\s?(km\/h|mph|km|kg|cm|mm|kb|mb|gb|tb|ms)\b/gi, (_m, n: string, u: string) => `${n} ${UNITS[u.toLowerCase()][/^1(\.0+)?$/.test(n) ? 0 : 1]}`)
    .replace(/~\s?(?=\d)/g, "about ")
    .replace(/\s&\s/g, " and ")
    .replace(/\s×\s/g, " times ")
    .replace(/\s?(?:->|→)\s?/g, ", ")
    .replace(/\p{Extended_Pictographic}|\p{Regional_Indicator}|\p{Emoji_Modifier}|[️‍]/gu, "")
    .replace(/^[\s|]+|[\s|]+$/g, "")
    .replace(/\s*\|\s*/g, ", ")
    .replace(/\s+/g, " ")
    .replace(/ +([.,;:!?])/g, "$1")
    .trim();
}

/** A line with nothing to say: a rule, a table's divider. */
const RULE = /^[\s|:\-_=*]{3,}$/;

/** What is said for a text: no markdown, links, footnote marks, bullets or code (fenced code is dropped). */
export function cleanForSpeech(text: string): string {
  const out: string[] = [];
  let fence = false;
  for (const line of text.replace(/\r/g, "").split("\n")) {
    if (line.trimStart().startsWith("```")) {
      fence = !fence;
      continue;
    }
    if (fence || RULE.test(line)) continue;
    const said = inline(line.replace(MARK, ""));
    if (said) out.push(said);
  }
  return out.join(" ");
}

/** A code span or a link is still open in this text: a cut here would split it. */
function open(s: string): boolean {
  const ticks = s.split("`").length - 1;
  const square = s.split("[").length - s.split("]").length;
  return ticks % 2 === 1 || square > 0 || /\]\([^)]*$/.test(s);
}

/** The word before a full stop is an abbreviation, an initial or has dots inside ("U.S", "p.m", "e.g"). */
function abbreviation(before: string): boolean {
  const word = /[\p{L}\p{N}.]*$/u.exec(before)![0].replace(/^\.+/, "");
  return word.includes(".") || (/^\p{L}$/u.test(word) && word !== "I") || ABBR.has(word.toLowerCase());
}

interface Cut {
  end: number;
  next: number;
  newline: boolean;
}

/** One per answer. */
export class Splitter {
  private rest = "";
  /** At the start of a line: a mark or a fence may follow. */
  private bol = true;
  private fence = false;
  /** Dropping the rest of this line (a fence's own line, or code inside one). */
  private skip = false;
  /** Nothing released yet: the first chunk may be cut at a comma. */
  private first = true;
  /** How long the chunk before was: the next may be `GROW` times that. */
  private prev = 0;

  /** More text arrived; answers the chunks that are complete now. */
  push(delta: string): string[] {
    this.rest += delta.replace(/\r/g, "");
    return this.drain(false);
  }

  /** The text is over; answers what is left. */
  end(): string[] {
    const out = this.drain(true);
    const last = inline(this.rest);
    this.rest = "";
    if (last && !this.fence) out.push(last);
    return out;
  }

  private drain(final: boolean): string[] {
    const out: string[] = [];
    for (;;) {
      const r = this.rest;
      if (!r) break;
      if (this.skip) {
        const nl = r.indexOf("\n");
        this.rest = nl < 0 ? "" : r.slice(nl + 1);
        if (nl < 0) break;
        this.skip = false;
        this.bol = true;
        continue;
      }
      if (this.bol) {
        const t = r.replace(/^[ \t]+/, "");
        if (t.startsWith("\n")) {
          this.rest = t.slice(1);
          continue;
        }
        if (!final && PART.test(t)) break;
        if (t.startsWith("```")) this.fence = !this.fence;
        if (t.startsWith("```") || this.fence) {
          this.skip = true;
          continue;
        }
        const nl = r.indexOf("\n");
        const line = nl < 0 ? r : r.slice(0, nl);
        if (RULE.test(line) || (nl < 0 && !final && /^[\s|:\-_=*]+$/.test(line))) {
          if (nl < 0 && !final) break;
          this.skip = true;
          continue;
        }
        const m = MARK.exec(r);
        this.bol = false;
        if (m) {
          this.rest = r.slice(m[0].length);
          continue;
        }
      }
      const cut = this.find(r, final);
      if (!cut) break;
      const said = inline(r.slice(0, cut.end));
      this.rest = r.slice(cut.next);
      this.bol = cut.newline;
      if (said) {
        out.push(said);
        this.first = false;
        this.prev = said.length;
      }
    }
    return out;
  }

  /** Where the next chunk of `r` ends, or null when more text is needed. */
  private find(r: string, final: boolean): Cut | null {
    const cap = this.first ? MAX : Math.round(this.prev * GROW);
    const fits = (end: number) => end >= MIN && !open(r.slice(0, end)) && inline(r.slice(0, end)).length >= MIN;
    // Where a sentence longer than `cap` is cut: the last , ; : within it, else the last space after a word
    // and before a word or a number (never one `inline` reads across: "10 000", "5 %", "A & B").
    let soft: Cut | null = null;
    let clause = false;
    for (let i = 0; i < r.length && i <= MAX; i++) {
      const c = r[i];
      // It runs on past `cap`, and by more than MIN, so what is left of it is no stub.
      if (soft && i > cap + MIN) return soft;
      if (!this.first && i <= cap) {
        if (",;:".includes(c) && /\s/.test(r[i + 1] ?? "")) {
          if (fits(i + 1)) {
            soft = { end: i + 1, next: i + 1, newline: false };
            clause = true;
          }
        } else if (c === " " && !clause && /\p{L}/u.test(r[i - 1] ?? "") && /[\p{L}\p{N}]/u.test(r[i + 1] ?? "") && fits(i)) {
          soft = { end: i, next: i + 1, newline: false };
        }
      }
      if (c === "\n") return { end: i, next: i + 1, newline: true };
      if (".!?…".includes(c)) {
        let j = i + 1;
        while (j < r.length && /["'”’)\]»*_]/.test(r[j])) j++;
        if (j >= r.length ? !final : !/\s/.test(r[j])) {
          if (j >= r.length) return null;
          continue;
        }
        if (c === "." && r[i - 1] !== "." && abbreviation(r.slice(0, i))) continue;
        if (open(r.slice(0, j)) || inline(r.slice(0, j)).length < MIN) continue;
        return { end: j, next: j, newline: false };
      }
      if (this.first && ",;:".includes(c) && /\s/.test(r[i + 1] ?? "")) {
        const words = r.slice(0, i).split(/\s+/).filter(Boolean).length;
        if (words >= FIRST_WORDS && !open(r.slice(0, i + 1))) return { end: i + 1, next: i + 1, newline: false };
      }
    }
    if (r.length > MAX) {
      let k = r.lastIndexOf(" ", MAX);
      if (k < 1) k = r.indexOf(" ", MAX);
      if (k > 0) return { end: k, next: k + 1, newline: false };
    }
    return null;
  }
}

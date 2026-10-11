// The question card, as the terminal does it: pick an option, tick several,
// answer in words under "Other…", step back. Ported from upstream
// `windows/src/views/views.ts` (pull request 335), as a state machine without
// DOM so it can be checked.
//
// Keys belong to the question card only. `questionKey` is the one place that
// turns a key into an action, and it answers nothing for any other view: no
// key ever answers a permission card.

import type { AskedQuestion } from "./approvals";

export type Answers = Record<string, string | string[]>;

/** Where the user is in one request's questions. */
export class QuestionFlow {
  index = 0;
  answers: Answers = {};
  /** The questions answered in words rather than with a label. */
  other = new Set<string>();
  /** What is ticked in a pick-several question. */
  picked = new Set<string>();
  /** "Other…" is open: the row is a text field. */
  typing = false;
  readonly questions: AskedQuestion[];

  constructor(questions: AskedQuestion[]) {
    this.questions = questions;
  }

  get current(): AskedQuestion {
    return this.questions[Math.min(this.index, this.questions.length - 1)];
  }

  get done(): boolean {
    return this.index >= this.questions.length;
  }

  /** A click or a key on an option: answers a pick-one question, ticks or unticks otherwise. */
  choose(label: string) {
    if (this.done || this.typing) return;
    const q = this.current;
    if (!q.options.some((o) => o.label === label)) return;
    if (!q.multiSelect) {
      this.next(label);
    } else if (this.picked.has(label)) {
      this.picked.delete(label);
    } else {
      this.picked.add(label);
    }
  }

  /** Done / Next on a pick-several question: nothing while nothing is ticked. */
  confirm() {
    if (this.done || this.typing || !this.current.multiSelect || this.picked.size === 0) return;
    this.next([...this.picked]);
  }

  openOther() {
    if (!this.done) this.typing = true;
  }

  closeOther() {
    this.typing = false;
  }

  /** The words typed under "Other…": nothing while they are blank. */
  submitOther(text: string) {
    const words = text.trim();
    if (this.done || !this.typing || !words) return;
    this.next(words, true);
  }

  /** The question before, with what was ticked there ticked again. */
  back() {
    if (this.index === 0 || this.done) return;
    this.typing = false;
    this.index -= 1;
    const previous = this.answers[this.current.question];
    this.picked = new Set(Array.isArray(previous) ? previous : []);
  }

  private next(answer: string | string[], free = false) {
    const q = this.current.question;
    this.answers[q] = answer;
    if (free) this.other.add(q);
    else this.other.delete(q);
    this.picked = new Set();
    this.typing = false;
    this.index += 1;
  }
}

export interface KeyLike {
  key: string;
  ctrlKey?: boolean;
  altKey?: boolean;
  metaKey?: boolean;
  shiftKey?: boolean;
}

export interface KeyContext {
  /** The view on screen. */
  view: string;
  /** The pointer is on the card, or the island has the keyboard by a click (docs/ARCHITECTURE.md §4.1). */
  allowed: boolean;
  /** The question on screen, or null when there is none to pick from. */
  flow: QuestionFlow | null;
  /** The key was typed into a text field. */
  inField: boolean;
}

/**
 * What a key does on the question card: 1–9 pick an option, Enter confirms a
 * pick-several question. Null everywhere else, and always for a permission
 * card: that one is answered by a click on its own buttons, never by a key.
 */
export function questionKey(e: KeyLike, cx: KeyContext): { choose: string } | "confirm" | null {
  if (cx.view !== "question" || !cx.allowed || cx.inField) return null;
  const flow = cx.flow;
  if (!flow || flow.done || flow.typing) return null;
  // Ctrl+digit stays the pill shortcut.
  if (e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return null;
  if (/^[1-9]$/.test(e.key)) {
    const option = flow.current.options[Number(e.key) - 1];
    return option ? { choose: option.label } : null;
  }
  if (e.key === "Enter" && flow.current.multiSelect && flow.picked.size > 0) return "confirm";
  return null;
}

/** The keys Rust reads for the question card while the pointer is on it: its digits, and Enter when it confirms. */
export function questionKeyNames(flow: QuestionFlow | null): string[] {
  if (!flow || flow.done || flow.typing) return [];
  const digits = flow.current.options.slice(0, 9).map((_, i) => String(i + 1));
  return flow.current.multiSelect ? [...digits, "Enter"] : digits;
}

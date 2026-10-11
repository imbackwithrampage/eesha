// The permission requests and questions that wait for the user: first come,
// first served, five at most. Behaviour taken from upstream pull requests 157
// and 36; no code of theirs.
//
// Nothing in here answers anything. A request leaves the queue for exactly one
// of three reasons, each naming the request by its own id or by the session and
// turn it belongs to: the user answered that request, its turn ended, or its
// time ran out. Moving up the queue is not an answer.

/** One question of an AskUserQuestion call. */
export interface AskedQuestion {
  question: string;
  options: { label: string; description: string }[];
  multiSelect: boolean;
}

export interface ApprovalInfo {
  requestId: string;
  /** The agent it comes from, as its catalog pill: `integration_claude`, `agent_codex`… */
  family: string;
  sessionId: string;
  /** The session's turn when it asked (core/sessions.ts counts them). */
  turn: number;
  /** The pill the request belongs to: its session's. */
  pillId: string;
  tool: string;
  command: string;
  /** Set when Claude Code is asking a question rather than for a permission. */
  questions?: AskedQuestion[];
  /** The request came with a rule the agent offers to remember: what Always sends back. */
  suggested?: boolean;
}

/** Beyond this the newest request goes straight back to its terminal. */
export const MAX_APPROVALS = 5;

/**
 * Whether a click on Allow, Always or Deny counts: only one with a pointer
 * press in it. A key on a focused button (Tab, then Enter or Space) makes a
 * click too, with `detail` 0, and so does a click made by code: no key ever
 * answers a permission card.
 */
export const byPointer = (click?: { detail?: number }): boolean => (click?.detail ?? 0) > 0;

type Asked = Pick<ApprovalInfo, "family" | "sessionId" | "tool" | "command">;

const same = (a: Asked, b: Asked) =>
  a.family === b.family && a.sessionId === b.sessionId && a.tool === b.tool && a.command === b.command;

export class ApprovalQueue {
  private items: ApprovalInfo[] = [];
  /** Requests the user answered here: the tool result that follows is theirs, not a waiting twin's. */
  private settled: Asked[] = [];

  /** The request on the card. */
  get head(): ApprovalInfo | null {
    return this.items[0] ?? null;
  }

  get size(): number {
    return this.items.length;
  }

  all(): readonly ApprovalInfo[] {
    return this.items;
  }

  has(requestId: string): boolean {
    return this.items.some((a) => a.requestId === requestId);
  }

  /**
   * Takes a request in, behind the ones already waiting. "known" when that very
   * request is already here (sent twice), "full" when five are waiting.
   */
  push(info: ApprovalInfo): "queued" | "known" | "full" {
    if (this.has(info.requestId)) return "known";
    if (this.items.length >= MAX_APPROVALS) return "full";
    this.items.push(info);
    return "queued";
  }

  /** Removes one request, by its id and nothing else. `answered`: the user decided it here. */
  take(requestId: string, answered = false): ApprovalInfo | null {
    const at = this.items.findIndex((a) => a.requestId === requestId);
    if (at < 0) return null;
    const [gone] = this.items.splice(at, 1);
    if (answered) {
      this.settled.push(gone);
      if (this.settled.length > MAX_APPROVALS) this.settled.shift();
    }
    return gone;
  }

  /** What a turn that is over leaves waiting: same agent, same session, that turn or an older one. */
  ofTurn(family: string, sessionId: string, turn: number): ApprovalInfo[] {
    return this.items.filter((a) => a.family === family && a.sessionId === sessionId && a.turn <= turn);
  }

  /**
   * The waiting request a tool result settles: the tool ran or was refused, so
   * it was answered somewhere else (the terminal). A result that follows an
   * answer given here settles nothing more.
   */
  answeredBy(asked: Asked): ApprovalInfo | null {
    const mine = this.settled.findIndex((s) => same(s, asked));
    if (mine >= 0) {
      this.settled.splice(mine, 1);
      return null;
    }
    return this.items.find((a) => same(a, asked)) ?? null;
  }

  clear() {
    this.items = [];
    this.settled = [];
  }
}

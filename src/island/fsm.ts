// Island state machine (docs/ARCHITECTURE.md §4): hidden → peek → open, and
// greet at launch. Hidden until needed is the only mode.
// No DOM, no Tauri: it only reports transitions.

export type FsmState = "hidden" | "peek" | "open" | "greet";

/**
 * Settings › Island "The small island stays": once the peek strip is out it
 * leaves after a moment, stays while an agent is at work, or stays until the
 * user puts it away.
 */
export type Stay = "moment" | "agent" | "always";

/** The states of an agent session that works or waits for the user. One that rests, finished or failed is neither. */
const BUSY: ReadonlySet<string> = new Set(["working", "thinking", "searching", "approval", "question"]);

/** An agent session is working or waiting: what "While an agent is working" asks. */
export function agentBusy(tasks: readonly { state: string; pillBadge?: string | null }[]): boolean {
  return tasks.some((t) => BUSY.has(t.state) || t.pillBadge === "approval");
}

/** The setting keeps a strip that is out from leaving by itself. */
export function stays(stay: Stay, busy: boolean): boolean {
  return stay === "always" || (stay === "agent" && busy);
}

export class IslandStateMachine {
  state: FsmState = "hidden";

  onTransition: ((from: FsmState, to: FsmState) => void) | null = null;

  /**
   * open → peek delay after the pointer leaves an island opened by a click, a
   * shortcut or an alert, seconds: the auto-close preference. Changing it while
   * a countdown runs starts that countdown again with the new delay.
   */
  get openToPeekDelay(): number {
    return this.openDelay;
  }
  set openToPeekDelay(seconds: number) {
    if (!Number.isFinite(seconds) || seconds < 0 || seconds === this.openDelay) return;
    this.openDelay = seconds;
    if (this.state === "open" && this.openCollapse != null) this.scheduleOpenCollapse();
  }
  /** peek → hidden once the pointer has left, seconds. */
  peekToHiddenDelay = 2;
  /** Resting on the peek strip this long opens the island, seconds. */
  hoverOpenDelay = 0.25;
  /** greet → peek once the greeting animation ends. The greeting plays on the peek strip. */
  greetAutoCollapseDelay = 0.6;
  /** A card waiting for an answer keeps the island on screen, even folded to peek. */
  pinned = false;
  /** Resting on the peek strip opens the island (Settings › Island; on by default). */
  openOnHover = true;
  /** open → peek after the pointer leaves a hover-opened island, seconds. */
  hoverCloseDelay = 0.6;
  /** Open because of a hover, until the user clicks inside it. */
  get openedByHover(): boolean {
    return this.byHover;
  }

  /**
   * When the open island will fold, on the performance.now() clock, while the
   * mouse-leave countdown runs; null otherwise. The island draws its countdown
   * bar from it.
   */
  openCollapseDueAt: number | null = null;

  private openDelay = 15;
  private byHover = false;
  private peekHide: number | null = null;
  private openCollapse: number | null = null;
  private greetCollapse: number | null = null;
  private hoverOpen: number | null = null;
  /** Features keeping the island from folding (a drag, a share sheet, a timer page). */
  private holds = new Set<symbol>();
  /** The pointer is on the island, as last told. */
  private inside = false;
  /** The pointer has been on the island since it last came out of hidden. */
  private touched = false;
  /** A reveal keeps the peek strip until then (performance.now clock). */
  private revealUntil = 0;
  /** "The small island stays", and whether an agent session works or waits. */
  private stay: Stay = "moment";
  private busy = false;

  /** Nothing may fold or hide the island: a card waits, or a feature holds it. */
  get held(): boolean {
    return this.pinned || this.holds.size > 0;
  }

  /**
   * The strip is out and the setting alone keeps it there: no card waits on it
   * and no feature holds it. This is what a dismissal puts away, what shows the
   * fold mark and what makes Esc under the pointer the island's.
   */
  get kept(): boolean {
    return this.state === "peek" && !this.held && stays(this.stay, this.busy);
  }

  /** Where the machine believes the pointer is; the island corrects it from the cursor poll. */
  private onControl = false;

  get pointerInside(): boolean {
    return this.inside;
  }

  // ── Inputs ──────────────────────────────────────────────────────────────────

  launch() {
    this.cancelTimers();
    this.transition("greet");
  }

  mouseEntered() {
    this.inside = true;
    this.touched = true;
    switch (this.state) {
      case "hidden":
        this.cancelTimers();
        this.transition("peek");
        this.armHoverOpen();
        break;
      case "peek":
        this.clear("peekHide");
        this.armHoverOpen();
        break;
      case "open":
        this.clear("openCollapse");
        break;
      case "greet":
        // The greeting is the peek strip with the mascot arriving: a pointer on it is a pointer on the strip.
        this.cancelTimers();
        this.transition("peek");
        this.armHoverOpen();
        break;
    }
  }

  mouseLeft() {
    this.inside = false;
    // A button that went away under the pointer never said so.
    this.onControl = false;
    this.clear("hoverOpen");
    switch (this.state) {
      case "hidden":
        break;
      case "peek":
        this.schedulePeekHide();
        break;
      case "open":
        this.scheduleOpenCollapse();
        break;
      case "greet":
        break;
    }
  }

  /** A press on the peek strip opens the island for good (auto-close delay, keyboard). */
  click() {
    this.byHover = false;
    this.touched = true;
    if (this.state !== "peek" && this.state !== "greet") return;
    this.cancelTimers();
    this.transition("open");
  }

  /** Greeting animation finished. */
  greetComplete() {
    if (this.state === "greet") this.scheduleGreetCollapse(this.greetAutoCollapseDelay);
  }

  /**
   * Something to see (a notice, an activity that changed, an agent at work):
   * the peek strip for `ms`, then hidden again unless something holds it or the
   * pointer came. Does nothing to an open island. `exact` is a notice: the strip
   * is out for that notice's time, not for what an earlier one had left.
   */
  reveal(ms: number, exact = false) {
    const until = performance.now() + ms;
    this.revealUntil = exact ? until : Math.max(this.revealUntil, until);
    if (this.state === "hidden") {
      // Nothing reported the pointer while the window was the wake strip: the
      // cursor poll says where it is as soon as the window is back.
      this.inside = false;
      this.cancelTimers();
      this.transition("peek");
    }
    if (this.state === "peek") this.arm();
  }

  /**
   * The user clicked inside the island: a hover-opened island now stays like
   * any open island (normal auto-close) instead of folding once the pointer leaves.
   */
  userInteracted() {
    if (!this.byHover) return;
    this.byHover = false;
    // The short countdown already running becomes the normal one.
    if (this.state === "open" && this.openCollapse != null) this.scheduleOpenCollapse();
  }

  /** A key typed into the open island: the countdown to folding starts over. */
  activity() {
    if (this.state === "open" && this.openCollapse != null) this.scheduleOpenCollapse();
  }

  /** Alert or explicit request: open straight away. */
  forceOpen() {
    this.byHover = false;
    this.touched = true;
    this.cancelTimers();
    this.transition("open");
    this.arm();
  }

  /** Explicit close (OK button, Escape, an alert being answered or folded). */
  forcePeek() {
    this.byHover = false;
    this.touched = true;
    this.cancelTimers();
    this.transition("peek");
    this.arm();
  }

  /** Wheel up on the strip, tray Pause. */
  forceHidden() {
    this.byHover = false;
    this.cancelTimers();
    this.transition("hidden");
  }

  /**
   * The user went from the island to what it opened for them (a session's
   * terminal or app): it leaves at once instead of waiting at the strip for the
   * pointer to move off. A strip that stays, or one that is held, stays.
   */
  leave() {
    if (this.state === "hidden") return;
    if (this.held || stays(this.stay, this.busy)) this.forcePeek();
    else this.forceHidden();
  }

  /** Keeps the island from folding or hiding until the returned function is called. */
  hold(): () => void {
    const token = Symbol("hold");
    this.holds.add(token);
    this.clear("peekHide");
    this.clear("openCollapse");
    return () => {
      // The countdown the hold held back starts now.
      if (this.holds.delete(token)) this.arm();
    };
  }

  /** The setting changed, or an agent began or ended its work: a strip that no longer stays leaves as it always did. */
  setStay(stay: Stay, busy: boolean) {
    const was = stays(this.stay, this.busy);
    this.stay = stay;
    this.busy = busy;
    if (stays(stay, busy)) this.clear("peekHide");
    else if (was && this.state === "peek") this.arm();
  }

  /**
   * The user puts away a strip that stays: its fold mark, the island's own
   * shortcut. Hidden until it next comes out. False when there was nothing to
   * put away: a card that waits and a strip a feature holds are not touched.
   */
  dismiss(): boolean {
    if (!this.kept) return false;
    this.forceHidden();
    return true;
  }

  /** Esc reached the island (§4.1: it only can while the pointer is on it): the same, and only under the pointer. */
  escape(): boolean {
    return this.inside && this.dismiss();
  }

  /**
   * The pointer is on a button of the strip (the fold mark, a notice's action, Pause and Stop of a recording):
   * resting there opens nothing, whichever is heard first, the button or the strip.
   */
  overControl(on: boolean) {
    this.onControl = on;
    if (on) this.clear("hoverOpen");
    else if (this.state === "peek" && this.inside) this.armHoverOpen();
  }

  /** A card stopped waiting, or Ctrl+P let go: the countdowns may run again. */
  unpin() {
    this.pinned = false;
    this.arm();
  }

  // ── Timers ──────────────────────────────────────────────────────────────────

  /** Starts the countdown of the state we are in, when the pointer is elsewhere. */
  private arm() {
    if (this.inside) return;
    if (this.state === "peek") this.schedulePeekHide();
    else if (this.state === "open") this.scheduleOpenCollapse();
  }

  private armHoverOpen() {
    this.clear("hoverOpen");
    // A folded card is reopened by a click, never by a passing pointer.
    if (!this.openOnHover || this.pinned || this.onControl) return;
    this.hoverOpen = window.setTimeout(() => {
      this.hoverOpen = null;
      if (this.state !== "peek" || !this.inside || this.pinned) return;
      this.cancelTimers();
      this.byHover = true;
      this.transition("open");
    }, this.hoverOpenDelay * 1000);
  }

  private schedulePeekHide() {
    this.clear("peekHide");
    if (this.held || stays(this.stay, this.busy)) return;
    // 2 s after the pointer leaves; a reveal nobody touched lasts exactly its time.
    const ms = Math.max(this.touched ? this.peekToHiddenDelay * 1000 : 0, this.revealUntil - performance.now(), 0);
    this.peekHide = window.setTimeout(() => {
      this.peekHide = null;
      if (this.state === "peek" && !this.held && !this.inside) this.transition("hidden");
    }, ms);
  }

  private scheduleOpenCollapse() {
    this.clear("openCollapse");
    if (this.held) return;
    const ms = (this.byHover ? this.hoverCloseDelay : this.openDelay) * 1000;
    // The countdown bar is for the long, click-opened delay only.
    this.openCollapseDueAt = this.byHover ? null : performance.now() + ms;
    this.openCollapse = window.setTimeout(() => {
      this.openCollapse = null;
      this.openCollapseDueAt = null;
      // A card pinned or a hold taken while the countdown ran keeps the island open.
      if (this.state !== "open" || this.held) return;
      this.byHover = false;
      this.transition("peek");
      this.arm();
    }, ms);
  }

  private scheduleGreetCollapse(delay: number) {
    this.clear("greetCollapse");
    this.greetCollapse = window.setTimeout(() => {
      this.greetCollapse = null;
      if (this.state !== "greet") return;
      this.transition("peek");
      this.arm();
    }, delay * 1000);
  }

  private clear(which: "peekHide" | "openCollapse" | "greetCollapse" | "hoverOpen") {
    const id = this[which];
    if (id != null) window.clearTimeout(id);
    this[which] = null;
    if (which === "openCollapse") this.openCollapseDueAt = null;
  }

  cancelTimers() {
    this.clear("peekHide");
    this.clear("openCollapse");
    this.clear("greetCollapse");
    this.clear("hoverOpen");
  }

  private transition(next: FsmState) {
    if (next === this.state) return;
    const from = this.state;
    this.state = next;
    if (next === "hidden") {
      this.touched = false;
      this.revealUntil = 0;
    }
    // After the greeting the strip stays its two seconds, like after a visit.
    if (from === "greet") this.touched = true;
    this.onTransition?.(from, next);
  }
}

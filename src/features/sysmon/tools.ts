// Ending a process and the tools window (processes, open ports). The window
// reads only while it is visible: the list every three seconds, the ports when
// asked. Nothing is ended without a click on a confirmation that names it.
//
// Behaviour after the upstream files Services/KillProcess/KillProcessSupport.swift
// and Services/PortManager/PortManagerSupport.swift.

import type { FeatureCtx } from "../../core/feature";
import { t, tn } from "../../i18n/i18n";
import { btn, field, iconBtn, tab, toggle } from "../../ui/components";
import { icon } from "../../ui/icons";
import { h } from "../../views/dom";
import { bytes } from "./format";

/** Who a process is: an ID is handed out again, the start time with it is not. */
export interface Target {
  pid: number;
  started: string;
}

export interface KillResult {
  pid: number;
  ok: boolean;
  why: "protected" | "gone" | "denied" | "windowless" | "failed" | null;
}

interface Proc extends Target {
  name: string;
  /** 0…1 of all cores; null on the first reading. */
  cpu: number | null;
  mem: number;
  protected: boolean;
}

interface Port extends Target {
  port: number;
  address: string;
  v6: boolean;
  /** Bound to every interface. */
  all: boolean;
  name: string;
  protected: boolean;
}

/** `chrome.exe` as "chrome". */
export const appName = (image: string) => image.replace(/\.exe$/i, "");

/** A share of the whole machine's processor time, as Task Manager counts it. */
export const cpuText = (share: number | null) => (share == null ? "" : t("{n} %", { n: (share * 100).toFixed(1) }));

/** What became of a request to end or close processes, in a sentence. */
export function killSummary(results: KillResult[] | null, ask: boolean): { text: string; ok: boolean } {
  const failed = (results ?? []).filter((r) => !r.ok);
  if (!results || results.length === 0) return { text: t("It could not be ended."), ok: false };
  if (failed.length === 0) return { text: ask ? t("Asked to close.") : t("Ended."), ok: true };
  const why = failed[0].why;
  const reason =
    why === "protected" ? t("Windows needs it.")
    : why === "gone" ? t("It had already ended.")
    : why === "denied" ? t("It runs as administrator or as another user.")
    : why === "windowless" ? t("It has no window to ask. Use End now.")
    : t("Windows refused.");
  const text = results.length === 1 ? reason : `${tn("{count} was left running.", "{count} were left running.", failed.length)} ${reason}`;
  return { text, ok: false };
}

/**
 * "End chrome?" with its three answers. Asking closes it as a click on its
 * close button would (it may ask about unsaved work); ending is at once.
 */
export function confirmBar(name: string, count: number, pick: (how: "ask" | "end" | null) => void): HTMLElement {
  const question = count > 1 ? tn("End {name} ({count} process)?", "End {name} ({count} processes)?", count, { name }) : t("End {name}?", { name });
  const small = (b: HTMLButtonElement) => (b.classList.add("sm"), b);
  return h(
    "div",
    { class: "f-sysmon-confirm" },
    h("span", { text: question, title: question }),
    small(btn(t("Ask to close"), "secondary", () => pick("ask"))),
    small(btn(t("End now"), "danger", () => pick("end"))),
    small(iconBtn("close", t("Cancel"), () => pick(null))),
  );
}

// ── The window ────────────────────────────────────────────────────────────────

type SortKey = "name" | "pid" | "cpu" | "mem";

interface Entry {
  key: string;
  name: string;
  /** The process's ID, or how many processes the row stands for. */
  pid: number;
  count: number;
  cpu: number | null;
  mem: number;
  protected: boolean;
  targets: Target[];
}

export function mountTools(root: HTMLElement, cx: FeatureCtx) {
  let view: "processes" | "ports" = "processes";
  let procs: Proc[] = [];
  let ports: Port[] | null = null;
  let portsFailed = false;
  let query = "";
  let grouped = true;
  let sort: SortKey = "cpu";
  let ascending = false;
  let pending: { name: string; count: number; targets: Target[] } | null = null;
  let timer = 0;
  let noteTimer = 0;
  /** A button is held: the rows are not rebuilt under the pointer (the click would be lost). */
  let pressed = false;

  const search = field({ icon: "search", placeholder: t("Search"), onInput: (v) => ((query = v.trim().toLowerCase()), paint()) });
  const tabs = {
    processes: tab("cpu", t("Processes"), () => show("processes")),
    ports: tab("link", t("Open ports"), () => show("ports")),
  };
  const head = h("div", { class: "f-sysmon-thead" });
  const list = h("div", { class: "f-sysmon-tbody" });
  const note = h("span", { class: "f-sysmon-note" });
  const group = toggle(grouped, (on) => {
    grouped = on;
    void cx.settings.patch({ groupProcesses: on });
    paint();
  });
  const groupRow = h("label", { class: "f-sysmon-group" }, group, h("span", { text: t("One row per app") }));
  const foot = h("div", { class: "f-sysmon-tfoot" });

  const say = (text: string, ok = true) => {
    note.textContent = text;
    note.classList.toggle("bad", !ok);
    window.clearTimeout(noteTimer);
    noteTimer = window.setTimeout(() => (note.textContent = ""), 6000);
    paintFoot();
  };

  // ── Reading ──

  const readProcs = async () => {
    const got = await cx.call<Proc[]>("procs");
    // A failed reading never wipes a good list.
    if (got && got.length) procs = got;
    if (view === "processes" && !pressed) paint();
  };
  const readPorts = async () => {
    const got = await cx.call<Port[]>("ports");
    portsFailed = !got;
    ports = got ?? ports;
    if (view === "ports") paint();
  };
  const refresh = () => (view === "processes" ? readProcs() : readPorts());
  /** The list refreshes by itself only while the window can be seen. */
  const pace = () => {
    window.clearInterval(timer);
    timer = 0;
    if (document.hidden) return;
    void refresh();
    if (view === "processes") timer = window.setInterval(() => void readProcs(), 3000);
  };

  // ── Ending ──

  const ask = (name: string, count: number, targets: Target[]) => {
    pending = { name, count, targets };
    paintFoot();
  };
  const answer = async (how: "ask" | "end" | null) => {
    const what = pending;
    pending = null;
    paintFoot();
    if (!what || !how) return;
    const results = await cx.call<KillResult[]>("kill", { targets: what.targets, ask: how === "ask" });
    const said = killSummary(results, how === "ask");
    say(t("{name}: {result}", { name: what.name, result: said.text }), said.ok);
    // The rows go at once where it worked; the real list follows.
    const gone = new Set((results ?? []).filter((r) => r.ok && how === "end").map((r) => r.pid));
    procs = procs.filter((p) => !gone.has(p.pid));
    paint();
    window.setTimeout(() => void refresh(), 500);
  };

  // ── Drawing ──

  const entries = (): Entry[] => {
    const rows = new Map<string, Entry>();
    for (const p of procs) {
      const key = grouped ? p.name.toLowerCase() : String(p.pid);
      const e = rows.get(key);
      if (!e) {
        rows.set(key, { key, name: appName(p.name), pid: p.pid, count: 1, cpu: p.cpu, mem: p.mem, protected: p.protected, targets: [{ pid: p.pid, started: p.started }] });
        continue;
      }
      e.count++;
      e.cpu = p.cpu == null ? e.cpu : (e.cpu ?? 0) + p.cpu;
      e.mem += p.mem;
      e.protected ||= p.protected;
      e.targets.push({ pid: p.pid, started: p.started });
    }
    const found = [...rows.values()].filter((e) => !query || e.name.toLowerCase().includes(query) || e.targets.some((x) => String(x.pid) === query));
    const by: Record<SortKey, (a: Entry, b: Entry) => number> = {
      name: (a, b) => a.name.localeCompare(b.name),
      pid: (a, b) => (grouped ? a.count - b.count : a.pid - b.pid),
      cpu: (a, b) => (a.cpu ?? 0) - (b.cpu ?? 0),
      mem: (a, b) => a.mem - b.mem,
    };
    // Ties by ID, so rows do not swap places between two readings.
    return found.sort((a, b) => (ascending ? by[sort](a, b) : by[sort](b, a)) || a.pid - b.pid);
  };

  const column = (key: SortKey, label: string) => {
    const el = h("button", { class: `f-sysmon-th${key === sort ? " on" : ""}` }, h("span", { text: label }), key === sort ? icon(ascending ? "chevu" : "chevd", 11) : null);
    el.addEventListener("click", () => {
      // The same column again turns it round; a new one starts with the most (names from A).
      ascending = key === sort ? !ascending : key === "name";
      sort = key;
      paint();
    });
    return el;
  };

  const endButton = (name: string, count: number, targets: Target[]) => {
    const b = iconBtn("close", t("End"), () => ask(name, count, targets));
    b.classList.add("sm");
    return b;
  };

  function paintProcesses() {
    head.className = "f-sysmon-thead f-sysmon-pcols";
    head.replaceChildren(column("name", t("Name")), column("pid", grouped ? t("Processes") : t("PID")), column("cpu", t("CPU")), column("mem", t("Memory")), h("span"));
    const rows = entries();
    list.replaceChildren(
      ...rows.map((e) =>
        h(
          "div",
          { class: "f-sysmon-tr f-sysmon-pcols" },
          h("span", { class: "f-sysmon-name", text: e.name, title: e.name }),
          h("span", { class: "f-sysmon-num", text: grouped ? String(e.count) : String(e.pid) }),
          h("span", { class: "f-sysmon-num", text: cpuText(e.cpu) }),
          h("span", { class: "f-sysmon-num", text: bytes(e.mem) }),
          e.protected ? h("span") : endButton(e.name, e.count, e.targets),
        ),
      ),
    );
    return rows.length === 0 ? t("No process matches.") : tn("{count} process", "{count} processes", procs.length);
  }

  function paintPorts() {
    head.className = "f-sysmon-thead f-sysmon-ocols";
    head.replaceChildren(
      h("span", { class: "f-sysmon-th", text: t("Port") }),
      h("span", { class: "f-sysmon-th", text: t("Address") }),
      h("span", { class: "f-sysmon-th", text: t("Process") }),
      h("span", { class: "f-sysmon-th", text: t("PID") }),
      h("span"),
    );
    const found = (ports ?? []).filter((p) => !query || `${p.port} ${p.name} ${p.pid} ${p.address}`.toLowerCase().includes(query));
    list.replaceChildren(
      ...found.map((p) => {
        const open = iconBtn("external", t("Open in the browser"), () => void cx.call("openPort", { port: p.port, address: p.address }));
        open.classList.add("sm");
        const name = appName(p.name) || t("Ended");
        return h(
          "div",
          { class: "f-sysmon-tr f-sysmon-ocols" },
          h("b", { class: "f-sysmon-num", text: String(p.port) }),
          h(
            "span",
            { class: `f-sysmon-addr${p.all ? " all" : ""}`, title: p.all ? t("Listens on every network: other devices can reach it unless a firewall stops them.") : p.address },
            p.all ? icon("alert", 12) : null,
            h("span", { text: p.all ? t("Every network") : p.address }),
          ),
          h("span", { class: "f-sysmon-name", text: name, title: p.name }),
          h("span", { class: "f-sysmon-num", text: String(p.pid) }),
          h("span", { class: "f-sysmon-acts" }, open, p.protected ? null : endButton(t("{name}, port {port}", { name, port: p.port }), 1, [{ pid: p.pid, started: p.started }])),
        );
      }),
    );
    if (portsFailed && !ports) return t("The ports could not be listed. Try again.");
    if (!ports) return t("Reading…");
    return found.length === 0 ? t("No port matches.") : tn("{count} port open", "{count} ports open", ports.length);
  }

  let count = "";
  function paintFoot() {
    foot.replaceChildren(
      ...(pending
        ? [confirmBar(pending.name, pending.count, (how) => void answer(how))]
        : [...(view === "processes" ? [groupRow] : []), h("span", { class: "f-sysmon-count", text: count }), note]),
    );
  }

  function paint() {
    count = view === "processes" ? paintProcesses() : paintPorts();
    paintFoot();
  }

  function show(next: "processes" | "ports") {
    view = next;
    for (const [name, el] of Object.entries(tabs)) el.classList.toggle("on", name === view);
    pending = null;
    list.scrollTop = 0;
    paint();
    pace();
  }

  const reload = iconBtn("refresh", t("Refresh"), () => void refresh());
  root.className = "f-sysmon-win";
  root.append(h("div", { class: "f-sysmon-bar" }, tabs.processes, tabs.ports, h("span", { class: "f-sysmon-gap" }), search, reload), head, list, foot);

  list.addEventListener("pointerdown", () => (pressed = true));
  window.addEventListener("pointerup", () => (pressed = false));
  document.addEventListener("visibilitychange", pace);
  cx.on<string>("tab", (name) => show(name === "ports" ? "ports" : "processes"));
  void Promise.all([cx.settings.get(), cx.call<string>("tab")]).then(([stored, name]) => {
    grouped = stored.groupProcesses !== false;
    group.setAttribute("aria-checked", String(grouped));
    show(name === "ports" ? "ports" : "processes");
  });
}

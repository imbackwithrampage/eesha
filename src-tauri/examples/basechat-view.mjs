// Walks the chat view (src/views/chat.ts) through what was added to it, on the
// tests' pretend DOM and pretend Rust side, without the app:
//
//   node --import ./tests/setup.mjs src-tauri/examples/basechat-view.mjs
//
// The search field, a model server found on this computer, the CLI chip, the
// error note, Clear chat, the task chip, and Stop.

import assert from "node:assert/strict";
import { installFakeDom } from "../../tests/fakedom.mjs";
import { emit, internals, sent, calls } from "../../tests/tauri.mjs";

installFakeDom();
const { buildPrompt } = await import("../../src/views/chat.ts");
const { DEFAULT_SETTINGS, State } = await import("../../src/core/state.ts");

/** What the pretend Rust side answers, by command. */
let answers = {};
const plainInvoke = internals.invoke;
internals.invoke = async (cmd, args) => {
  const result = await plainInvoke(cmd, args);
  if (cmd in answers) return typeof answers[cmd] === "function" ? answers[cmd](args) : answers[cmd];
  return result;
};
/** chat.rs's questions ride `local_connect`: what each of them answers. */
const asked = (table) => (args) => (args.provider in table ? { url: "", models: [], loopback: true, extra: table[args.provider] } : null);
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

let view;
/** A new chat view; `first` is what Rust answers while it is built and first shown. */
function fresh(settings = {}, first = {}) {
  calls.length = 0;
  answers = first;
  State.settings = { ...DEFAULT_SETTINGS, chatModels: {}, ...settings };
  State.chatHistory = [];
  State.tasks = [];
  State.stateOverride = null;
  State.view = "prompt";
  State.mode = "expanded";
  view = buildPrompt(() => {});
  view.sync();
}
const $ = (cls) => view.el.querySelector(cls);
const texts = (cls) => view.el.find(cls).map((el) => el.textContent);

// ── 1. The search field ───────────────────────────────────────────────────────

fresh();
answers.secret_present = true;
answers.chat_models = Array.from({ length: 40 }, (_, i) => ({ id: `claude-model-${i}`, label: i === 7 ? "Claude Sonnet 5" : `Claude Model ${i}` }));
$(".model-btn").fire("click");
await flush();
assert.ok($(".chat-body").classList.contains("picking"));
assert.ok($(".picker-search-field").classList.contains("on"), "forty models: the search field is up");
assert.equal(texts(".picker-model").length, 40);
const search = $(".picker-search-field").children[1];
search.value = "sonnet";
search.fire("input");
assert.deepEqual(texts(".picker-model"), ["Claude Sonnet 5"]);
search.value = "nothing like it";
search.fire("input");
assert.deepEqual(texts(".picker-model"), []);
assert.match($(".picker-status").textContent, /No model matches/);
search.value = "";
search.fire("input");
assert.equal(texts(".picker-model").length, 40);
view.el.find(".picker-model")[7].fire("click");
assert.equal(State.settings.model, "claude-model-7");
assert.ok(!$(".chat-body").classList.contains("picking"));

// A short list has no search field.
fresh();
answers.secret_present = true;
answers.chat_models = [{ id: "a", label: "A" }, { id: "b", label: "B" }];
$(".model-btn").fire("click");
await flush();
assert.ok(!$(".picker-search-field").classList.contains("on"));
console.log("1. the search field shows past ten models and filters the list");

// ── 2. A model server found on this computer, and the CLI chip ────────────────

fresh();
answers.secret_present = false;
answers.local_connect = asked({
  "chat:scan": [
    { id: "ollama", name: "Ollama", url: "http://127.0.0.1:11434", models: ["llama3.2", "qwen2.5"] },
    { id: "custom", name: "Jan", url: "http://127.0.0.1:1337", models: [] },
  ],
  "chat:clis": [{ id: "claude", name: "Claude Code" }],
});
$(".model-btn").fire("click");
await flush();
assert.deepEqual(texts(".picker-chip"), ["Anthropic", "Google", "OpenAI", "OpenRouter", "CLI", "Ollama"]);
const offer = view.el.find(".picker-chip").at(-1);
assert.ok(offer.classList.contains("found"), "a found server is an offer; one without models is not offered");
answers.chat_models = (args) => (args.provider === "ollama" ? [{ id: "llama3.2", label: "llama3.2" }, { id: "qwen2.5", label: "qwen2.5" }] : []);
offer.fire("click");
await flush();
assert.equal(State.settings.ollamaUrl, "http://127.0.0.1:11434");
assert.equal(State.settings.chatProvider, "ollama");
assert.equal(State.settings.chatModels.ollama, "llama3.2");
assert.deepEqual(sent("secret_set"), [], "connected with no key");
assert.deepEqual(texts(".picker-chip"), ["Anthropic", "Google", "OpenAI", "OpenRouter", "Ollama", "CLI"]);
assert.deepEqual(texts(".picker-model"), ["llama3.2", "qwen2.5"]);

answers.chat_models = (args) => (args.provider === "cli" ? [{ id: "claude", label: "Claude Code" }] : []);
view.el.find(".picker-chip").at(-1).fire("click");
await flush();
assert.equal(State.settings.chatProvider, "cli");
assert.deepEqual(texts(".picker-model"), ["Claude Code"]);
assert.equal(State.settings.chatModels.cli, "claude");
assert.equal($(".model-name").textContent, "Claude Code");
console.log("2. a found server connects in one click with no key; the CLI chip lists what is installed");

// ── 3. The error note stays in the chat, on one line, and can be closed ───────

fresh({ chatProvider: "google" });
answers.chat_send = () => {
  throw new Error("Google AI: model not found (404). Pick another one above the chat box. This model is gone.\nA second line of advice for whoever writes code.");
};
$(".chat-input").value = "hello";
$(".send-btn").fire("click");
await flush();
view.sync();
assert.equal(State.view, "prompt", "the chat stays: no note view takes its place");
assert.ok($(".chat-note").classList.contains("on"));
assert.equal($(".chat-note-text").textContent, "Google AI: model not found (404). Pick another one above the chat box. This model is gone.");
assert.match($(".chat-note").title, /second line/, "the whole message is the tooltip");
$(".chat-note").find("button")[0].fire("click");
assert.ok(!$(".chat-note").classList.contains("on"), "the close button closes it");
console.log("3. an error is one line in the chat, with a close button");

// ── 4. Clear chat ─────────────────────────────────────────────────────────────

fresh();
State.chatHistory = [{ id: 901, role: "user", content: "a" }, { id: 902, role: "assistant", content: "b" }];
State.promptContext = { kind: "file", name: "notes.txt", path: "C:\\inbox\\notes.txt" };
view.sync();
assert.equal(texts(".bubble").length, 1);
$(".chat-clear").fire("click");
view.sync();
assert.deepEqual(State.chatHistory, []);
assert.equal(State.promptContext, null);
assert.equal(sent("chat_reset").length, 1, "Rust forgets the conversation too");
assert.equal(texts(".bubble").length, 0);
assert.equal($(".chat-clear").style.display, "none", "nothing to clear: no button");
console.log("4. Clear chat empties the chat on both sides");

// ── 5. The task chip ──────────────────────────────────────────────────────────

// No Claude Code on this computer: no task chip.
fresh();
await flush();
assert.equal($(".task-chip").style.display, "none");

fresh({}, { local_connect: asked({ "chat:clis": [{ id: "claude", name: "Claude Code" }] }) });
await flush();
assert.equal($(".task-chip").style.display, "", "with Claude Code installed the chip is there");
answers.feature_settings = { taskFolders: ["C:\\work\\site"] };
answers.chat_send = (args) => (args.context?.kind === "task" ? { text: "", task: 7 } : { text: "a chat answer" });
$(".task-chip").fire("click");
await flush();
assert.ok($(".task-chip").classList.contains("on"));
$(".chat-input").value = "list the TODOs";
$(".send-btn").fire("click");
await flush();
await flush();
view.sync();
assert.deepEqual(sent("chat_send"), [{ query: "list the TODOs", context: { kind: "task", folder: "C:\\work\\site" } }]);
assert.deepEqual(State.chatHistory.map((m) => m.content), ["list the TODOs", "Running in site…"]);
assert.ok($(".chat-row.task"), "the running task has its line, with Cancel");
const pill = State.tasks.find((task) => task.id === "task_7");
assert.equal(pill.state, "working");
assert.equal(pill.sessionCwd, "C:\\work\\site");
assert.ok(!$(".task-chip").classList.contains("on"), "one message, one task");
assert.ok(!$(".send-btn").disabled, "the chat stays free while the task runs");

// Cancel goes to Rust; the answer comes back as an event, in Markdown.
$(".chat-row.task").find("button")[0].fire("click");
await flush();
assert.deepEqual(sent("local_connect").at(-1), { provider: "chat:task-cancel", url: "7" });
emit("chat-task", { id: 7, ok: true, cancelled: false, text: "## Done\n\nThree TODOs, all in **main.ts**." });
view.sync();
assert.equal(State.chatHistory[1].content, "## Done\n\nThree TODOs, all in **main.ts**.");
assert.equal(view.el.find(".reply")[0].find("STRONG")[0].textContent, "main.ts");
assert.equal($(".chat-row.task"), null);
assert.equal(pill.state, "finished");
assert.equal(pill.finalLine, "Done");
console.log("5. the task chip hands the message over, shows a session pill, and the answer returns as Markdown");

// ── 6. Stop, while a CLI writes ───────────────────────────────────────────────

fresh({ chatProvider: "cli", chatModels: { cli: "claude" } });
let fail;
answers.chat_send = () => new Promise((_, reject) => (fail = reject));
$(".chat-input").value = "a long question";
$(".send-btn").fire("click");
await flush();
view.sync();
assert.equal($(".send-btn").dataset.mode, "stop");
assert.ok(!$(".send-btn").disabled);
$(".send-btn").fire("click");
await flush();
assert.deepEqual(sent("local_connect").at(-1), { provider: "chat:stop", url: "" });
fail(new Error("stopped"));
await flush();
view.sync();
assert.ok(!$(".chat-note").classList.contains("on"), "a turn the user stopped is not an error");
assert.equal($(".send-btn").dataset.mode, "send");
console.log("6. Stop ends a CLI's answer, and says nothing more");

console.log("\nthe chat view does what was added to it");
// A finished task's pill is taken away by a timer: nothing else to wait for.
process.exit(0);

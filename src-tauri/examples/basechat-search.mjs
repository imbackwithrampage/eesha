// Checks the model picker's search (src/core/providers.ts) and the chips it
// shows, without the app:
//
//   node --import ./tests/setup.mjs src-tauri/examples/basechat-search.mjs

import assert from "node:assert/strict";
import { CLI, OPENROUTER_FREE, SEARCH_FROM, searchModels, visibleProviders } from "../../src/core/providers.ts";
import { DEFAULT_SETTINGS } from "../../src/core/state.ts";

// A list the size of OpenRouter's.
const makers = ["anthropic", "google", "openai", "meta-llama", "mistralai", "qwen", "deepseek", "x-ai"];
const models = [];
for (let i = 0; i < 320; i++) {
  const maker = makers[i % makers.length];
  models.push({ id: `${maker}/model-${i}`, label: `${maker.toUpperCase()}: Model ${i}` });
}
models.push({ id: "anthropic/claude-sonnet-5", label: "Anthropic: Claude Sonnet 5" });
models.push({ id: "qwen/qwen3-coder:free", label: "Qwen: Qwen3 Coder (free)" });

assert.ok(models.length > SEARCH_FROM, "a list this long gets the search field");
const ids = (typed) => searchModels(models, typed).map((m) => m.id);

assert.equal(ids("").length, models.length, "nothing typed: every model");
assert.equal(ids("   ").length, models.length);
assert.deepEqual(ids("sonnet"), ["anthropic/claude-sonnet-5"]);
// Whatever the case, in the name or in the id, every word wherever it is.
assert.deepEqual(ids("CLAUDE son"), ["anthropic/claude-sonnet-5"]);
assert.deepEqual(ids("coder qwen"), ["qwen/qwen3-coder:free"]);
assert.deepEqual(ids(":free"), ["qwen/qwen3-coder:free"]);
assert.equal(ids("deepseek").length, 40);
assert.deepEqual(ids("no such model"), []);
// The list it was given is left as it was.
assert.equal(models.length, 322);

// The chips: what the settings alone give, then what the picker finds when it opens.
const chips = (settings, extras) => visibleProviders({ ...DEFAULT_SETTINGS, ...settings }, extras).map((p) => p.id);
assert.deepEqual(chips({}), ["anthropic", "google", "openai", "openrouter"]);
assert.deepEqual(chips({}, { cli: true }), ["anthropic", "google", "openai", "openrouter", CLI.id]);
assert.deepEqual(chips({}, { openrouterFree: true }), ["anthropic", "google", "openai", OPENROUTER_FREE.id]);
// The picker never hides where the chat goes.
assert.deepEqual(chips({ chatProvider: "openrouter" }, { openrouterFree: true }), ["anthropic", "google", "openai", "openrouter", OPENROUTER_FREE.id]);
assert.deepEqual(chips({ chatProvider: "cli" }), ["anthropic", "google", "openai", "openrouter", "cli"]);

console.log(`the search finds its model among ${models.length}, and the chips are right`);

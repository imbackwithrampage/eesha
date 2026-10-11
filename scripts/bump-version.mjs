// Sets the version in the three places it lives, so a release cannot ship with them apart.
//
//   node scripts/bump-version.mjs 1.0.1

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const version = process.argv[2];
if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) {
  console.error("bump-version: give the new version, for example 1.0.1");
  process.exit(1);
}

/** Replaces the first match of `pattern` in a file; fails loudly when there is none. */
function set(file, pattern, replacement) {
  const path = join(root, file);
  const text = readFileSync(path, "utf8");
  if (!pattern.test(text)) throw new Error(`${file}: no version line found`);
  writeFileSync(path, text.replace(pattern, replacement));
  console.log(`${file} -> ${version}`);
}

set("package.json", /("version":\s*")[^"]+(")/, `$1${version}$2`);
set("src-tauri/tauri.conf.json", /("version":\s*")[^"]+(")/, `$1${version}$2`);
set("Cargo.toml", /(^version\s*=\s*")[^"]+(")/m, `$1${version}$2`);
console.log("Now add the entry to CHANGELOG.md, run `cargo check --workspace` so Cargo.lock follows, and commit.");

// Stubs a feature package (docs/ARCHITECTURE.md §3).
//
//   node scripts/new-feature.mjs <id> [--service <name>]
//
// Creates, from scripts/templates/:
//   crates/<id>/            the Rust crate `notch-feat-<id>` (a `Feature` stub, off by default)
//   src/features/<id>/      index.ts, style.css, strings.json, tsconfig.json
//   crates/svc-<name>/      with --service: a plain library crate for a shared service
// and edits exactly one shared file, src-tauri/Cargo.toml, adding the line
//   notch-feat-<id> = { path = "../crates/<id>" }
// (kept sorted). The app's build.rs turns those lines into the feature list and
// src/core/registry.ts finds the TypeScript side by glob, so nothing else
// changes. Run `cargo check --workspace` afterwards so Cargo.lock is complete.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const templates = join(root, "scripts", "templates");

function fail(message) {
  console.error(`new-feature: ${message}`);
  process.exit(1);
}

const args = process.argv.slice(2);
const at = args.indexOf("--service");
const service = at >= 0 ? args[at + 1] : null;
if (at >= 0) args.splice(at, 2);
const [id] = args;

const KEBAB = /^[a-z][a-z0-9]*(-[a-z0-9]+)*$/;
// Taken by the workspace's own crates and the built-in features of src-tauri/src/features.
const RESERVED = ["core", "brand", "agents", "chat", "integrations", "drop", "recap"];

if (!id || args.length !== 1 || (at >= 0 && !service)) fail("usage: node scripts/new-feature.mjs <id> [--service <name>]");
if (!KEBAB.test(id)) fail(`"${id}" is not a feature id: lowercase letters, digits and single dashes (media, huds-power)`);
if (RESERVED.includes(id) || id.startsWith("svc-")) fail(`"${id}" is reserved`);
if (service && !KEBAB.test(service)) fail(`"${service}" is not a service name`);

const crateDir = join(root, "crates", id);
const pageDir = join(root, "src", "features", id);
const serviceDir = service ? join(root, "crates", `svc-${service}`) : null;
for (const dir of [crateDir, pageDir, serviceDir]) {
  if (dir && existsSync(dir)) fail(`${dir} already exists`);
}

/** "huds-power" → "Huds power" (the manifest's English name, edited by the package). */
const title = (kebab) => kebab.charAt(0).toUpperCase() + kebab.slice(1).replaceAll("-", " ");
/** "huds-power" → "HudsPower" */
const pascal = (kebab) => kebab.split("-").map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join("");

function write(from, to, name) {
  const text = readFileSync(join(templates, from), "utf8")
    .replaceAll("__ID__", name)
    .replaceAll("__NAME__", title(name))
    .replaceAll("__TYPE__", pascal(name));
  mkdirSync(dirname(to), { recursive: true });
  writeFileSync(to, text);
  console.log(`  created ${to.slice(root.length + 1).replaceAll("\\", "/")}`);
}

write("feature/Cargo.toml", join(crateDir, "Cargo.toml"), id);
write("feature/lib.rs", join(crateDir, "src", "lib.rs"), id);
for (const file of ["index.ts", "style.css", "strings.json", "tsconfig.json"]) {
  write(`feature/${file}`, join(pageDir, file), id);
}
if (serviceDir) {
  write("service/Cargo.toml", join(serviceDir, "Cargo.toml"), service);
  write("service/lib.rs", join(serviceDir, "src", "lib.rs"), service);
}

// The one shared file: the app's dependency list, feature lines kept sorted
// right under notch-core.
const manifestPath = join(root, "src-tauri", "Cargo.toml");
const lines = readFileSync(manifestPath, "utf8").split("\n");
const isFeature = (line) => line.startsWith("notch-feat-");
const features = lines.filter(isFeature).map((line) => line.trimEnd());
features.push(`notch-feat-${id} = { path = "../crates/${id}" }`);
features.sort();
const rest = lines.filter((line) => !isFeature(line));
const anchor = rest.findIndex((line) => line.startsWith("notch-core "));
if (anchor < 0) fail("src-tauri/Cargo.toml has no notch-core dependency to put the feature under");
rest.splice(anchor + 1, 0, ...features);
writeFileSync(manifestPath, rest.join("\n"));
console.log(`  edited  src-tauri/Cargo.toml (+ notch-feat-${id})`);

console.log(`\nNext: cargo check --workspace, then
  cargo check -p notch-feat-${id}
  npx tsc --noEmit -p src/features/${id}`);

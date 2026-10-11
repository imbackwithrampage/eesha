// Rewrites the dependency table of NOTICE.md (between the deps markers) from
// `cargo metadata` and the npm packages package.json ships: `node scripts/licenses.mjs`.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const meta = JSON.parse(
  execFileSync("cargo", ["metadata", "--format-version", "1", "--filter-platform", "x86_64-pc-windows-msvc"], {
    cwd: root,
    maxBuffer: 256 * 1024 * 1024,
  }),
);
const ours = new Set(meta.workspace_members);
// Only what a Windows build links: the resolved graph, without dev-only crates.
const linked = new Set();
const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
const walk = (id) => {
  if (linked.has(id)) return;
  linked.add(id);
  for (const d of nodes.get(id).deps) if (d.dep_kinds.some((k) => k.kind !== "dev")) walk(d.pkg);
};
for (const id of ours) walk(id);

const families = new Map();
const add = (license, name) => {
  const key = (license || "not stated").replace(/\s*\/\s*/g, " OR ").trim();
  families.set(key, [...(families.get(key) ?? []), name]);
};
for (const p of meta.packages) if (linked.has(p.id) && !ours.has(p.id)) add(p.license, p.name);

const npm = Object.keys(JSON.parse(readFileSync(join(root, "package.json"), "utf8")).dependencies).map((name) => {
  const p = JSON.parse(readFileSync(join(root, "node_modules", name, "package.json"), "utf8"));
  return `${name} ${p.version} (${p.license})`;
});

const rows = [...families].sort((a, b) => b[1].length - a[1].length);
const lines = [
  "| License | Rust crates | Which |",
  "| --- | ---: | --- |",
  ...rows.map(([license, names]) => {
    const unique = [...new Set(names)].sort();
    return `| ${license} | ${unique.length} | ${unique.length <= 8 ? unique.join(", ") : ""} |`;
  }),
  "",
  `npm packages shipped in the app: ${npm.join(", ")}.`,
];

const file = join(root, "NOTICE.md");
const text = readFileSync(file, "utf8");
const [start, end] = ["<!-- deps:start -->", "<!-- deps:end -->"];
if (!text.includes(start) || !text.includes(end)) throw new Error("NOTICE.md has no deps markers");
writeFileSync(file, text.slice(0, text.indexOf(start) + start.length) + "\n" + lines.join("\n") + "\n" + text.slice(text.indexOf(end)));
console.log(`${rows.reduce((n, [, names]) => n + new Set(names).size, 0)} crates in ${rows.length} license families, ${npm.length} npm packages`);

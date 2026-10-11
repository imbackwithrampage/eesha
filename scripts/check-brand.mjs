// Fails when the upstream product or character name appears in the path or the
// content of any tracked or untracked-but-not-ignored file, except under docs/,
// licenses/ and site/ and the files NOTICE.md, CLAUDE.md, README.md and
// CONTRIBUTING.md, which may name where the product comes from (text only).
// Shipped code, strings and assets stay clean. Prints path:line, exits 1.
//
//   node scripts/check-brand.mjs

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
// Spelled in two halves so this file does not trip its own check.
const BANNED = new RegExp("cou" + "cou|mo" + "chi", "i");
const SKIPPED = (p) =>
  ["docs/", "licenses/", "site/"].some((dir) => p.startsWith(dir)) || ["NOTICE.md", "CLAUDE.md", "README.md", "CONTRIBUTING.md"].includes(p);

const listed = execFileSync("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard"], {
  cwd: root,
  maxBuffer: 1 << 28,
}).toString("utf8");
const files = [...new Set(listed.split("\0").filter(Boolean))].filter((p) => !SKIPPED(p));

const hits = [];
for (const path of files) {
  if (BANNED.test(path)) hits.push(`${path}  (file name)`);
  const abs = join(root, path);
  if (!existsSync(abs)) continue;
  const bytes = readFileSync(abs);
  if (bytes.includes(0)) {
    if (BANNED.test(bytes.toString("latin1"))) hits.push(`${path}  (binary content)`);
    continue;
  }
  bytes
    .toString("utf8")
    .split(/\r?\n/)
    .forEach((line, i) => {
      if (BANNED.test(line)) hits.push(`${path}:${i + 1}: ${line.trim().slice(0, 120)}`);
    });
}

if (hits.length) {
  console.error(hits.join("\n"));
  console.error(`\n${hits.length} upstream name(s) found.`);
  process.exit(1);
}
console.log("check-brand: clean");

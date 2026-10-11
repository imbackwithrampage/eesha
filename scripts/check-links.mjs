// Checks the links of the Markdown files that are published: every relative link
// and picture must exist on disk, and none may point into, or name, the working
// papers that the public tree leaves out. Prints file:line, exits 1.
//
//   node scripts/check-links.mjs

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
// Left out of the public tree: not checked, and nothing may link to them.
const PAPERS = ["docs/PLAN.md", "docs/REVIEW.md", "docs/review/", "docs/research/"];
const inPapers = (p) => PAPERS.some((w) => (w.endsWith("/") ? p.startsWith(w) || p === w.slice(0, -1) : p === w));
const SKIPPED = (p) => inPapers(p) || ["site/", "node_modules/", "reference/"].some((dir) => p.startsWith(dir));

const files = execFileSync("git", ["ls-files", "-z", "--cached", "--others", "--exclude-standard", "*.md"], { cwd: root, maxBuffer: 1 << 26 })
  .toString("utf8")
  .split("\0")
  .filter((p) => p && !SKIPPED(p) && existsSync(join(root, p)));

const missing = [];
const papers = [];
let checked = 0;
for (const file of new Set(files)) {
  let fenced = false;
  readFileSync(join(root, file), "utf8")
    .split(/\r?\n/)
    .forEach((line, i) => {
      const at = `${file}:${i + 1}`;
      // A path written out is a pointer too, link or not.
      const named = line.match(/\b(?:docs\/)?(?:PLAN|REVIEW)\.md\b|\bdocs\/(?:review|research)\b/);
      if (named) papers.push(`${at}: names ${named[0]}  (a working paper, not published)`);
      if (/^\s*(```|~~~)/.test(line)) fenced = !fenced;
      if (fenced) return;
      // [text](target), ![alt](target), [ref]: target, and src/href in inline HTML.
      const targets = [
        ...[...line.matchAll(/\]\(\s*<?([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)/g)].map((m) => m[1]),
        ...[...line.matchAll(/^\s*\[[^\]]+\]:\s*<?(\S+?)>?(?:\s|$)/g)].map((m) => m[1]),
        ...[...line.matchAll(/\b(?:src|href)="([^"]+)"/g)].map((m) => m[1]),
      ];
      for (const raw of targets) {
        if (/^(https?:|mailto:|#)/i.test(raw)) continue;
        const path = decodeURIComponent(raw.split("#")[0].split("?")[0]);
        if (!path) continue;
        const full = normalize(join(root, dirname(file), path));
        // Links such as ../../releases leave the repository on purpose: they are pages of the host.
        if (full !== root && !full.startsWith(root + sep)) continue;
        const inside = full.slice(root.length + 1).split(sep).join("/");
        checked++;
        if (inPapers(inside) || inPapers(inside + "/")) papers.push(`${at}: ${raw}  (a working paper, not published)`);
        else if (!existsSync(full)) missing.push(`${at}: ${raw}`);
      }
    });
}

for (const line of [...papers, ...missing]) console.error(line);
if (papers.length || missing.length) {
  console.error(`\n${papers.length} pointer(s) into working papers, ${missing.length} link(s) to nothing, of ${checked} checked in ${new Set(files).size} files.`);
  process.exit(1);
}
console.log(`check-links: clean (${checked} relative links in ${new Set(files).size} files)`);

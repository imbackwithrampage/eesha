// Checks the generated site: every internal link, anchor and picture resolves, each page has one h1,
// and no page uses the old code name as the product's name. Run: node site/_src/check.mjs
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const site = join(dirname(fileURLToPath(import.meta.url)), "..");
const pages = readdirSync(site).filter((f) => f.endsWith(".html"));
const html = Object.fromEntries(pages.map((f) => [f, readFileSync(join(site, f), "utf8")]));
let bad = 0;
const fail = (page, what) => { bad++; console.log(`${page}: ${what}`); };

for (const [page, s] of Object.entries(html)) {
  if ((s.match(/<h1/g) || []).length !== 1) fail(page, "not exactly one h1");
  for (const [, attr, raw] of s.matchAll(/\s(href|src)="([^"]*)"/g)) {
    if (/^(https?:|mailto:|data:)/.test(raw) || raw === "") continue;
    const [file, hash] = raw.replace(/^\//, "").replace(/^\.\/$/, "index.html").split("#");
    // Pages are linked without ".html" (the host adds it back): "download" is download.html.
    const target = !file ? page : !existsSync(join(site, file)) && existsSync(join(site, file + ".html")) ? file + ".html" : file;
    if (!existsSync(join(site, target))) { fail(page, `${attr} ${raw} does not exist`); continue; }
    if (hash && target.endsWith(".html") && !new RegExp(`id="${hash}"`).test(html[target])) fail(page, `anchor ${raw} does not exist`);
  }
  // "notch" is fine as the ordinary word and inside the names of other apps; anywhere else it is the old code name.
  // Code blocks are left out: the crates keep the prefix notch- on purpose.
  const text = s.replace(/<pre[\s\S]*?<\/pre>|<code[\s\S]*?<\/code>|<[^>]+>/g, " ")
    .replace(/NotchNook|Notchify|TopNotch|NotchBar|NotchFlow|DropNotch|Boring Notch|notch apps?|(a|the) notch\b/g, " ");
  for (const m of text.matchAll(/notch/gi)) fail(page, `old name: "${text.slice(Math.max(0, m.index - 24), m.index + 24).trim()}"`);
}
const used = new Set(Object.values(html).flatMap((s) => [...s.matchAll(/img\/shots\/([\w-]+\.png)/g)].map((m) => m[1])));
for (const f of readdirSync(join(site, "img", "shots"))) if (!used.has(f)) fail("img/shots", `${f} is not used on any page`);
console.log(bad ? `${bad} problems` : `ok: ${pages.length} pages, ${used.size} pictures, every link and picture resolves`);
process.exitCode = bad ? 1 : 0;

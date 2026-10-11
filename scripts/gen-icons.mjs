// Draws the app and tray icon, the mascot's face (docs/ARCHITECTURE.md §9), into the
// PNG/ICO set Tauri needs. It follows the drawing in src/mascot/engine.ts: an orb in the
// colour the VS Code pill has in the catalog, one dark band, one bar of light. That is the
// colour the mascot wears outside the island until the user picks another for that pill
// (src/island/tray-mascot.ts then draws the tray, the windows and the shortcuts in theirs),
// so a fresh install looks the same in the island, the tray and on the Desktop.
// No dependencies: rasterised here and encoded with node:zlib.
//
//   node scripts/gen-icons.mjs

import { deflateSync } from "node:zlib";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = join(ROOT, "src-tauri", "icons");

// ── The face ─────────────────────────────────────────────────────────────────

// Colours come from the design tokens, the one place a hex is written.
const TOKENS = readFileSync(join(ROOT, "src", "ui", "tokens.css"), "utf8");
function token(name) {
  let hex = TOKENS.match(new RegExp(`${name}:\\s*#([0-9a-f]{3,6})`, "i"))[1];
  if (hex.length === 3) hex = hex.replace(/./g, "$&$&");
  return [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16));
}
const mix = (a, b, t) => a.map((v, i) => v + (b[i] - v) * t);
const WHITE = [255, 255, 255];
const BLACK = token("--bg");
const hex = (text) => [0, 2, 4].map((i) => parseInt(text.slice(i, i + 2), 16));
/** The orb: the accent, the product's colour until the user gives the VS Code pill one (BRAND_DEFAULT in src/island/tray-mascot.ts). */
const BODY = token("--accent");
const GLASS = mix(BLACK, token("--panel"), 0.6);
const SS = 4; // supersampling factor

/** Distance from (x, y) to a w × h bar with round ends, centred on the origin; 0 or less is inside. */
function barDistance(x, y, w, h) {
  const r = Math.min(w, h) / 2;
  const dx = Math.max(Math.abs(x) - (w / 2 - r), 0);
  const dy = Math.max(Math.abs(y) - (h / 2 - r), 0);
  return Math.hypot(dx, dy) - r;
}

/**
 * The face's measures at `size`, as in the engine. Up to 48 px the band and the bar snap
 * to whole pixels, the bar is a little wider and has no glow, so the tray icon stays crisp.
 */
function measures(size) {
  const d = size;
  const c = size / 2;
  const base = { c, r: d * 0.46, rim: d > 24 };
  if (size > 48) {
    return { ...base, bandW: d * 0.7, bandH: d * 0.28, bandY: c - d * 0.1, barW: d * 0.3, barH: d * 0.074, glow: d * 0.06 };
  }
  const barH = Math.max(2, Math.round(d * 0.074));
  const bandH = barH + 2 * Math.round((d * 0.28 - barH) / 2);
  const top = Math.round(c - d * 0.1 - bandH / 2);
  return { ...base, bandW: 2 * Math.round(d * 0.35), bandH, bandY: top + bandH / 2, barW: 2 * Math.round(d * 0.18), barH, glow: 0 };
}

function renderIcon(size) {
  const px = new Uint8Array(size * size * 4);
  const m = measures(size);
  const rimR = m.r - size * 0.03;
  const rimW = Math.max(1, size * 0.022);
  const top = mix(BODY, WHITE, 0.3);
  const bottom = mix(BODY, BLACK, 0.32);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const sum = [0, 0, 0];
      let inside = 0;
      for (let sy = 0; sy < SS; sy++) {
        for (let sx = 0; sx < SS; sx++) {
          const dx = x + (sx + 0.5) / SS - m.c;
          const dy = y + (sy + 0.5) / SS - m.c;
          const far = Math.hypot(dx, dy);
          if (far > m.r) continue;
          inside++;
          // The orb, shaded top to bottom.
          const t = (dy + m.r) / (2 * m.r);
          let col = t < 0.55 ? mix(top, BODY, t / 0.55) : mix(BODY, bottom, (t - 0.55) / 0.45);
          // A thin rim of light, top left.
          const angle = Math.atan2(dy, dx);
          if (m.rim && Math.abs(far - rimR) <= rimW / 2 && angle > -2.75 && angle < -1.2) col = mix(col, WHITE, 0.28);
          // The band, and the bar of light with its glow.
          const by = dy - (m.bandY - m.c);
          if (barDistance(dx, by, m.bandW, m.bandH) <= 0) col = mix(col, GLASS, 0.94);
          const bar = barDistance(dx, by, m.barW, m.barH);
          if (bar <= 0) col = WHITE;
          else if (m.glow) col = mix(col, WHITE, 0.3 * Math.exp(-((bar / m.glow) ** 2)));
          for (let i = 0; i < 3; i++) sum[i] += col[i];
        }
      }
      if (!inside) continue;
      const o = (y * size + x) * 4;
      for (let i = 0; i < 3; i++) px[o + i] = Math.round(sum[i] / inside);
      px[o + 3] = Math.round((inside / (SS * SS)) * 255);
    }
  }
  return px;
}

// ── PNG ───────────────────────────────────────────────────────────────────────

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (const b of buf) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

function encodePNG(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    Buffer.from(rgba.buffer, y * size * 4, size * 4).copy(raw, y * (size * 4 + 1) + 1);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

// ── ICO: BMP entries below 256 px (RC.EXE rejects small PNG entries), PNG at 256 ──

function encodeDIB(size, rgba) {
  const maskRow = Math.ceil(size / 32) * 4;
  const head = Buffer.alloc(40);
  head.writeUInt32LE(40, 0);
  head.writeInt32LE(size, 4);
  head.writeInt32LE(size * 2, 8);
  head.writeUInt16LE(1, 12);
  head.writeUInt16LE(32, 14);
  head.writeUInt32LE(size * size * 4 + maskRow * size, 20);
  const pixels = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const src = ((size - 1 - y) * size + x) * 4;
      const dst = (y * size + x) * 4;
      pixels[dst] = rgba[src + 2];
      pixels[dst + 1] = rgba[src + 1];
      pixels[dst + 2] = rgba[src];
      pixels[dst + 3] = rgba[src + 3];
    }
  }
  return Buffer.concat([head, pixels, Buffer.alloc(maskRow * size)]);
}

function encodeICO(entries) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(entries.length, 4);
  const dir = Buffer.alloc(16 * entries.length);
  let offset = header.length + dir.length;
  entries.forEach((e, i) => {
    const o = i * 16;
    dir[o] = e.size >= 256 ? 0 : e.size;
    dir[o + 1] = e.size >= 256 ? 0 : e.size;
    dir[o + 2] = 0;
    dir[o + 3] = 0;
    dir.writeUInt16LE(1, o + 4);
    dir.writeUInt16LE(32, o + 6);
    dir.writeUInt32LE(e.png.length, o + 8);
    dir.writeUInt32LE(offset, o + 12);
    offset += e.png.length;
  });
  return Buffer.concat([header, dir, ...entries.map((e) => e.png)]);
}

// ── Go ────────────────────────────────────────────────────────────────────────

mkdirSync(OUT, { recursive: true });

const png = (size) => encodePNG(size, renderIcon(size));

const files = {
  "32x32.png": png(32),
  "128x128.png": png(128),
  "128x128@2x.png": png(256),
  "icon.png": png(512),
};
for (const [name, data] of Object.entries(files)) {
  writeFileSync(join(OUT, name), data);
  console.log(`${name} — ${data.length} bytes`);
}

const ico = encodeICO(
  [16, 24, 32, 48, 64, 128, 256].map((size) => ({
    size,
    png: size >= 256 ? png(size) : encodeDIB(size, renderIcon(size)),
  })),
);
writeFileSync(join(OUT, "icon.ico"), ico);
console.log(`icon.ico — ${ico.length} bytes`);

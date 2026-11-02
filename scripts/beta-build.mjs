#!/usr/bin/env node
// Makes the macOS signing check build a beta that installs next to the stable app. Used by
// .github/workflows/macos-signing.yml when the "beta" input is on, and by nothing else; the files
// of the stable app (src-tauri/tauri.conf.json, src-tauri/icons) are only read, never changed.
//
//   node scripts/beta-build.mjs prepare
//
// It does two things, after scripts/prepare-bundle.mjs has written
// src-tauri/tauri.bundle.generated.conf.json (the config every later step passes to Tauri):
//
//   1. Draws a "BETA" ribbon across the lower right corner of the app icon (assets/app-icon.png) and
//      makes the whole icon set from it with the Tauri CLI, into src-tauri/icons-beta.
//   2. Merges the beta settings over that generated config, so the compile and the signing step
//      need no change: another product name and identifier, no update files, no update server, no
//      file association, the new icons, and a version that says it is a beta build.
//
// No dependency is added: the PNG files are read and written with node:zlib, the ribbon is drawn
// here, and the icon set comes from the Tauri CLI that package-lock.json pins.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync, inflateSync } from "node:zlib";
import { isMain } from "./is-main.mjs";

export const BETA_PRODUCT_NAME = "Patient Partridge Beta";
/** Not the identifier of the stable app: settings, positions and tabs are kept in a folder named after it. */
export const BETA_IDENTIFIER = "com.ferdinand99.patientpartridge.beta";
/** Where the icon set is made, relative to src-tauri (where Tauri reads the paths of `bundle.icon`). */
export const BETA_ICON_DIR = "icons-beta";
const ICON_FILES = ["32x32.png", "128x128.png", "128x128@2x.png", "icon.icns", "icon.ico", "icon.png"];

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

// ---- the beta settings -----------------------------------------------------------------------

/** The version of the beta build: the version of the app, a beta mark and the number of the run. */
export function betaVersion(version, build) {
  const n = String(build ?? "").trim();
  if (!/^\d+$/.test(n)) throw new Error("GITHUB_RUN_NUMBER is not a number: the beta build has no number");
  return `${version.replace(/-.*$/, "")}-beta.${n}`;
}

/**
 * What differs in a beta build, merged over the config of the signed build.
 *  - productName: the name of the app, the disk image, the folder in Applications, the window title.
 *  - identifier: a folder of its own for settings, positions and tabs (the app data folder is named
 *    after it), its own web view storage and its own single-instance lock.
 *  - createUpdaterArtifacts off and no update endpoint: nothing is made that an updater could
 *    install, and the app has no address to ask.
 *  - no file association: the beta does not offer itself for .pdf files.
 */
export function betaOverride({ version, build }) {
  return {
    productName: BETA_PRODUCT_NAME,
    identifier: BETA_IDENTIFIER,
    version: betaVersion(version, build),
    bundle: {
      createUpdaterArtifacts: false,
      fileAssociations: [],
      icon: ICON_FILES.map((f) => `${BETA_ICON_DIR}/${f}`),
    },
    plugins: { updater: { endpoints: [] } },
  };
}

/** JSON merge patch (RFC 7396), the way Tauri applies a `--config` file: objects merge, arrays and values replace. */
export function mergeConfig(base, patch) {
  if (patch === null || typeof patch !== "object" || Array.isArray(patch)) return patch;
  const out = base !== null && typeof base === "object" && !Array.isArray(base) ? { ...base } : {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) delete out[key];
    else out[key] = mergeConfig(out[key], value);
  }
  return out;
}

// ---- PNG, 8 bits per channel, not interlaced -------------------------------------------------

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buffer) {
  let c = 0xffffffff;
  for (const byte of buffer) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/** Reads a PNG into `{ width, height, data }` with `data` as RGBA, 4 bytes a pixel. */
export function decodePng(buffer) {
  if (!buffer.subarray(0, 8).equals(PNG_SIGNATURE)) throw new Error("not a PNG file");
  let width = 0;
  let height = 0;
  let colorType = -1;
  const parts = [];
  for (let at = 8; at < buffer.length; ) {
    const length = buffer.readUInt32BE(at);
    const type = buffer.toString("latin1", at + 4, at + 8);
    const body = buffer.subarray(at + 8, at + 8 + length);
    if (type === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      colorType = body[9];
      if (body[8] !== 8 || body[12] !== 0) throw new Error("only 8-bit PNG files that are not interlaced are read");
    } else if (type === "IDAT") parts.push(body);
    else if (type === "IEND") break;
    at += 12 + length;
  }
  const channels = { 2: 3, 6: 4 }[colorType];
  if (!channels) throw new Error(`PNG colour type ${colorType} is not read (RGB and RGBA are)`);
  const raw = inflateSync(Buffer.concat(parts));
  const stride = width * channels;
  const data = new Uint8Array(width * height * 4);
  let previous = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = Uint8Array.from(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? line[i - channels] : 0;
      const b = previous[i];
      const c = i >= channels ? previous[i - channels] : 0;
      let add = 0;
      if (filter === 1) add = a;
      else if (filter === 2) add = b;
      else if (filter === 3) add = (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        add = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      } else if (filter !== 0) throw new Error(`PNG filter ${filter} is not known`);
      line[i] = (line[i] + add) & 0xff;
    }
    for (let x = 0; x < width; x++) {
      for (let k = 0; k < 3; k++) data[(y * width + x) * 4 + k] = line[x * channels + k];
      data[(y * width + x) * 4 + 3] = channels === 4 ? line[x * channels + 3] : 255;
    }
    previous = line;
  }
  return { width, height, data };
}

/** Writes `{ width, height, data }` (RGBA) as a PNG file. */
export function encodePng({ width, height, data }) {
  const chunk = (type, body) => {
    const out = Buffer.alloc(12 + body.length);
    out.writeUInt32BE(body.length, 0);
    out.write(type, 4, "latin1");
    Buffer.from(body).copy(out, 8);
    out.writeUInt32BE(crc32(out.subarray(4, 8 + body.length)), 8 + body.length);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 6;
  const raw = Buffer.alloc(height * (width * 4 + 1));
  for (let y = 0; y < height; y++) Buffer.from(data.buffer, data.byteOffset + y * width * 4, width * 4).copy(raw, y * (width * 4 + 1) + 1);
  return Buffer.concat([PNG_SIGNATURE, chunk("IHDR", header), chunk("IDAT", deflateSync(raw, { level: 9 })), chunk("IEND", Buffer.alloc(0))]);
}

/** Halves the size of an image, weighting each pixel by its opacity. */
export function halve({ width, height, data }) {
  const w = width >> 1;
  const h = height >> 1;
  const out = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;
      for (const [dx, dy] of [[0, 0], [1, 0], [0, 1], [1, 1]]) {
        const i = ((y * 2 + dy) * width + x * 2 + dx) * 4;
        const alpha = data[i + 3];
        r += data[i] * alpha;
        g += data[i + 1] * alpha;
        b += data[i + 2] * alpha;
        a += alpha;
      }
      const o = (y * w + x) * 4;
      if (a > 0) {
        out[o] = Math.round(r / a);
        out[o + 1] = Math.round(g / a);
        out[o + 2] = Math.round(b / a);
      }
      out[o + 3] = Math.round(a / 4);
    }
  }
  return { width: w, height: h, data: out };
}

// ---- the ribbon ------------------------------------------------------------------------------

// Hedgerow's own violet (--brand in src/styles/tokens.css) for the ribbon, its lighter shade for
// the edges, white for the letters.
const RIBBON = [0x66, 0x56, 0xe8];
const RIBBON_EDGE = [0x8b, 0x7d, 0xff];
const LETTERS = [0xff, 0xff, 0xff];

// Coordinates below are fractions of the icon's width. x + y = t is a line across the lower right
// corner at 45 degrees; the ribbon is the part of the icon between two such lines.
const RIBBON_CENTRE = 1.46;
const RIBBON_HALF = 0.17; // in t; the ribbon is 0.24 of the icon's width across
const EDGE_WIDTH = 0.03; // in t
const CELL = 0.0205; // one dot of a letter
const BOLD = 0.2; // how far a dot of a letter is widened, in dots, so that the strokes hold up when small

const FONT = {
  B: ["11110", "10001", "10001", "11110", "10001", "10001", "11110"],
  E: ["11111", "10000", "10000", "11110", "10000", "10000", "11111"],
  T: ["11111", "00100", "00100", "00100", "00100", "00100", "00100"],
  A: ["01110", "10001", "10001", "11111", "10001", "10001", "10001"],
};
const WORD = "BETA";
const GAP = 1;
const TEXT_COLUMNS = WORD.length * 5 + (WORD.length - 1) * GAP;
const TEXT_ROWS = 7;

/** Whether the point, in dots from the top left of the word, is on a letter. */
function onLetter(column, row) {
  for (let k = Math.ceil(column - 1 - BOLD); k <= Math.floor(column + BOLD); k++) {
    const letter = Math.floor(k / (5 + GAP));
    const inside = k - letter * (5 + GAP);
    if (letter < 0 || letter >= WORD.length || inside > 4 || k < 0) continue;
    for (let j = Math.ceil(row - 1 - BOLD); j <= Math.floor(row + BOLD); j++) {
      if (j >= 0 && j < TEXT_ROWS && FONT[WORD[letter]][j][inside] === "1") return true;
    }
  }
  return false;
}

const SAMPLES = 4;

/** How much of a pixel is ribbon (`band`, `edge`) and letter (`text`), from samples inside the pixel. */
function coverage(px, py, size) {
  let band = 0;
  let edge = 0;
  let text = 0;
  const middle = RIBBON_CENTRE / 2;
  for (let sy = 0; sy < SAMPLES; sy++) {
    for (let sx = 0; sx < SAMPLES; sx++) {
      const x = (px + (sx + 0.5) / SAMPLES) / size;
      const y = (py + (sy + 0.5) / SAMPLES) / size;
      const across = x + y - RIBBON_CENTRE; // in t
      if (Math.abs(across) > RIBBON_HALF) continue;
      band++;
      if (Math.abs(across) > RIBBON_HALF - EDGE_WIDTH) {
        edge++;
        continue;
      }
      // Along the ribbon, rising to the right; the letters stand on the side of the tile's middle.
      const along = (x - middle - (y - middle)) / Math.SQRT2;
      const down = (x - middle + (y - middle)) / Math.SQRT2;
      const column = along / CELL + TEXT_COLUMNS / 2;
      const row = down / CELL + TEXT_ROWS / 2;
      if (onLetter(column, row)) text++;
    }
  }
  const n = SAMPLES * SAMPLES;
  return { band: band / n, edge: edge / n, text: text / n };
}

const mix = (from, to, amount) => from + (to - from) * amount;

/** The icon with a "BETA" ribbon across its lower right corner. Where the icon is see-through, it stays so. */
export function badgeIcon({ width, height, data }) {
  if (width !== height) throw new Error("the icon is not square");
  const out = new Uint8Array(data);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const o = (y * width + x) * 4;
      if (out[o + 3] === 0) continue;
      const c = coverage(x, y, width);
      if (c.band === 0) continue;
      for (let k = 0; k < 3; k++) {
        let v = mix(out[o + k], RIBBON[k], c.band);
        v = mix(v, RIBBON_EDGE[k], c.edge);
        v = mix(v, LETTERS[k], c.text);
        out[o + k] = Math.round(v);
      }
    }
  }
  return { width, height, data: out };
}

// ---- the command -----------------------------------------------------------------------------

function prepare() {
  const stable = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8"));
  const generated = join(root, "src-tauri", "tauri.bundle.generated.conf.json");
  if (!existsSync(generated)) throw new Error(`${generated} is not there: run prepare-bundle first`);

  const icon = badgeIcon(decodePng(readFileSync(join(root, "assets", "app-icon.png"))));
  const work = join(root, "beta-preview");
  mkdirSync(work, { recursive: true });
  const source = join(work, "beta-icon-source.png");
  writeFileSync(source, encodePng(icon));

  const iconDir = join(root, "src-tauri", BETA_ICON_DIR);
  const made = spawnSync("npx", ["tauri", "icon", source, "--output", iconDir], { cwd: root, stdio: "inherit", shell: process.platform === "win32" });
  if (made.status !== 0) throw new Error("the Tauri CLI did not make the icon set");
  for (const file of ICON_FILES) if (!existsSync(join(iconDir, file))) throw new Error(`${file} was not made`);

  const build = process.env.GITHUB_RUN_NUMBER;
  const merged = mergeConfig(JSON.parse(readFileSync(generated, "utf8")), betaOverride({ version: stable.version, build }));
  writeFileSync(generated, `${JSON.stringify(merged, null, 2)}\n`);
  console.log(`Beta build ${merged.version}: ${merged.productName}, ${merged.identifier}; update files, update endpoint and file association are off.`);
}

if (isMain(import.meta.url)) {
  const [command] = process.argv.slice(2);
  if (command !== "prepare") {
    console.error("usage: beta-build.mjs prepare");
    process.exit(2);
  }
  try {
    prepare();
  } catch (error) {
    console.error(`::error title=Beta build::${error.message}`);
    process.exit(1);
  }
}

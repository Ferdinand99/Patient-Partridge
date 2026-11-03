// The picture of a signature, on the window's side: what a picture is made of, the limits it is
// held to, and the small pure functions that turn pointer positions, a typed name or an imported
// image into something the engine takes. Nothing here touches the page, the engine or the disk.
//
// A picture of a signature is a picture. It is not a digital signature, it does not show who
// placed it, and the words in this file say so. The limits are the engine's own (the protocol
// holds the same ones), so a picture the window makes is never refused for its size.

import type { Rgba } from "./annotations";

// ---- the limits (the same numbers as the protocol's) -----------------------------------------

/** Most strokes in a drawn picture, and most points in all of them. */
export const MAX_STROKES = 64;
export const MAX_POINTS = 4_096;

/** Largest side of the box a drawn picture is made in, in its own units. */
export const MAX_SPACE = 4_096;

/** Thinnest and thickest line of a drawn picture, in the units of its box. */
export const MIN_LINE = 0.1;
export const MAX_LINE = 256;

/** Most characters of a typed signature. */
export const MAX_TEXT_CHARS = 40;

/** Largest side of an imported picture, in pixels, and most pixels in all. */
export const MAX_IMAGE_SIDE = 1_024;
export const MAX_IMAGE_PIXELS = 1_024 * 512;

/** Largest file the window reads as an image to import, in bytes. */
export const MAX_IMAGE_FILE_BYTES = 12 * 1024 * 1024;

/**
 * Largest side and most pixels an image file may declare in its header and still be decoded. A
 * file of a few kilobytes can declare a picture of many gigabytes, and the decoder makes all of
 * it before the window can scale it down. The signature picture is scaled to a much smaller size
 * afterwards, so these only guard the decoder.
 */
export const MAX_DECODE_SIDE = 8_192;
export const MAX_DECODE_PIXELS = 40_000_000;

/** How much of the start of an image file is read to find its size in the header. */
export const HEADER_BYTES = 2 * 1024 * 1024;

/** Most signatures kept for later, the longest name of one, and the most bytes of a kept image (PNG). */
export const MAX_SAVED = 20;
export const MAX_NAME_CHARS = 60;
export const MAX_SAVED_PNG_BYTES = 400_000;

/** Smallest side of a picture on a page, in points. */
export const MIN_SIDE = 8;

/** The width a new picture gets on a page, in points (never more than half the page). */
export const DEFAULT_WIDTH = 160;

// ---- the shapes ------------------------------------------------------------------------------

export type FontId = "great-vibes" | "allura" | "kristi";

export interface SignatureFontInfo {
  id: FontId;
  /** The name shown to the person. */
  label: string;
  /** The name the window's own copy of the font goes by in CSS (see signpicture.css). */
  family: string;
}

/** The fonts a typed signature can be set in, in the order they are offered. */
export const SIGNATURE_FONTS: readonly SignatureFontInfo[] = [
  { id: "great-vibes", label: "Great Vibes", family: "Partridge Great Vibes" },
  { id: "allura", label: "Allura", family: "Partridge Allura" },
  { id: "kristi", label: "Kristi", family: "Partridge Kristi" },
];

export function fontInfo(id: FontId): SignatureFontInfo {
  return SIGNATURE_FONTS.find((f) => f.id === id) ?? SIGNATURE_FONTS[0];
}

/** Lines in a box of `width` by `height` units, from the top left, drawn with a line of `line` units. */
export interface DrawnPicture {
  kind: "draw";
  width: number;
  height: number;
  line: number;
  strokes: [number, number][][];
}

export interface TypedPicture {
  kind: "type";
  font: FontId;
  text: string;
}

/** An imported picture as raw pixels, red, green, blue and alpha, row by row from the top. */
export interface ImagePicture {
  kind: "image";
  width: number;
  height: number;
  rgba: Uint8ClampedArray;
}

export type Picture = DrawnPicture | TypedPicture | ImagePicture;

/**
 * A picture the person chose to place: the colour a drawn or typed one is drawn in (an image keeps
 * its own colours), and its width over height, which is measured for a typed one.
 */
export type Chosen = Picture & { color: Rgba; aspect: number };

/** The colours of ink offered for a drawn or typed picture. The first is the default. */
export const INK_COLORS: readonly { id: string; label: string; rgba: Rgba }[] = [
  { id: "black", label: "Black", rgba: [20, 20, 22, 255] },
  { id: "blue", label: "Blue", rgba: [24, 56, 150, 255] },
];

/** The thicknesses of the pen offered for a drawn picture, in pixels of the drawing surface. */
export const PEN_THICKNESS: readonly { id: string; label: string; width: number }[] = [
  { id: "thin", label: "Thin", width: 2 },
  { id: "medium", label: "Medium", width: 4 },
  { id: "thick", label: "Thick", width: 7 },
];

/** A picture kept for later, as the app process keeps it. An image is kept as a PNG in base64. */
export type SavedBody =
  | { kind: "draw"; width: number; height: number; line: number; strokes: [number, number][][] }
  | { kind: "type"; font: string; text: string }
  | { kind: "image"; width: number; height: number; png: string };

export type SavedPicture = { id: string; name: string; created: number } & SavedBody;

// ---- what the window says --------------------------------------------------------------------

/** The words of the signature picture. A test reads them all and looks for what must not be said. */
export const PICTURE_WORDS = {
  tool: "Signature picture",
  notice:
    "This puts a picture of your signature on the page. A picture is not a digital signature. It does not show who placed it, and anyone can copy it.",
  saved:
    "Saved signatures are kept on this computer only, in the data folder of Patient Partridge. Anyone who can read that folder can use them. Remove them with Forget saved signatures in the settings menu.",
  placing: "Signature picture: click on the page to place it.",
  forgetTitle: "Forget saved signatures?",
  forgetDetail:
    "Patient Partridge keeps the signatures you saved on this computer only. Forgetting them deletes all of them from this computer. Signature pictures that are already on a page stay there.",
  drawHint: "Draw your signature with the mouse, a pen or a finger.",
  typeHint: "Type your name. It is set in the font you pick.",
  imageHint: "Choose a picture of your signature. It is read in this window and never leaves this computer.",
  full: "That is as much as a signature picture holds. Undo the last line or clear it to draw more.",
  tooLong: "A signature picture holds at most 40 characters.",
  missingLetter: "This font has no letter for one of the characters. Take it out or pick another font.",
  imageFailed: "That file could not be read as a picture. Choose a PNG, JPEG, WebP, GIF or BMP file.",
  imageTooBig: "That file is too big to read. Choose a smaller picture.",
  storeFull: "As many signatures are saved as can be. Delete one first.",
  placed: "Signature picture placed.",
  resizeHint: "Plus and minus change the size.",
} as const;

/** Every text above and the labels of the dialog, for the test that looks for words not to say. */
export function pictureWords(): string[] {
  return Object.values(PICTURE_WORDS);
}

// ---- text ------------------------------------------------------------------------------------

/** True for a character that is not shown or that steers the text around it: controls, line and direction marks. */
function hidden(cp: number): boolean {
  return (
    cp < 0x20 ||
    (cp >= 0x7f && cp <= 0x9f) ||
    cp === 0x2028 ||
    cp === 0x2029 ||
    (cp >= 0x200b && cp <= 0x200f) ||
    (cp >= 0x202a && cp <= 0x202e) ||
    (cp >= 0x2066 && cp <= 0x2069) ||
    cp === 0xfeff
  );
}

/** The text with its hidden characters turned into spaces. */
function flatten(text: string): string {
  return Array.from(text)
    .map((c) => (hidden(c.codePointAt(0) ?? 0) ? " " : c))
    .join("");
}

/** A typed name made fit: one line, no control characters, no more than the cap. */
export function cleanTypedText(text: string): string {
  const flat = flatten(text).replace(/\s+/g, " ").trim();
  return Array.from(flat).slice(0, MAX_TEXT_CHARS).join("");
}

/** True when a name can be set: it has something that is not a space. */
export function typedOk(text: string): boolean {
  const clean = cleanTypedText(text);
  return clean.length > 0 && Array.from(clean).length <= MAX_TEXT_CHARS;
}

/** A name for a kept signature: trimmed, on one line, within the cap. Empty when nothing is left. */
export function cleanName(name: string): string {
  return Array.from(flatten(name).trim()).slice(0, MAX_NAME_CHARS).join("");
}

// ---- drawing ---------------------------------------------------------------------------------

export type Point = [number, number];

/** One round of corner cutting: a line made of straight pieces becomes rounder, with twice the points. */
export function chaikin(points: Point[]): Point[] {
  if (points.length < 3) return points.slice();
  const out: Point[] = [points[0]];
  for (let i = 0; i < points.length - 1; i++) {
    const [ax, ay] = points[i];
    const [bx, by] = points[i + 1];
    out.push([0.75 * ax + 0.25 * bx, 0.75 * ay + 0.25 * by], [0.25 * ax + 0.75 * bx, 0.25 * ay + 0.75 * by]);
  }
  out.push(points[points.length - 1]);
  return out;
}

/** Drops a point closer than `distance` to the last one kept; the first and the last stay. */
export function thin(points: Point[], distance: number): Point[] {
  if (points.length <= 2) return points.slice();
  const out: Point[] = [points[0]];
  let last = points[0];
  for (let i = 1; i < points.length - 1; i++) {
    if (Math.hypot(points[i][0] - last[0], points[i][1] - last[1]) >= distance) {
      out.push(points[i]);
      last = points[i];
    }
  }
  out.push(points[points.length - 1]);
  return out;
}

/** How many strokes and points a set of strokes has. */
export function strokeTotals(strokes: Point[][]): { strokes: number; points: number } {
  return { strokes: strokes.length, points: strokes.reduce((n, s) => n + s.length, 0) };
}

/** True when another stroke may be begun: below the cap of strokes and of points. */
export function canBeginStroke(strokes: Point[][]): boolean {
  const t = strokeTotals(strokes);
  return t.strokes < MAX_STROKES && t.points < MAX_POINTS;
}

/**
 * The picture that drawn strokes make. `pixels` are positions on the drawing surface, `line` is the
 * thickness of the pen in the same unit. The box is cut to the lines with the pen's width around
 * them, so the picture on the page has no empty margin. Returns null when nothing was drawn.
 * Lines are rounded, thinned and, when there are more points than the cap, thinned further, so
 * that what is returned is always within the limits.
 */
export function drawnPicture(pixels: Point[][], line: number): DrawnPicture | null {
  const strokes = pixels.filter((s) => s.length > 0).slice(0, MAX_STROKES);
  if (strokes.length === 0) return null;
  const width = Math.min(Math.max(line, MIN_LINE), MAX_LINE);
  let shaped = strokes.map((s) => thin(chaikin(s), 1));
  // Over the cap of points: keep every n-th point of each line, and the ends.
  let guard = 0;
  while (strokeTotals(shaped).points > MAX_POINTS && guard++ < 16) {
    const factor = Math.ceil(strokeTotals(shaped).points / MAX_POINTS) + (guard > 1 ? 1 : 0);
    shaped = shaped.map((s) => {
      if (s.length <= 2) return s;
      const kept = s.filter((_, i) => i % factor === 0);
      if (kept[kept.length - 1] !== s[s.length - 1]) kept.push(s[s.length - 1]);
      return kept;
    });
  }
  if (strokeTotals(shaped).points > MAX_POINTS) {
    // One point for each stroke is the least a stroke can be; keep as many whole strokes as fit.
    const keep: Point[][] = [];
    let n = 0;
    for (const s of shaped) {
      if (n + s.length > MAX_POINTS) break;
      keep.push(s);
      n += s.length;
    }
    shaped = keep;
    if (shaped.length === 0) return null;
  }
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const s of shaped) {
    for (const [x, y] of s) {
      if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
      minX = Math.min(minX, x);
      minY = Math.min(minY, y);
      maxX = Math.max(maxX, x);
      maxY = Math.max(maxY, y);
    }
  }
  const pad = width / 2 + 1;
  const w = maxX - minX + 2 * pad;
  const h = maxY - minY + 2 * pad;
  if (w > MAX_SPACE || h > MAX_SPACE) return null;
  const round = (v: number) => Math.round(v * 100) / 100;
  return {
    kind: "draw",
    width: round(w),
    height: round(h),
    line: round(width),
    strokes: shaped.map((s) => s.map(([x, y]): Point => [round(x - minX + pad), round(y - minY + pad)])),
  };
}

// ---- images ----------------------------------------------------------------------------------

/** The size an imported picture is scaled to: whole pixels, inside the side and the pixel caps. */
export function fitImageSize(width: number, height: number): { width: number; height: number } | null {
  if (!(width > 0) || !(height > 0) || !Number.isFinite(width) || !Number.isFinite(height)) return null;
  const scale = Math.min(
    1,
    MAX_IMAGE_SIDE / width,
    MAX_IMAGE_SIDE / height,
    Math.sqrt(MAX_IMAGE_PIXELS / (width * height)),
  );
  let w = Math.max(1, Math.floor(width * scale));
  let h = Math.max(1, Math.floor(height * scale));
  while (w * h > MAX_IMAGE_PIXELS && w > 1 && h > 1) {
    w = Math.max(1, w - 1);
    h = Math.max(1, Math.floor((height * w) / width));
  }
  return { width: w, height: h };
}

/** The size an image file declares, in pixels. */
export interface ImageSize {
  width: number;
  height: number;
}

/** Most blocks of a GIF that are walked before the file is given up on (a block is at least a byte). */
const MAX_GIF_BLOCKS = 2_000_000;

/**
 * The size of a GIF as a decoder will make it: the largest of the screen and of every frame (its
 * place plus its size). The walk goes from block to block, over the colour tables, the extensions
 * and the sub-blocks of data, to the trailer. A file that is cut short, has a block that is not
 * one, or has no frame at all gives `null`: it is not decoded.
 */
function gifSize(bytes: Uint8Array): ImageSize | null {
  const n = bytes.length;
  if (n < 13) return null;
  const le16 = (at: number) => bytes[at] | (bytes[at + 1] << 8);
  // The index after a run of sub-blocks (a length, then that many bytes, ending with a length of
  // 0), or -1 when the file ends first. Each step moves on by at least a byte.
  const skipSubBlocks = (from: number): number => {
    let at = from;
    while (at < n) {
      const length = bytes[at];
      at += 1 + length;
      if (length === 0) return at;
    }
    return -1;
  };
  let width = le16(6);
  let height = le16(8);
  let at = 13 + (bytes[10] & 0x80 ? 3 * (1 << ((bytes[10] & 7) + 1)) : 0);
  let frames = 0;
  for (let blocks = 0; blocks < MAX_GIF_BLOCKS; blocks++) {
    if (at >= n) return null;
    const block = bytes[at];
    if (block === 0x3b) {
      return frames > 0 && width > 0 && height > 0 ? { width, height } : null;
    }
    if (block === 0x21) {
      // An extension: its label, then sub-blocks.
      at = skipSubBlocks(at + 2);
      if (at < 0) return null;
    } else if (block === 0x2c) {
      // A frame: left, top, width, height, a byte of flags, perhaps a colour table, the size of
      // the code, and the data in sub-blocks.
      if (at + 10 > n) return null;
      width = Math.max(width, le16(at + 1) + le16(at + 5));
      height = Math.max(height, le16(at + 3) + le16(at + 7));
      frames += 1;
      const flags = bytes[at + 9];
      at += 10 + (flags & 0x80 ? 3 * (1 << ((flags & 7) + 1)) : 0) + 1;
      at = skipSubBlocks(at);
      if (at < 0) return null;
    } else {
      return null;
    }
  }
  return null;
}

/**
 * The width and the height an image file declares in its header: PNG, JPEG, WebP, GIF and BMP.
 * `null` for a file that is none of these or whose header is cut short or does not hold a size.
 * Nothing is decoded.
 */
export function imageSizeOf(bytes: Uint8Array): ImageSize | null {
  const u8 = (at: number) => (at >= 0 && at < bytes.length ? bytes[at] : -1);
  const be16 = (at: number) => (at + 1 < bytes.length ? (bytes[at] << 8) | bytes[at + 1] : -1);
  const le16 = (at: number) => (at + 1 < bytes.length ? bytes[at] | (bytes[at + 1] << 8) : -1);
  const be32 = (at: number) => (at + 3 < bytes.length ? bytes[at] * 0x1000000 + ((bytes[at + 1] << 16) | (bytes[at + 2] << 8) | bytes[at + 3]) : -1);
  const le32 = (at: number) => (at + 3 < bytes.length ? bytes[at] + (bytes[at + 1] << 8) + (bytes[at + 2] << 16) + bytes[at + 3] * 0x1000000 : -1);
  const le24 = (at: number) => (at + 2 < bytes.length ? bytes[at] | (bytes[at + 1] << 8) | (bytes[at + 2] << 16) : -1);
  const text = (at: number, length: number) => {
    if (at + length > bytes.length) return "";
    let out = "";
    for (let i = 0; i < length; i++) out += String.fromCharCode(bytes[at + i]);
    return out;
  };
  const size = (width: number, height: number): ImageSize | null =>
    width > 0 && height > 0 && Number.isFinite(width) && Number.isFinite(height) ? { width, height } : null;

  // PNG: the signature, then the header chunk (IHDR) with the width and the height.
  if (u8(0) === 0x89 && text(1, 3) === "PNG" && be32(4) === 0x0d0a1a0a) {
    if (text(12, 4) !== "IHDR") return null;
    return size(be32(16), be32(20));
  }
  // GIF: the screen, and every frame in the file (a decoder makes the picture as big as the
  // largest of them, whatever the screen says).
  if (text(0, 6) === "GIF87a" || text(0, 6) === "GIF89a") return gifSize(bytes);
  // BMP: a file header, then a header whose first number is its own length.
  if (text(0, 2) === "BM") {
    const header = le32(14);
    if (header === 12) return size(le16(18), le16(20));
    if (header >= 40) {
      const width = le32(18);
      const rows = le32(22);
      // A height above 2^31 is a negative number: rows stored from the top.
      return size(width > 0x7fffffff ? -1 : width, rows > 0x7fffffff ? 0x100000000 - rows : rows);
    }
    return null;
  }
  // WebP: a RIFF file whose first chunk says how the picture is coded.
  if (text(0, 4) === "RIFF" && text(8, 4) === "WEBP") {
    const kind = text(12, 4);
    if (kind === "VP8 ") {
      if (bytes.length < 30 || u8(23) !== 0x9d || u8(24) !== 0x01 || u8(25) !== 0x2a) return null;
      return size(le16(26) & 0x3fff, le16(28) & 0x3fff);
    }
    if (kind === "VP8L") {
      if (bytes.length < 25 || u8(20) !== 0x2f) return null;
      const bits = le32(21);
      return size((bits & 0x3fff) + 1, ((bits >>> 14) & 0x3fff) + 1);
    }
    if (kind === "VP8X") return size(le24(24) + 1, le24(27) + 1);
    return null;
  }
  // JPEG: segments that each start with 0xFF and a marker; a "start of frame" holds the size.
  if (u8(0) === 0xff && u8(1) === 0xd8) {
    let at = 2;
    while (at + 3 < bytes.length) {
      if (bytes[at] !== 0xff) return null;
      let marker = bytes[at + 1];
      // Any number of 0xFF may fill before a marker.
      while (marker === 0xff && at + 2 < bytes.length) {
        at += 1;
        marker = bytes[at + 1];
      }
      at += 2;
      // Markers that stand alone: no length follows.
      if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd8)) continue;
      // The end of the picture or the start of the data, and no size before it.
      if (marker === 0xd9 || marker === 0xda) return null;
      const length = be16(at);
      if (length < 2) return null;
      const frame = marker >= 0xc0 && marker <= 0xcf && marker !== 0xc4 && marker !== 0xc8 && marker !== 0xcc;
      if (frame) return length >= 7 ? size(be16(at + 5), be16(at + 3)) : null;
      at += length;
    }
    return null;
  }
  return null;
}

/** What the header of an image file says about it. */
export type ImageHeader = "ok" | "too-big" | "unreadable";

/** Judges a size from a header: inside the side and the pixel limits of the decoder or not. */
export function judgeImageSize(size: ImageSize | null): ImageHeader {
  if (!size) return "unreadable";
  if (size.width > MAX_DECODE_SIDE || size.height > MAX_DECODE_SIDE || size.width * size.height > MAX_DECODE_PIXELS) {
    return "too-big";
  }
  return "ok";
}

/** The first bytes of a file, whichever way this environment can read a Blob. */
async function headOf(blob: Blob, length: number = HEADER_BYTES): Promise<Uint8Array> {
  const part = blob.slice(0, length);
  if (typeof part.arrayBuffer === "function") return new Uint8Array(await part.arrayBuffer());
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(new Uint8Array(reader.result as ArrayBuffer));
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(part);
  });
}

/**
 * Reads the header of an image file and judges its size, before anything decodes the picture.
 * A file whose header cannot be read is not decoded.
 */
export async function checkImageHeader(blob: Blob): Promise<ImageHeader> {
  try {
    let head = await headOf(blob);
    // The frames of a GIF follow one another through the whole file, and each is judged, so the
    // whole file (which is held to its own size limit) is read.
    if (head.length >= 4 && head[0] === 0x47 && head[1] === 0x49 && head[2] === 0x46 && head[3] === 0x38) {
      head = await headOf(blob, MAX_IMAGE_FILE_BYTES);
    }
    return judgeImageSize(imageSizeOf(head));
  } catch {
    return "unreadable";
  }
}

/** Makes the near-white pixels see-through. `threshold` is the lowest level (0 to 255) of the darkest channel that counts as white. */
export function whiteToTransparent(rgba: Uint8ClampedArray, threshold: number): Uint8ClampedArray {
  const out = new Uint8ClampedArray(rgba);
  const limit = Math.min(Math.max(Math.round(threshold), 1), 255);
  for (let i = 0; i + 3 < out.length; i += 4) {
    if (Math.min(out[i], out[i + 1], out[i + 2]) >= limit) out[i + 3] = 0;
  }
  return out;
}

/** True when some pixel of the picture can be seen (is not wholly see-through). */
export function hasInk(rgba: Uint8ClampedArray): boolean {
  for (let i = 3; i < rgba.length; i += 4) if (rgba[i] > 8) return true;
  return false;
}

// ---- the box on the page ---------------------------------------------------------------------

export type Rect = [number, number, number, number];

export interface PageSize {
  width: number;
  height: number;
}

/** Width over height of a picture, or null for one that cannot be placed. */
export function aspectOf(picture: Picture, typedAspect?: number): number | null {
  let aspect: number;
  switch (picture.kind) {
    case "draw":
      aspect = picture.width / picture.height;
      break;
    case "image":
      aspect = picture.width / picture.height;
      break;
    case "type":
      aspect = typedAspect ?? Math.max(1, cleanTypedText(picture.text).length * 0.42);
      break;
  }
  return Number.isFinite(aspect) && aspect > 0 ? aspect : null;
}

/**
 * The box a new picture is given: `DEFAULT_WIDTH` wide (at most half the page), the proportions of
 * the picture, its middle at `centre`, kept on the page.
 */
export function startingRect(aspect: number, centre: Point, page: PageSize): Rect {
  let w = Math.min(DEFAULT_WIDTH, page.width / 2);
  let h = w / aspect;
  if (h > page.height / 2) {
    h = page.height / 2;
    w = h * aspect;
  }
  const left = Math.min(Math.max(0, centre[0] - w / 2), Math.max(0, page.width - w));
  const top = Math.min(Math.max(0, centre[1] - h / 2), Math.max(0, page.height - h));
  return [left, top, left + w, top + h];
}

/**
 * The box of a picture made wider by `dx` points, with its proportions and its top left corner
 * kept, and held to a size and a place the page holds.
 */
export function widenedRect(rect: Rect, dx: number, page: PageSize): Rect {
  const w0 = rect[2] - rect[0];
  const h0 = rect[3] - rect[1];
  if (!(w0 > 0) || !(h0 > 0)) return rect;
  const aspect = w0 / h0;
  const most = Math.max(MIN_SIDE, Math.min(page.width - rect[0], (page.height - rect[1]) * aspect));
  const least = Math.min(most, Math.max(MIN_SIDE, MIN_SIDE * aspect));
  const w = Math.min(Math.max(w0 + dx, least), most);
  return [rect[0], rect[1], rect[0] + w, rect[1] + w / aspect];
}

/** The box of a picture scaled by `factor` around its top left corner. */
export function scaledRect(rect: Rect, factor: number, page: PageSize): Rect {
  return widenedRect(rect, (rect[2] - rect[0]) * (factor - 1), page);
}

/** True when two boxes are the same to within a hundredth of a point. */
export function sameRect(a: Rect, b: Rect): boolean {
  return a.every((v, i) => Math.abs(v - b[i]) < 0.01);
}

// ---- sending and keeping ---------------------------------------------------------------------

/** What the app process takes for a drawn or a typed picture. */
export function bodyOf(picture: DrawnPicture | TypedPicture): SavedBody {
  return picture.kind === "draw"
    ? { kind: "draw", width: picture.width, height: picture.height, line: picture.line, strokes: picture.strokes }
    : { kind: "type", font: picture.font, text: picture.text };
}

/** The picture a kept signature stands for, when it is a drawn or a typed one. */
export function pictureOfSaved(saved: SavedPicture): DrawnPicture | TypedPicture | null {
  if (saved.kind === "draw") {
    return { kind: "draw", width: saved.width, height: saved.height, line: saved.line, strokes: saved.strokes };
  }
  if (saved.kind === "type") {
    const font = SIGNATURE_FONTS.find((f) => f.id === saved.font);
    return font ? { kind: "type", font: font.id, text: saved.text } : null;
  }
  return null;
}

/** Bytes of a base64 text, near enough (the padding is left out). */
export function base64Bytes(text: string): number {
  const pad = text.endsWith("==") ? 2 : text.endsWith("=") ? 1 : 0;
  return Math.floor((text.length * 3) / 4) - pad;
}

/** Bytes to base64 text, in pieces so that a big array does not overflow the arguments of a call. */
export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  const step = 0x8000;
  for (let i = 0; i < bytes.length; i += step) {
    binary += String.fromCharCode(...bytes.subarray(i, i + step));
  }
  return btoa(binary);
}

/** Base64 text to bytes. Throws on text that is not base64. */
export function fromBase64(text: string): Uint8Array {
  const binary = atob(text);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

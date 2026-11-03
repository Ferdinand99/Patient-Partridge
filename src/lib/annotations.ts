// What the window knows about annotations: their shape, the tools and colours, and the small pure
// functions that turn a pointer into a place on a page. Nothing here touches the page or the
// engine. The text of an annotation comes from the file: it is data, to be shown as text only.

export type Rgba = [number, number, number, number];

export type AnnotKind = "highlight" | "underline" | "strikeout" | "note" | "ink" | "stamp" | "signature" | "other";

export type StampName = "approved" | "rejected" | "draft" | "final" | "confidential" | "reviewed";

/** What can be done with an annotation. */
export interface AnnotAbilities {
  editText: boolean;
  recolour: boolean;
  move: boolean;
  remove: boolean;
  restore: boolean;
  /** The file or the document forbids changing it (read-only, locked, or no permission). */
  locked: boolean;
  /** The annotation can be made bigger or smaller (a picture of a signature). Absent means no. */
  resize?: boolean;
}

/** An annotation of a page as the engine reports it. */
export interface Annotation {
  /** Names the annotation for as long as the document is open. */
  id: number;
  kind: AnnotKind;
  can: AnnotAbilities;
  /** `[left, top, right, bottom]` in points from the top left of the page as it is drawn. */
  rect: [number, number, number, number];
  color: Rgba;
  /** The note, or the comment. From the file: show it as text. */
  text: string;
  author: string;
}

export type Tool = "select" | "highlight" | "underline" | "strikeout" | "note" | "pen" | "stamp" | "signature";

export interface ToolInfo {
  id: Tool;
  /** The name a screen reader says, and the tooltip. */
  label: string;
  /** The key that picks the tool when nothing is being typed. */
  key: string;
}

export const TOOLS: readonly ToolInfo[] = [
  { id: "select", label: "Select", key: "v" },
  { id: "highlight", label: "Highlight", key: "h" },
  { id: "underline", label: "Underline", key: "u" },
  { id: "strikeout", label: "Strike out", key: "t" },
  { id: "note", label: "Note", key: "n" },
  { id: "pen", label: "Pen", key: "p" },
  { id: "stamp", label: "Stamp", key: "m" },
  { id: "signature", label: "Signature picture", key: "g" },
];

export function toolForKey(key: string): Tool | null {
  const lower = key.toLowerCase();
  return TOOLS.find((t) => t.key === lower)?.id ?? null;
}

/** The tools that mark the text that is selected. */
export function isMarkupTool(tool: Tool): tool is "highlight" | "underline" | "strikeout" {
  return tool === "highlight" || tool === "underline" || tool === "strikeout";
}

export interface SwatchInfo {
  id: string;
  label: string;
  rgba: Rgba;
}

/** The colours of the toolbar. The first is the default. */
export const PALETTE: readonly SwatchInfo[] = [
  { id: "yellow", label: "Yellow", rgba: [255, 235, 59, 255] },
  { id: "green", label: "Green", rgba: [126, 217, 87, 255] },
  { id: "blue", label: "Blue", rgba: [64, 156, 255, 255] },
  { id: "pink", label: "Pink", rgba: [255, 121, 190, 255] },
  { id: "red", label: "Red", rgba: [230, 40, 40, 255] },
  { id: "black", label: "Black", rgba: [20, 20, 22, 255] },
];

export interface PenWidthInfo {
  id: string;
  label: string;
  /** In points on the page, so the line has the same thickness at every zoom. */
  width: number;
}

export const PEN_WIDTHS: readonly PenWidthInfo[] = [
  { id: "thin", label: "Thin", width: 1.5 },
  { id: "medium", label: "Medium", width: 3 },
  { id: "thick", label: "Thick", width: 6 },
];

export const STAMPS: readonly { id: StampName; label: string }[] = [
  { id: "approved", label: "Approved" },
  { id: "rejected", label: "Rejected" },
  { id: "draft", label: "Draft" },
  { id: "final", label: "Final" },
  { id: "confidential", label: "Confidential" },
  { id: "reviewed", label: "Reviewed" },
];

/** Longest text of a note, in bytes of UTF-8 (the same as the engine's limit). */
export const MAX_NOTE_BYTES = 4096;

const encoder = new TextEncoder();

export function utf8Length(text: string): number {
  return encoder.encode(text).length;
}

/** Cuts `text` to at most `max` bytes of UTF-8, on a character boundary. */
export function clampToBytes(text: string, max: number = MAX_NOTE_BYTES): string {
  if (utf8Length(text) <= max) return text;
  let out = "";
  let used = 0;
  for (const ch of text) {
    const n = utf8Length(ch);
    if (used + n > max) break;
    out += ch;
    used += n;
  }
  return out;
}

export function rgbaCss(c: Rgba): string {
  return `rgb(${c[0]} ${c[1]} ${c[2]})`;
}

export function sameColor(a: Rgba, b: Rgba): boolean {
  return a[0] === b[0] && a[1] === b[1] && a[2] === b[2];
}

/** The swatch of the palette that a colour is, or null when it is none of them. */
export function swatchOf(color: Rgba): SwatchInfo | null {
  return PALETTE.find((s) => sameColor(s.rgba, color)) ?? null;
}

const KIND_NAMES: Record<AnnotKind, string> = {
  highlight: "Highlight",
  underline: "Underline",
  strikeout: "Strike-out",
  note: "Note",
  ink: "Drawing",
  stamp: "Stamp",
  signature: "Signature picture",
  other: "Annotation",
};

export function kindName(kind: AnnotKind): string {
  return KIND_NAMES[kind];
}

const SNIPPET_CHARS = 80;

/** A short piece of the text of an annotation, one line, for a label. */
export function snippet(text: string): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const chars = Array.from(flat);
  return chars.length > SNIPPET_CHARS ? `${chars.slice(0, SNIPPET_CHARS).join("")}…` : flat;
}

/** What a screen reader says about an annotation. */
export function describeAnnotation(a: Annotation, page: number): string {
  const text = snippet(a.text);
  const author = snippet(a.author);
  const parts = [kindName(a.kind)];
  if (author) parts.push(`by ${author}`);
  const head = `${parts.join(" ")}, page ${page + 1}${a.can.locked ? ", locked" : ""}`;
  return text ? `${head}: ${text}` : head;
}

/**
 * The part of a box that lies on its page, as `[left, top, right, bottom]` in points. A box
 * far outside the page (the file can say anything) is cut to the page, so it cannot cover other
 * pages or make the area that scrolls bigger.
 */
export function onPage(rect: [number, number, number, number], page: { width: number; height: number }): [number, number, number, number] {
  const cut = (v: number, limit: number) => (Number.isFinite(v) ? Math.min(Math.max(v, 0), limit) : 0);
  const l = cut(rect[0], page.width);
  const t = cut(rect[1], page.height);
  return [l, t, Math.max(cut(rect[2], page.width), l), Math.max(cut(rect[3], page.height), t)];
}

/** True when a box covers so much of its page that it would hide the text and the links under it. */
export function coversPage(rect: [number, number, number, number], page: { width: number; height: number }): boolean {
  const area = Math.max(rect[2] - rect[0], 0) * Math.max(rect[3] - rect[1], 0);
  return page.width > 0 && page.height > 0 && area > 0.35 * page.width * page.height;
}

/** A point on a page in points from its top left, from a point on the screen. */
export function clientToPage(
  box: { left: number; top: number; width: number; height: number },
  page: { width: number; height: number },
  clientX: number,
  clientY: number,
): [number, number] {
  if (box.width <= 0 || box.height <= 0) return [0, 0];
  return [((clientX - box.left) / box.width) * page.width, ((clientY - box.top) / box.height) * page.height];
}

/** A number of points as a percentage of a side of the page, for placing a box. */
export function percentOf(value: number, side: number): string {
  return `${((side > 0 ? value / side : 0) * 100).toFixed(3)}%`;
}

/** Most points one drawing keeps (the engine's limit is the same). */
export const MAX_STROKE_POINTS = 20_000;

/**
 * Thins out the points of a stroke: a point closer than `minDistance` points to the last one kept
 * is dropped, and the first and the last point always stay. A pen that rests on the paper does not
 * pile up thousands of points, and a stroke is never longer than the engine accepts.
 */
export function simplifyStroke(points: [number, number][], minDistance = 0.6): [number, number][] {
  if (points.length <= 2) return points.slice();
  const out: [number, number][] = [points[0]];
  let last = points[0];
  for (let i = 1; i < points.length - 1; i++) {
    const p = points[i];
    if (Math.hypot(p[0] - last[0], p[1] - last[1]) >= minDistance) {
      out.push(p);
      last = p;
    }
  }
  out.push(points[points.length - 1]);
  if (out.length <= MAX_STROKE_POINTS) return out;
  // Too many even so: keep an even share, and the last point.
  const step = Math.ceil(out.length / (MAX_STROKE_POINTS - 1));
  const thin = out.filter((_, i) => i % step === 0);
  thin.push(out[out.length - 1]);
  return thin.slice(0, MAX_STROKE_POINTS);
}

/** Size of a stamp in points. The same formula as the engine's, so a stamp can be centred. */
export function stampSize(stamp: StampName): { width: number; height: number } {
  const label = STAMPS.find((s) => s.id === stamp)?.label ?? stamp;
  return { width: label.length * 26 * 0.72 + 18, height: 26 + 18 };
}

/** Keeps a box of a given size on the page: the top left corner moved the least that is needed. */
export function keepOnPage(
  x: number,
  y: number,
  size: { width: number; height: number },
  page: { width: number; height: number },
): [number, number] {
  return [
    Math.min(Math.max(0, x), Math.max(0, page.width - size.width)),
    Math.min(Math.max(0, y), Math.max(0, page.height - size.height)),
  ];
}

/** The size of a sticky note's icon on the page, in points (the engine's size). */
export const NOTE_SIZE = { width: 20, height: 20 };

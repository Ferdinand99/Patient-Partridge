// The logic of the page tools that needs no page and no engine: which pages are selected, what
// order a drag makes, where a crop box is, and the words that say what a tool keeps and loses.
// Pure functions and small classes, so every rule can be tested alone.

/** Most pages the page tools work on (the engine refuses more; see MAX_EDIT_PAGES in the protocol). */
export const MAX_EDIT_PAGES = 10_000;

/** The smallest side a crop may leave, in points (MIN_CROP_SIDE in the protocol). */
export const MIN_CROP_SIDE = 8;

/** Sizes offered for a blank page, in points. */
export const BLANK_SIZES = [
  { id: "letter", label: "Letter (8.5 x 11 in)", width: 612, height: 792 },
  { id: "a4", label: "A4 (210 x 297 mm)", width: 595, height: 842 },
  { id: "legal", label: "Legal (8.5 x 14 in)", width: 612, height: 1008 },
  { id: "a5", label: "A5 (148 x 210 mm)", width: 420, height: 595 },
] as const;

export interface Mods {
  shift: boolean;
  ctrl: boolean;
}

/**
 * The pages that are picked in the page panel, the one the keyboard is on (the cursor), and the one
 * a range starts from (the anchor). Page numbers count from 0.
 */
export class PageSelection {
  private picked = new Set<number>();
  private anchorAt = -1;
  private cursorAt = -1;

  constructor(private count = 0) {}

  /** The number of pages changed: what is past the end is let go of. */
  setCount(count: number): void {
    this.count = count;
    for (const page of [...this.picked]) if (page >= count) this.picked.delete(page);
    if (this.cursorAt >= count) this.cursorAt = count - 1;
    if (this.anchorAt >= count) this.anchorAt = count - 1;
  }

  get size(): number {
    return this.picked.size;
  }

  get cursor(): number {
    return this.cursorAt;
  }

  has(page: number): boolean {
    return this.picked.has(page);
  }

  /** The picked pages in page order. */
  toArray(): number[] {
    return [...this.picked].sort((a, b) => a - b);
  }

  clear(): void {
    this.picked.clear();
    this.anchorAt = -1;
    this.cursorAt = -1;
  }

  /** Picks one page and nothing else. */
  only(page: number): void {
    if (page < 0 || page >= this.count) return;
    this.picked = new Set([page]);
    this.anchorAt = page;
    this.cursorAt = page;
  }

  /** Picks a page, or lets it go when it is picked, keeping the others. */
  toggle(page: number): void {
    if (page < 0 || page >= this.count) return;
    if (this.picked.has(page)) this.picked.delete(page);
    else this.picked.add(page);
    this.anchorAt = page;
    this.cursorAt = page;
  }

  /** Picks the pages from the anchor to `page` (and keeps the others when `keep`). */
  rangeTo(page: number, keep = false): void {
    if (page < 0 || page >= this.count) return;
    const from = this.anchorAt < 0 ? page : this.anchorAt;
    const lo = Math.min(from, page);
    const hi = Math.max(from, page);
    if (!keep) this.picked = new Set();
    for (let i = lo; i <= hi; i++) this.picked.add(i);
    this.cursorAt = page;
    if (this.anchorAt < 0) this.anchorAt = page;
  }

  all(): void {
    this.picked = new Set(Array.from({ length: this.count }, (_, i) => i));
    this.cursorAt = Math.max(0, this.cursorAt);
    if (this.anchorAt < 0) this.anchorAt = 0;
  }

  /** A press or a click on a page. */
  click(page: number, mods: Mods): void {
    if (mods.shift) this.rangeTo(page, mods.ctrl);
    else if (mods.ctrl) this.toggle(page);
    else this.only(page);
  }

  /** An arrow key: the cursor moves, and with Shift the range from the anchor follows it. */
  step(by: number, extend: boolean): void {
    if (this.count === 0) return;
    const from = this.cursorAt < 0 ? (by > 0 ? -1 : this.count) : this.cursorAt;
    const to = Math.min(Math.max(from + by, 0), this.count - 1);
    if (extend) this.rangeTo(to);
    else this.only(to);
  }

  /** Moves the cursor only (Ctrl and an arrow), so that a page can be toggled with Space. */
  moveCursor(by: number): void {
    if (this.count === 0) return;
    const from = this.cursorAt < 0 ? 0 : this.cursorAt;
    this.cursorAt = Math.min(Math.max(from + by, 0), this.count - 1);
  }

  /** The cursor goes to the first or the last page. */
  edge(last: boolean, extend: boolean): void {
    if (this.count === 0) return;
    const to = last ? this.count - 1 : 0;
    if (extend) this.rangeTo(to);
    else this.only(to);
  }
}

/**
 * The order that moving the picked pages to a place makes, as the engine wants it: for every place in
 * the new document, the page that goes there. `insertion` is where the pages go: before page
 * `insertion` of the document as it is now (0 is the front, `count` is the end). The picked pages
 * keep their order among themselves.
 */
export function orderAfterMove(count: number, picked: number[], insertion: number): number[] {
  const set = new Set(picked);
  const moving = [...set].filter((p) => p >= 0 && p < count).sort((a, b) => a - b);
  const stay: number[] = [];
  for (let i = 0; i < count; i++) if (!set.has(i)) stay.push(i);
  // How many of the pages that stay come before the insertion place.
  const before = Math.min(Math.max(insertion, 0), count);
  let leading = 0;
  for (let i = 0; i < before && i < count; i++) if (!set.has(i)) leading++;
  return [...stay.slice(0, leading), ...moving, ...stay.slice(leading)];
}

/** True when an order leaves every page where it is. */
export function isIdentity(order: number[]): boolean {
  return order.every((page, place) => page === place);
}

/** Where the picked pages are after a move: the new place of each. */
export function placesAfterMove(order: number[], picked: number[]): number[] {
  const set = new Set(picked);
  const places: number[] = [];
  order.forEach((page, place) => {
    if (set.has(page)) places.push(place);
  });
  return places;
}

/** The place Alt and an arrow key moves the picked pages to: one page up or down, past the pages in the way. */
export function insertionForStep(count: number, picked: number[], by: -1 | 1): number {
  if (picked.length === 0) return 0;
  const lo = Math.min(...picked);
  const hi = Math.max(...picked);
  if (by < 0) return Math.max(0, lo - 1);
  return Math.min(count, hi + 2);
}

/** Page numbers as people write them: "3", "3-5, 9". `pages` count from 0 and are shown from 1. */
export function describePages(pages: number[]): string {
  const sorted = [...new Set(pages)].sort((a, b) => a - b);
  const parts: string[] = [];
  for (let i = 0; i < sorted.length; ) {
    let j = i;
    while (j + 1 < sorted.length && sorted[j + 1] === sorted[j] + 1) j++;
    parts.push(j === i ? String(sorted[i] + 1) : j - i === 1 ? `${sorted[i] + 1}, ${sorted[j] + 1}` : `${sorted[i] + 1}-${sorted[j] + 1}`);
    i = j + 1;
  }
  return parts.join(", ");
}

/** "1 page" or "3 pages". */
export function pagesWord(count: number): string {
  return count === 1 ? "1 page" : `${count.toLocaleString("en-US")} pages`;
}

// ---- crop ------------------------------------------------------------------------------------

/** Margins in points on the page as it is drawn: how much each edge is cut. */
export interface Margins {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** A box in any unit: the edges. */
export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export function marginsToArray(m: Margins): [number, number, number, number] {
  return [m.left, m.top, m.right, m.bottom];
}

/** Keeps margins from being negative or leaving less than the smallest side. */
export function clampMargins(m: Margins, width: number, height: number): Margins {
  const num = (v: number) => (Number.isFinite(v) ? v : 0);
  const left = Math.min(Math.max(0, num(m.left)), Math.max(0, width - MIN_CROP_SIDE));
  const right = Math.min(Math.max(0, num(m.right)), Math.max(0, width - MIN_CROP_SIDE - left));
  const top = Math.min(Math.max(0, num(m.top)), Math.max(0, height - MIN_CROP_SIDE));
  const bottom = Math.min(Math.max(0, num(m.bottom)), Math.max(0, height - MIN_CROP_SIDE - top));
  return { left, top, right, bottom };
}

/** The margins that a box inside a page of `width` by `height` (any unit) leaves, in that unit. */
export function marginsOfBox(box: Box, width: number, height: number): Margins {
  return {
    left: box.left,
    top: box.top,
    right: width - box.right,
    bottom: height - box.bottom,
  };
}

export function boxOfMargins(m: Margins, width: number, height: number): Box {
  return { left: m.left, top: m.top, right: width - m.right, bottom: height - m.bottom };
}

export type Handle = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw" | "move";

/**
 * Drags one handle (or, with "move", the whole box) of a crop box by `dx`, `dy`, keeping the box
 * inside `width` by `height` and at least `min` on each side. All in the same unit.
 */
export function dragBox(
  box: Box,
  handle: Handle,
  dx: number,
  dy: number,
  width: number,
  height: number,
  min: number,
): Box {
  let { left, top, right, bottom } = box;
  if (handle === "move") {
    const w = right - left;
    const h = bottom - top;
    left = Math.min(Math.max(0, left + dx), Math.max(0, width - w));
    top = Math.min(Math.max(0, top + dy), Math.max(0, height - h));
    return { left, top, right: left + w, bottom: top + h };
  }
  if (handle.includes("w")) left = Math.min(Math.max(0, left + dx), right - min);
  if (handle.includes("e")) right = Math.max(Math.min(width, right + dx), left + min);
  if (handle.includes("n")) top = Math.min(Math.max(0, top + dy), bottom - min);
  if (handle.includes("s")) bottom = Math.max(Math.min(height, bottom + dy), top + min);
  return { left, top, right, bottom };
}

// ---- what each tool keeps and loses ------------------------------------------------------------

/**
 * The plain words about what the tools do to a document, shown by the app and copied into the
 * README, so that the two say the same.
 */
export const TOOL_NOTES = {
  crop:
    "Cropping hides the edges of a page. It does not remove anything: the text, pictures and everything else outside the box are still in the file, and a program that ignores the crop box shows them. To remove content for good, delete the page.",
  delete:
    "Deleting a page removes it from the file, with its text, pictures, fonts and notes, unless something that stays refers to them. Bookmarks that led to it stay in the list and lead nowhere.",
  deleteForms:
    "PDFium keeps the list of the form's fields, so form fields that were on a deleted page stay in the file. The text typed into text fields was cleared; other kinds of fields (check boxes, choices) and fields grouped under a parent field may keep what they hold.",
  move:
    "Moving a page puts a copy of it in its new place. Links to and from a moved page and its bookmark stop working, and a form field on it is no longer part of the form's field tree. The pages that did not have to move are not touched.",
  insert:
    "Pages from another file are copied with what they show, their notes and their links to pages that come before them in that file. Bookmarks, links to later pages, the structure of tagged documents, form fields (they stay on the page as they look, but cannot be filled in) and signatures do not come with them.",
  extract:
    "The new file holds the pages you picked, with what they show and their notes. Bookmarks, links to pages that are not in it, form fields (they stay as they look, but cannot be filled in) and signatures are not carried over, and the new file has no password.",
  protectedSource:
    "is protected by a password. Its pages join this document without that protection: they are as readable in this document as the rest of it. If this document has a password, they are protected by that.",
  signedSource:
    "is signed. Its pages come without the signature, and a signature cannot be kept when pages are copied.",
  unprotectedCopy:
    "is protected by a password. The new file will not be: anyone who gets it can read it.",
  signedCopy:
    "is signed. The new file will not have the signature, and a signature cannot be kept when pages are copied.",
  undoGone:
    "This change is too old to take back, or the document is too big for the app to keep a copy of it.",
} as const;

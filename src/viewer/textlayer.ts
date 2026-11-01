// Text of a page, selection, and where things are on the page. Pure functions: no DOM.
//
// The engine sends the text of a page as one string plus a box for every character, in points from
// the top left of the page as it is drawn. Nothing here parses a PDF: it only works with that
// answer. PDF text is data; it is never put in the page as markup, and the selection is drawn as
// plain boxes.

export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** The text of one page: one entry in `chars` and four numbers in `boxes` for each character. */
export interface PageText {
  chars: string[];
  /** left, top, right, bottom of each character, one after the other. */
  boxes: number[];
}

/** Turns the answer of the engine into a `PageText`, or null when the two parts do not agree. */
export function parsePageText(text: string, boxes: number[]): PageText | null {
  const chars = Array.from(text);
  if (boxes.length !== chars.length * 4) return null;
  for (const v of boxes) if (!Number.isFinite(v)) return null;
  return { chars, boxes };
}

export function boxOf(page: PageText, index: number): Rect {
  const b = page.boxes;
  const i = index * 4;
  return { left: b[i], top: b[i + 1], right: b[i + 2], bottom: b[i + 3] };
}

/** True when the character has a place on the page (line breaks and the like have none). */
export function hasBox(page: PageText, index: number): boolean {
  const b = page.boxes;
  const i = index * 4;
  return b[i + 2] > b[i] && b[i + 3] > b[i + 1];
}

/**
 * The place between two characters that is closest to a point, as a number from 0 (before the
 * first character) to the number of characters (after the last). The line nearest to the point
 * wins first, then the character nearest along the line, so a point in the margin or between
 * lines still gives a sensible answer.
 */
export function offsetAt(page: PageText, x: number, y: number): number {
  let best = -1;
  let bestDy = Infinity;
  let bestDx = Infinity;
  for (let i = 0; i < page.chars.length; i++) {
    if (!hasBox(page, i)) continue;
    const r = boxOf(page, i);
    const dx = Math.max(r.left - x, 0, x - r.right);
    const dy = Math.max(r.top - y, 0, y - r.bottom);
    if (dy < bestDy - 0.5 || (Math.abs(dy - bestDy) <= 0.5 && dx < bestDx)) {
      best = i;
      bestDy = dy;
      bestDx = dx;
    }
  }
  if (best < 0) return 0;
  const r = boxOf(page, best);
  return x >= (r.left + r.right) / 2 ? best + 1 : best;
}

/**
 * The boxes to draw for the characters from `start` to `end` (not including `end`): one box for
 * each run of characters on a line, so a selection of a paragraph is a few wide boxes and not
 * thousands of small ones.
 */
export function lineRects(page: PageText, start: number, end: number): Rect[] {
  const from = Math.max(0, Math.min(start, page.chars.length));
  const to = Math.max(from, Math.min(end, page.chars.length));
  const out: Rect[] = [];
  let group: Rect | null = null;
  let lastLeft = -Infinity;
  for (let i = from; i < to; i++) {
    if (!hasBox(page, i)) continue;
    const r = boxOf(page, i);
    if (group) {
      const height = group.bottom - group.top;
      const centre = (r.top + r.bottom) / 2;
      const sameLine = centre > group.top - height * 0.25 && centre < group.bottom + height * 0.25;
      // A character that starts well to the left of the one before it begins a new line or column.
      const jumpedBack = r.left < lastLeft - (r.right - r.left) * 2 && r.left < group.right - height;
      if (sameLine && !jumpedBack) {
        group.left = Math.min(group.left, r.left);
        group.top = Math.min(group.top, r.top);
        group.right = Math.max(group.right, r.right);
        group.bottom = Math.max(group.bottom, r.bottom);
        lastLeft = r.left;
        continue;
      }
      out.push(group);
    }
    group = { ...r };
    lastLeft = r.left;
  }
  if (group) out.push(group);
  return out;
}

export function textInRange(page: PageText, start: number, end: number): string {
  return page.chars.slice(Math.max(0, start), Math.max(0, end)).join("");
}

type CharClass = "space" | "word" | "other";

function classOf(ch: string): CharClass {
  if (/^\s$/u.test(ch)) return "space";
  if (/^[\p{L}\p{N}\p{M}_]$/u.test(ch)) return "word";
  return "other";
}

/** The word at a place (a double click): a run of letters and digits, of spaces, or one mark. */
export function wordRange(page: PageText, offset: number): [number, number] {
  const n = page.chars.length;
  if (n === 0) return [0, 0];
  const at = Math.min(Math.max(offset, 0), n - 1);
  const kind = classOf(page.chars[at]);
  if (kind === "other") return [at, at + 1];
  let start = at;
  let end = at + 1;
  while (start > 0 && classOf(page.chars[start - 1]) === kind && page.chars[start - 1] !== "\n") start--;
  while (end < n && classOf(page.chars[end]) === kind && page.chars[end] !== "\n") end++;
  return [start, end];
}

/** The line at a place (a triple click): from the line break before it to the one after it. */
export function lineRange(page: PageText, offset: number): [number, number] {
  const n = page.chars.length;
  if (n === 0) return [0, 0];
  const at = Math.min(Math.max(offset, 0), n - 1);
  let start = at;
  let end = at;
  while (start > 0 && page.chars[start - 1] !== "\n") start--;
  while (end < n && page.chars[end] !== "\n") end++;
  return [start, end];
}

// ---- selection across pages ------------------------------------------------

/** A place in the document: a page and a place between two characters on it. */
export interface Caret {
  page: number;
  offset: number;
}

export interface Selection {
  anchor: Caret;
  focus: Caret;
}

function before(a: Caret, b: Caret): boolean {
  return a.page < b.page || (a.page === b.page && a.offset <= b.offset);
}

/** The selection with its two ends in reading order. */
export function orderSelection(sel: Selection): { start: Caret; end: Caret } {
  return before(sel.anchor, sel.focus)
    ? { start: sel.anchor, end: sel.focus }
    : { start: sel.focus, end: sel.anchor };
}

export function isEmptySelection(sel: Selection | null): boolean {
  return sel === null || (sel.anchor.page === sel.focus.page && sel.anchor.offset === sel.focus.offset);
}

/**
 * The characters of one page that are selected, from the first to just past the last, or null when
 * the page is outside the selection. A page in the middle of a selection is selected whole, which
 * is `[0, Infinity]`: the caller cuts that to the length of the page.
 */
export function rangeOnPage(sel: Selection, page: number): [number, number] | null {
  const { start, end } = orderSelection(sel);
  if (page < start.page || page > end.page) return null;
  const from = page === start.page ? start.offset : 0;
  const to = page === end.page ? end.offset : Infinity;
  return to > from ? [from, to] : null;
}

/** The text of a selection, joined from the pages that are loaded; `missing` lists the others. */
export function selectionText(
  sel: Selection,
  pageText: (page: number) => PageText | undefined,
): { text: string; missing: number[] } {
  const { start, end } = orderSelection(sel);
  const parts: string[] = [];
  const missing: number[] = [];
  for (let page = start.page; page <= end.page; page++) {
    const text = pageText(page);
    if (!text) {
      missing.push(page);
      continue;
    }
    const range = rangeOnPage(sel, page);
    if (!range) continue;
    const piece = textInRange(text, range[0], Math.min(range[1], text.chars.length));
    if (piece.length > 0) parts.push(piece);
  }
  return { text: parts.join("\n"), missing };
}

// ---- keeping the text of pages -----------------------------------------------

/** Most characters of page text kept at once. About 100 MB of strings and numbers at the limit. */
export const MAX_KEPT_CHARS = 3_000_000;

/**
 * The text of the pages asked for so far. Each page is asked from the engine once; pages that are
 * not needed any more are dropped when the total grows too large, oldest first.
 */
export class TextStore {
  private readonly pages = new Map<number, PageText>();
  private readonly pending = new Map<number, Promise<PageText | null>>();
  private chars = 0;

  constructor(
    private readonly fetch: (page: number) => Promise<PageText | null>,
    /** Pages that must not be dropped, such as those inside the selection. */
    private readonly keep: (page: number) => boolean = () => false,
    private readonly limit = MAX_KEPT_CHARS,
  ) {}

  get(page: number): PageText | undefined {
    return this.pages.get(page);
  }

  get size(): number {
    return this.pages.size;
  }

  /** Asks for a page unless it is here or on its way. Resolves with null when it cannot be read. */
  load(page: number): Promise<PageText | null> {
    const have = this.pages.get(page);
    if (have) return Promise.resolve(have);
    const waiting = this.pending.get(page);
    if (waiting) return waiting;
    const request = this.fetch(page)
      .catch(() => null)
      .then((text) => {
        this.pending.delete(page);
        if (text) {
          this.pages.set(page, text);
          this.chars += text.chars.length;
          this.trim(page);
        }
        return text;
      });
    this.pending.set(page, request);
    return request;
  }

  /** Drops every page that `keep` does not name. */
  retain(keep: (page: number) => boolean): void {
    for (const [page, text] of this.pages) {
      if (keep(page)) continue;
      this.pages.delete(page);
      this.chars -= text.chars.length;
    }
  }

  clear(): void {
    this.pages.clear();
    this.pending.clear();
    this.chars = 0;
  }

  private trim(justLoaded: number): void {
    if (this.chars <= this.limit) return;
    for (const [page, text] of this.pages) {
      if (this.chars <= this.limit) break;
      if (page === justLoaded || this.keep(page)) continue;
      this.pages.delete(page);
      this.chars -= text.chars.length;
    }
  }
}

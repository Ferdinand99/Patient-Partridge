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
  const rightHalf = x >= (r.left + r.right) / 2;
  // In left-to-right text the right half of a character is after it. In right-to-left text the
  // characters are in reading order but run from the right, so the right half is before it.
  return rightHalf !== (direction(page, best) < 0) ? best + 1 : best;
}

/** How many characters on each side are looked at to tell which way the text runs. */
const DIRECTION_REACH = 4;

/**
 * Which way the text runs around a character: 1 for left to right, -1 for right to left, 0 when it
 * cannot be told (a lone character). The engine gives characters in reading order, so the next
 * character on the same line is to the right in left-to-right text and to the left in right-to-left
 * text. A character at the edge of a run may have neighbours that disagree; then the nearer wins.
 */
function direction(page: PageText, index: number): -1 | 0 | 1 {
  const here = boxOf(page, index);
  const centre = (here.left + here.right) / 2;
  const middle = (here.top + here.bottom) / 2;
  const width = here.right - here.left;
  const height = here.bottom - here.top;

  const look = (step: 1 | -1): { sign: -1 | 1; distance: number } | null => {
    let seen = 0;
    for (let j = index + step; j >= 0 && j < page.chars.length && seen < DIRECTION_REACH; j += step) {
      if (!hasBox(page, j)) continue;
      seen++;
      const r = boxOf(page, j);
      // A character on another line says nothing about this one.
      if (Math.abs((r.top + r.bottom) / 2 - middle) > height * 0.5) return null;
      const dx = (r.left + r.right) / 2 - centre;
      // A mark drawn over this character (a vowel sign, say) does not show the direction.
      if (Math.abs(dx) < 0.3 * Math.min(width, r.right - r.left)) continue;
      return { sign: dx * step > 0 ? 1 : -1, distance: Math.abs(dx) };
    }
    return null;
  };

  const next = look(1);
  const previous = look(-1);
  if (next && previous) {
    if (next.sign === previous.sign) return next.sign;
    return previous.distance < next.distance ? previous.sign : next.sign;
  }
  return (next ?? previous)?.sign ?? 0;
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

/** Most characters of page text kept at once for one document. About 60 MB at the limit. */
export const MAX_KEPT_CHARS = 1_500_000;

/**
 * The text of the pages asked for so far. Each page is asked from the engine once. When the total
 * grows past the limit, pages that are not needed any more are dropped first, oldest first. Pages
 * that `keep` names (the ones on screen) stay, but the limit is hard: a page that would take the
 * total past it, with everything that can be dropped gone, is not kept at all. It is `refused`,
 * the caller gets its text for the moment, and the page has no text layer until it is let in again.
 */
export class TextStore {
  private readonly pages = new Map<number, PageText>();
  private readonly pending = new Map<number, Promise<PageText | null>>();
  private readonly refusedPages = new Set<number>();
  /** Pages that could not be read: nobody asks for them again until they are allowed to. */
  private readonly failedPages = new Set<number>();
  private chars = 0;

  constructor(
    private readonly fetch: (page: number) => Promise<PageText | null>,
    /** Pages that are dropped last, such as those on screen. */
    private readonly keep: (page: number) => boolean = () => false,
    private readonly limit = MAX_KEPT_CHARS,
  ) {}

  get(page: number): PageText | undefined {
    return this.pages.get(page);
  }

  /** True when the page was too much for the limit the last time it was loaded. */
  refused(page: number): boolean {
    return this.refusedPages.has(page);
  }

  /** True when the page could not be read the last time it was loaded. */
  failed(page: number): boolean {
    return this.failedPages.has(page);
  }

  /** True when the page has no text and asking again now is no use: it was refused or is unreadable. */
  unavailable(page: number): boolean {
    return this.refusedPages.has(page) || this.failedPages.has(page);
  }

  /** Lets a refused or unreadable page try again, such as when it comes on screen again. */
  allow(page: number): void {
    this.refusedPages.delete(page);
    this.failedPages.delete(page);
  }

  get size(): number {
    return this.pages.size;
  }

  /** Characters of text kept now. */
  get keptChars(): number {
    return this.chars;
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
          this.failedPages.delete(page);
          this.trim(page);
        } else {
          this.failedPages.add(page);
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
    this.refusedPages.clear();
    this.failedPages.clear();
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
    if (this.chars > this.limit) {
      const text = this.pages.get(justLoaded);
      if (text) {
        this.pages.delete(justLoaded);
        this.chars -= text.chars.length;
        this.refusedPages.add(justLoaded);
      }
    }
  }
}

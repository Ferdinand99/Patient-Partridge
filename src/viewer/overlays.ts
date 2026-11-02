// What sits on top of the drawn pages: the selection, the search highlights and the links.
//
// The engine sends the text of a page, and the links, as plain data. Here that data is turned into
// boxes placed over the page with percentages of the page size, so they stay right at any zoom
// without being placed again. PDF text and addresses are data: they are never put in the page as
// markup, only as text content, and nothing here follows a link by itself.

import { describeLink } from "../lib/linkPolicy";
import { scriptsIn, type TextScript } from "../lib/scripts";
import type { PageSizePt } from "./layout";
import type { SearchMatch } from "./search";
import {
  MAX_KEPT_CHARS,
  TextStore,
  isEmptySelection,
  lineRange,
  lineRects,
  offsetAt,
  orderSelection,
  parsePageText,
  rangeOnPage,
  selectionText,
  textInRange,
  wordRange,
  type PageText,
  type Rect,
  type Selection,
} from "./textlayer";

/** A link on a page: a box in points from the top left, and where it leads. */
export interface PageLinkInfo {
  rect: [number, number, number, number];
  uri: string | null;
  page: number | null;
}

export interface OverlayApi {
  getPageText?(doc: number, page: number): Promise<{ text: string; rects: number[] }>;
  getPageLinks?(doc: number, page: number): Promise<PageLinkInfo[]>;
}

/** What the overlays need to know about the view they sit in. */
export interface OverlayHost {
  /** The scrolling element: pointer events are listened to here. */
  element: HTMLElement;
  /** The element of a page that has one right now. */
  pageElement(page: number): HTMLElement | null;
  /** Pages that have an element right now. */
  mountedPages(): number[];
  /** Scrolls so that a box of a page, in points from its top left, is in view. */
  scrollToRect(page: number, rect: Rect): void;
}

export interface OverlayEvents {
  /** A link was pressed. Nothing has been opened: the receiver decides. */
  onLink(link: PageLinkInfo, page: number): void;
  onSelectionChange?(hasSelection: boolean): void;
  /** The reader let go of the selection (or it was made whole): its text is final for now. */
  onSelectionSettled?(): void;
  /**
   * A page has more text or more links than the document may keep (see `MAX_KEPT_CHARS`), so it has
   * no text layer for now. Called once for the document. The page itself is drawn as usual.
   */
  onTextLimit?(): void;
  /**
   * The text of a page uses these scripts (see `scriptsIn`). Called each time a page is read,
   * with the scripts of that page; the receiver decides what it means.
   */
  onScripts?(scripts: TextScript[]): void;
}

/** Limits on the text a document keeps in the window. */
export interface OverlayLimits {
  /**
   * Most characters of text loaded ahead after Select all, so that a short Copy needs no waiting
   * and the Copy of the system has something to copy. A longer selection is fetched when it is
   * copied.
   */
  preloadChars: number;
  /** Most characters one Copy gathers. About 40 MB of text at the limit. */
  copyChars: number;
  /**
   * Most characters of page text kept for the document, the pages on screen included. A page that
   * would take the total past it has no text layer until it comes on screen again.
   */
  keptChars: number;
}

export const DEFAULT_LIMITS: OverlayLimits = {
  preloadChars: 600_000,
  copyChars: 20_000_000,
  keptChars: MAX_KEPT_CHARS,
};

/**
 * Most characters of web addresses kept for the links of the pages on screen. A page may hold 500
 * links of 2,048 characters, and a PDF can make hundreds of pages share a few pixels of screen.
 */
export const MAX_LINK_CHARS = 1_000_000;

/** Most boxes drawn for the selection, or for the matches of a search, on one page. Copying is not affected. */
const MAX_SELECTION_BOXES = 5_000;

/**
 * Most boxes (selection and search matches) drawn for all the pages of a document together, and
 * most link buttons. The viewer mounts a fixed number of pages at most (`MAX_MOUNTED_PAGES`), but a
 * crafted file can still give each of them thousands of boxes and 500 links. About 25,000 elements
 * at the limit, and 5,000 more for the match of a search that is picked out. What does not fit is
 * not drawn; copying and searching are not affected.
 */
export const MAX_OVERLAY_BOXES = 20_000;
export const MAX_LINK_BUTTONS = 5_000;

/** A count that all the pages of a document share: each page asks for what it wants to draw. */
class Budget {
  private readonly used = new Map<string, number>();
  private sum = 0;

  constructor(private readonly limit: number) {}

  /** How many of `wanted` the owner may draw now. Replaces what the owner was allowed before. */
  claim(owner: string, wanted: number): number {
    const had = this.used.get(owner) ?? 0;
    const allowed = Math.max(0, Math.min(wanted, this.limit - (this.sum - had)));
    this.sum += allowed - had;
    if (allowed === 0) this.used.delete(owner);
    else this.used.set(owner, allowed);
    return allowed;
  }

  release(owner: string): void {
    this.claim(owner, 0);
  }
}

/** The engine cuts the text of one page at this many characters (see MAX_PAGE_CHARS there). */
const ENGINE_PAGE_CHARS = 100_000;

/**
 * True when the engine cut the text of the page and the selection runs to the end of what it gave:
 * only then is the end of the page missing from a copy. A word picked on a long page is whole.
 */
function reachesCutEnd(sel: Selection, page: number, text: PageText | undefined): boolean {
  if (!text || text.chars.length < ENGINE_PAGE_CHARS) return false;
  const range = rangeOnPage(sel, page);
  return range !== null && range[1] >= text.chars.length;
}

/** What a Copy of the selection gathered, and what it could not. */
export interface CopyResult {
  text: string;
  /** Pages of the selection that were read and are in the text. */
  pages: number;
  /** Pages the selection covers. */
  total: number;
  /** Pages that could not be read. */
  failed: number[];
  /** True when the copy stopped at the limit and the pages after the last one are not in the text. */
  truncated: boolean;
  /** Pages as long as the engine allows: the engine cuts such a page, so its end is missing. */
  cut: number;
  /**
   * How the copy ended. "busy": another copy of this document is still running, so nothing was
   * done. "cancelled": the document was closed on the way, so there is nothing to write. "changed":
   * the selection was changed on the way, so the text is not what is selected now. Only "done"
   * has text that is meant to be written.
   */
  status: "done" | "busy" | "cancelled" | "changed";
}
/** How far the pointer must move before a press becomes a drag, in pixels. */
const DRAG_DISTANCE = 4;
/** How close to the edge of the view the pointer must be to scroll it while dragging, in pixels. */
const EDGE = 28;
/** Presses closer together than this, in milliseconds and pixels, count as a double or triple click. */
const MULTI_CLICK_MS = 500;
const MULTI_CLICK_PX = 5;

function percent(value: number, of: number): string {
  return `${((of > 0 ? value / of : 0) * 100).toFixed(3)}%`;
}

export class PageOverlays {
  private readonly texts: TextStore;
  private readonly overlays = new Map<number, HTMLElement>();
  private readonly links = new Map<number, PageLinkInfo[]>();
  private readonly linksPending = new Set<number>();
  /** Characters of addresses kept for each page in `links`, and their total. */
  private readonly linkSizes = new Map<number, number>();
  private linkChars = 0;
  private limitReported = false;
  /** Mounted pages that were given fewer boxes or link buttons than they wanted. */
  private readonly starved = new Set<number>();
  private starvedPaint = false;
  /** Set while a page is painted when a claim for room came back short. */
  private shortOfRoom = false;
  private readonly boxBudget = new Budget(MAX_OVERLAY_BOXES);
  private readonly pickedBudget = new Budget(MAX_SELECTION_BOXES);
  private readonly linkBudget = new Budget(MAX_LINK_BUTTONS);
  /** True while a copy fetches pages: there is one at a time for each document. */
  private copying = false;
  /** Pages of the selection whose text was loaded ahead, within the limit, and is kept. */
  private readonly preloaded = new Set<number>();
  private readonly limits: OverlayLimits;
  private selection: Selection | null = null;
  private find: ReadonlyMap<number, readonly [number, number][]> = new Map();
  private findCurrent: SearchMatch | null = null;
  private dragging = false;
  private pressed: { x: number; y: number; moved: boolean; detail: number } | null = null;
  private pointer: { x: number; y: number } | null = null;
  /** The last press, to count double and triple clicks (pointer events carry no click count). */
  private lastPress: { time: number; x: number; y: number; count: number } | null = null;
  private destroyed = false;

  constructor(
    private readonly doc: number,
    private readonly pages: PageSizePt[],
    private readonly api: OverlayApi,
    private readonly host: OverlayHost,
    private readonly events: OverlayEvents,
    limits: Partial<OverlayLimits> = {},
  ) {
    this.limits = { ...DEFAULT_LIMITS, ...limits };
    this.texts = new TextStore(
      (page) => this.fetchText(page),
      (page) => this.isKept(page),
      this.limits.keptChars,
    );
    const el = host.element;
    el.addEventListener("pointerdown", this.onPointerDown);
    el.addEventListener("pointermove", this.onPointerMove);
    el.addEventListener("pointerup", this.onPointerUp);
    el.addEventListener("pointercancel", this.onPointerUp);
  }

  // ---- the pages ---------------------------------------------------------------

  /** A page got an element: give it a layer for the highlights and the links. */
  mount(page: number, pageEl: HTMLElement): void {
    const layer = document.createElement("div");
    layer.className = "page-overlay";
    for (const name of ["find", "select", "links"]) {
      const sub = document.createElement("div");
      sub.className = `layer-${name}`;
      layer.append(sub);
    }
    pageEl.append(layer);
    this.overlays.set(page, layer);
    this.texts.allow(page);
    this.paint(page);
  }

  unmount(page: number): void {
    this.overlays.get(page)?.remove();
    this.overlays.delete(page);
    this.boxBudget.release(`${page}:find`);
    this.boxBudget.release(`${page}:select`);
    this.pickedBudget.release(String(page));
    this.linkBudget.release(String(page));
    this.texts.allow(page);
    // The links of a page that is off screen are not kept: they are asked for again when the page
    // is drawn again, so a long document does not pile them up.
    this.dropLinks(page);
    this.starved.delete(page);
    this.paintStarvedLater();
  }

  /**
   * A page that left gave its room back, so pages that got less than they wanted get another try.
   * Once for all the pages that leave in one step.
   */
  private paintStarvedLater(): void {
    if (this.starved.size === 0 || this.starvedPaint) return;
    this.starvedPaint = true;
    queueMicrotask(() => {
      this.starvedPaint = false;
      if (this.destroyed) return;
      for (const page of [...this.starved]) {
        if (this.overlays.has(page)) this.paint(page);
        else this.starved.delete(page);
      }
    });
  }

  private dropLinks(page: number): void {
    this.links.delete(page);
    this.linkChars -= this.linkSizes.get(page) ?? 0;
    this.linkSizes.delete(page);
  }

  /** The text or the links of a page did not fit: say so, once for the document. */
  private reportLimit(): void {
    if (this.limitReported || this.destroyed) return;
    this.limitReported = true;
    this.events.onTextLimit?.();
  }

  /** A page has been drawn: now is the time to ask for its text and its links. */
  pageDrawn(page: number): void {
    if (this.destroyed) return;
    // A redraw (a new zoom) is a fresh try for a page that could not be read. A page that was
    // refused for lack of room keeps waiting for room.
    this.texts.retry(page);
    if (!this.texts.unavailable(page)) {
      void this.texts.load(page).then(() => {
        if (this.texts.refused(page)) this.reportLimit();
        this.paint(page);
      });
    }
    this.loadLinks(page);
  }

  destroy(): void {
    this.destroyed = true;
    const el = this.host.element;
    el.removeEventListener("pointerdown", this.onPointerDown);
    el.removeEventListener("pointermove", this.onPointerMove);
    el.removeEventListener("pointerup", this.onPointerUp);
    el.removeEventListener("pointercancel", this.onPointerUp);
    for (const layer of this.overlays.values()) layer.remove();
    this.overlays.clear();
    this.texts.clear();
  }

  /** Text and links come from the engine; a page that cannot be read just has none. */
  private async fetchText(page: number): Promise<PageText | null> {
    if (!this.api.getPageText) return null;
    const wire = await this.api.getPageText(this.doc, page);
    if (!this.destroyed && this.events.onScripts) {
      const scripts = scriptsIn(wire.text);
      if (scripts.size > 0) this.events.onScripts([...scripts]);
    }
    return parsePageText(wire.text, wire.rects);
  }

  private loadLinks(page: number): void {
    if (!this.api.getPageLinks || this.links.has(page) || this.linksPending.has(page)) return;
    this.linksPending.add(page);
    this.api
      .getPageLinks(this.doc, page)
      .then((links) => {
        if (!this.overlays.has(page)) return;
        const size = links.reduce((n, l) => n + (l.uri?.length ?? 0), 0);
        if (this.linkChars + size > MAX_LINK_CHARS) {
          // Too many addresses for the document: this page has no link boxes.
          this.links.set(page, []);
          this.reportLimit();
          return;
        }
        this.links.set(page, links);
        this.linkSizes.set(page, size);
        this.linkChars += size;
      })
      .catch(() => {
        if (this.overlays.has(page)) this.links.set(page, []);
      })
      .then(() => {
        this.linksPending.delete(page);
        this.paint(page);
      });
  }

  /**
   * A page whose text must be kept: it is on screen (its highlights are drawn from the text), or
   * it is part of the selection that was loaded ahead. Everything else can be fetched again, so the
   * text kept for a document stays within a fixed total however much is selected or found.
   */
  private isKept(page: number): boolean {
    return this.overlays.has(page) || this.preloaded.has(page);
  }

  // ---- painting ----------------------------------------------------------------

  private paint(page: number): void {
    const layer = this.overlays.get(page);
    if (!layer || this.destroyed) return;
    const [find, select, links] = [...layer.children] as HTMLElement[];
    this.shortOfRoom = false;
    const text = this.texts.get(page);
    const size = this.pages[page];

    const box = (r: Rect, className: string): HTMLElement => {
      const el = document.createElement("div");
      el.className = className;
      el.style.left = percent(r.left, size.width);
      el.style.top = percent(r.top, size.height);
      el.style.width = percent(r.right - r.left, size.width);
      el.style.height = percent(r.bottom - r.top, size.height);
      return el;
    };

    // Search matches: the one picked out is drawn stronger. Its boxes have a count of their own, so
    // that it is still drawn when the other matches have used up the boxes of the document.
    const found = this.find.get(page);
    if (found && text) {
      const current = this.findCurrent;
      const picked: Rect[] = [];
      const others: Rect[] = [];
      for (const [start, len] of found) {
        const isCurrent = current !== null && current.page === page && current.start === start;
        const into = isCurrent ? picked : others;
        if (into.length >= MAX_SELECTION_BOXES) continue;
        for (const r of lineRects(text, start, start + len)) {
          if (into.length >= MAX_SELECTION_BOXES) break;
          into.push(r);
        }
      }
      const pickedAllowed = this.pickedBudget.claim(String(page), picked.length);
      const othersAllowed = this.claimBoxes(`${page}:find`, others.length);
      find.replaceChildren(
        ...picked.slice(0, pickedAllowed).map((r) => box(r, "hl hl-find hl-current")),
        ...others.slice(0, othersAllowed).map((r) => box(r, "hl hl-find")),
      );
    } else {
      find.replaceChildren();
      this.boxBudget.release(`${page}:find`);
      this.pickedBudget.release(String(page));
      if (found && !text && !this.texts.unavailable(page)) void this.texts.load(page).then(() => this.paint(page));
    }

    // Selection
    const range = this.selection ? rangeOnPage(this.selection, page) : null;
    if (range && text) {
      const wanted = lineRects(text, range[0], Math.min(range[1], text.chars.length)).slice(0, MAX_SELECTION_BOXES);
      const allowed = this.claimBoxes(`${page}:select`, wanted.length);
      select.replaceChildren(...wanted.slice(0, allowed).map((r) => box(r, "hl hl-select")));
    } else {
      select.replaceChildren();
      this.boxBudget.release(`${page}:select`);
      // A page that cannot be read is asked for once, not again at every repaint of the selection.
      if (range && !text && !this.texts.unavailable(page)) void this.texts.load(page).then(() => this.paint(page));
    }

    // Links
    const wantedLinks = this.links.get(page) ?? [];
    const allowedLinks = this.linkBudget.claim(String(page), wantedLinks.length);
    if (allowedLinks < wantedLinks.length) {
      this.shortOfRoom = true;
      this.reportLimit();
    }
    const pageLinks = wantedLinks.slice(0, allowedLinks);
    links.replaceChildren(
      ...pageLinks.map((link) => {
        const b = document.createElement("button");
        b.type = "button";
        b.className = "page-link";
        const [l, t, r, bt] = link.rect;
        b.style.left = percent(l, size.width);
        b.style.top = percent(t, size.height);
        b.style.width = percent(r - l, size.width);
        b.style.height = percent(bt - t, size.height);
        // Only the site a web address would open is named, not the address as the document wrote it.
        const label =
          link.page !== null ? `Go to page ${link.page + 1}` : `Link to ${describeLink(link.uri ?? "")}`;
        b.setAttribute("aria-label", label);
        b.title = link.page !== null ? label : describeLink(link.uri ?? "");
        b.addEventListener("click", (e) => {
          e.preventDefault();
          e.stopPropagation();
          this.events.onLink(link, page);
        });
        return b;
      }),
    );
    if (this.shortOfRoom) this.starved.add(page);
    else this.starved.delete(page);
  }

  /** Asks for room for boxes of one kind on one page. When the document's boxes are used up, says so. */
  private claimBoxes(owner: string, wanted: number): number {
    const allowed = this.boxBudget.claim(owner, wanted);
    if (allowed < wanted) {
      this.shortOfRoom = true;
      this.reportLimit();
    }
    return allowed;
  }

  private paintAll(): void {
    for (const page of this.overlays.keys()) this.paint(page);
  }

  // ---- selection ---------------------------------------------------------------

  get hasSelection(): boolean {
    return !isEmptySelection(this.selection);
  }

  clearSelection(): void {
    if (this.selection === null) return;
    this.selection = null;
    this.paintAll();
    this.events.onSelectionChange?.(false);
  }

  private setSelection(sel: Selection | null): void {
    const was = this.hasSelection;
    this.selection = sel;
    for (const page of this.preloaded) {
      if (!sel || !rangeOnPage(sel, page)) this.preloaded.delete(page);
    }
    this.paintAll();
    const now = this.hasSelection;
    if (was !== now) this.events.onSelectionChange?.(now);
  }

  /** Selects all the text of the document. Some of the text is loaded ahead, for a quick copy. */
  selectAll(): void {
    const last = this.pages.length - 1;
    if (last < 0) return;
    this.setSelection({
      anchor: { page: 0, offset: 0 },
      focus: { page: last, offset: Number.MAX_SAFE_INTEGER },
    });
    void this.preloadSelected();
  }

  /** The selected text from the pages that are loaded, and the pages that are not. */
  selectedText(): { text: string; missing: number[]; cut: number } {
    const sel = this.selection;
    if (!sel || isEmptySelection(sel)) return { text: "", missing: [], cut: 0 };
    const { start, end } = orderSelection(sel);
    // Pages the engine has cut (it gives 100,000 characters at most) and whose end the selection
    // reaches: the end is not in the text.
    let cut = 0;
    for (let page = start.page; page <= end.page; page++) {
      if (reachesCutEnd(sel, page, this.texts.get(page))) cut++;
    }
    return { ...selectionText(sel, (page) => this.texts.get(page)), cut };
  }

  /**
   * The selected text for a Copy. Pages that are not loaded are fetched one at a time and left to
   * the store to drop, so a selection of a whole book never sits in memory page by page. The copy
   * stops at a fixed size, and the result says how far it got, which pages could not be read, and
   * whether the engine had cut a page. Only one copy runs for a document at a time, so the limit
   * is for the document and not for each press of Copy.
   */
  async loadSelectedText(onProgress?: (done: number, total: number) => void): Promise<CopyResult> {
    const result: CopyResult = {
      text: "",
      pages: 0,
      total: 0,
      failed: [],
      truncated: false,
      cut: 0,
      status: "done",
    };
    const sel = this.selection;
    if (!sel || isEmptySelection(sel)) return result;
    if (this.copying) return { ...result, status: "busy" };
    this.copying = true;
    try {
      return await this.gather(sel, result, onProgress);
    } finally {
      this.copying = false;
    }
  }

  private async gather(
    sel: Selection,
    result: CopyResult,
    onProgress?: (done: number, total: number) => void,
  ): Promise<CopyResult> {
    const { start, end } = orderSelection(sel);
    result.total = end.page - start.page + 1;
    const parts: string[] = [];
    let used = 0;
    for (let page = start.page; page <= end.page; page++) {
      const text = this.texts.get(page) ?? (await this.texts.load(page));
      // A closed document gives nothing: half a text must not reach the clipboard.
      if (this.destroyed) return { ...result, text: "", pages: 0, status: "cancelled" };
      onProgress?.(page - start.page + 1, result.total);
      if (!text) {
        result.failed.push(page);
        continue;
      }
      if (reachesCutEnd(sel, page, text)) result.cut++;
      const range = rangeOnPage(sel, page);
      const piece = range ? textInRange(text, range[0], Math.min(range[1], text.chars.length)) : "";
      if (piece.length > 0) {
        const cost = piece.length + (parts.length > 0 ? 1 : 0);
        if (used + cost > this.limits.copyChars) {
          result.truncated = true;
          break;
        }
        parts.push(piece);
        used += cost;
      }
      result.pages++;
    }
    result.text = parts.join("\n");
    if (this.destroyed) return { ...result, text: "", pages: 0, status: "cancelled" };
    if (this.selection !== sel) result.status = "changed";
    return result;
  }

  /** Loads the text of the first pages of the selection, up to the limit, and keeps it. */
  private async preloadSelected(): Promise<void> {
    const sel = this.selection;
    if (!sel) return;
    const { start, end } = orderSelection(sel);
    let chars = 0;
    for (let page = start.page; page <= end.page && chars < this.limits.preloadChars; page++) {
      if (this.destroyed || this.selection !== sel) return;
      const text = this.texts.get(page) ?? (await this.texts.load(page));
      if (text) {
        chars += text.chars.length;
        this.preloaded.add(page);
      }
    }
    if (!this.destroyed && this.selection === sel) {
      this.paintAll();
      this.events.onSelectionSettled?.();
    }
  }

  private pointAt(
    clientX: number,
    clientY: number,
  ): { page: number; x: number; y: number } | null {
    const mounted = this.host.mountedPages();
    let best = -1;
    let bestDistance = Infinity;
    for (const page of mounted) {
      const el = this.host.pageElement(page);
      if (!el) continue;
      const rect = el.getBoundingClientRect();
      const distance = clientY < rect.top ? rect.top - clientY : clientY > rect.bottom ? clientY - rect.bottom : 0;
      if (distance < bestDistance) {
        best = page;
        bestDistance = distance;
      }
    }
    if (best < 0) return null;
    const rect = (this.host.pageElement(best) as HTMLElement).getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return null;
    const size = this.pages[best];
    return {
      page: best,
      x: ((clientX - rect.left) / rect.width) * size.width,
      y: ((clientY - rect.top) / rect.height) * size.height,
    };
  }

  private caretAt(clientX: number, clientY: number): { page: number; offset: number } | null {
    const at = this.pointAt(clientX, clientY);
    if (!at) return null;
    const text = this.texts.get(at.page);
    if (!text) {
      // A page that has no text layer for lack of room cannot be pointed at.
      if (this.texts.unavailable(at.page)) return null;
      void this.texts.load(at.page).then(() => {
        if (this.dragging && this.pointer) this.extendTo(this.pointer.x, this.pointer.y);
      });
      return null;
    }
    return { page: at.page, offset: offsetAt(text, at.x, at.y) };
  }

  private onPointerDown = (e: Event): void => {
    const p = e as PointerEvent;
    if (p.button !== 0 || this.destroyed) return;
    const target = p.target;
    if (target instanceof Element && target.closest(".page-link")) return;
    const last = this.lastPress;
    const count =
      last &&
      p.timeStamp - last.time < MULTI_CLICK_MS &&
      Math.hypot(p.clientX - last.x, p.clientY - last.y) < MULTI_CLICK_PX
        ? Math.min(last.count + 1, 3)
        : 1;
    this.lastPress = { time: p.timeStamp, x: p.clientX, y: p.clientY, count };
    // A press is a fresh try for a page whose text could not be read.
    const at = this.pointAt(p.clientX, p.clientY);
    if (at) this.texts.retry(at.page);
    const caret = this.caretAt(p.clientX, p.clientY);
    this.pressed = { x: p.clientX, y: p.clientY, moved: false, detail: count };
    this.pointer = { x: p.clientX, y: p.clientY };
    if (!caret) {
      this.clearSelection();
      return;
    }
    const text = this.texts.get(caret.page) as PageText;
    if (count === 2 || count === 3) {
      const [from, to] = count === 2 ? wordRange(text, caret.offset) : lineRange(text, caret.offset);
      this.setSelection({ anchor: { page: caret.page, offset: from }, focus: { page: caret.page, offset: to } });
      this.dragging = false;
      this.events.onSelectionSettled?.();
      return;
    }
    this.dragging = true;
    this.setSelection({ anchor: caret, focus: caret });
    try {
      this.host.element.setPointerCapture?.(p.pointerId);
    } catch {
      // Without capture the drag still works while the pointer stays in the view.
    }
  };

  private onPointerMove = (e: Event): void => {
    const p = e as PointerEvent;
    if (!this.dragging || !this.pressed) return;
    this.pointer = { x: p.clientX, y: p.clientY };
    if (!this.pressed.moved && Math.hypot(p.clientX - this.pressed.x, p.clientY - this.pressed.y) >= DRAG_DISTANCE) {
      this.pressed.moved = true;
    }
    if (!this.pressed.moved) return;
    this.scrollNearEdge(p.clientY);
    this.extendTo(p.clientX, p.clientY);
  };

  private onPointerUp = (e: Event): void => {
    const p = e as PointerEvent;
    const pressed = this.pressed;
    this.pressed = null;
    this.dragging = false;
    this.pointer = null;
    try {
      this.host.element.releasePointerCapture?.(p.pointerId);
    } catch {
      // Nothing was captured.
    }
    // A click without a drag only puts the caret there: it selects nothing.
    if (pressed && !pressed.moved && pressed.detail === 1) this.clearSelection();
    else if (pressed && this.hasSelection) this.events.onSelectionSettled?.();
  };

  private extendTo(clientX: number, clientY: number): void {
    const sel = this.selection;
    if (!sel) return;
    const caret = this.caretAt(clientX, clientY);
    if (!caret) return;
    if (caret.page === sel.focus.page && caret.offset === sel.focus.offset) return;
    this.setSelection({ anchor: sel.anchor, focus: caret });
  }

  /** While dragging near the top or bottom edge, the view scrolls so the selection can go on. */
  private scrollNearEdge(clientY: number): void {
    const view = this.host.element.getBoundingClientRect();
    if (clientY < view.top + EDGE) this.host.element.scrollTop -= 24;
    else if (clientY > view.bottom - EDGE) this.host.element.scrollTop += 24;
  }

  // ---- search highlights ---------------------------------------------------------

  /** Shows the matches of a search. `current` is the one that is picked out. */
  setFind(matches: ReadonlyMap<number, readonly [number, number][]>, current: SearchMatch | null): void {
    const before = this.find;
    const beforeCurrent = this.findCurrent;
    // The lists of matches are kept as they are given (a search does not change a list once it has
    // given it), so a page is painted again only when its list is a new one, or when the match that
    // is picked out moved from it or to it.
    this.find = new Map(matches);
    this.findCurrent = current;
    const changed = new Set<number>();
    for (const [page, list] of this.find) if (before.get(page) !== list) changed.add(page);
    for (const page of before.keys()) if (!this.find.has(page)) changed.add(page);
    if (beforeCurrent?.page !== current?.page || beforeCurrent?.start !== current?.start) {
      if (beforeCurrent) changed.add(beforeCurrent.page);
      if (current) changed.add(current.page);
    }
    for (const page of changed) this.paint(page);
  }

  clearFind(): void {
    this.setFind(new Map(), null);
  }

  /** Scrolls to a match so that it is in view. The page's text is loaded first if needed. */
  async revealMatch(match: SearchMatch): Promise<void> {
    const text = await this.texts.load(match.page);
    if (!text || this.destroyed) return;
    const rects = lineRects(text, match.start, match.start + match.len);
    if (rects.length === 0) {
      this.host.scrollToRect(match.page, { left: 0, top: 0, right: 1, bottom: 1 });
      return;
    }
    this.host.scrollToRect(match.page, rects[0]);
  }

  /** Lets go of the text that is not needed now, such as when the view is hidden. */
  releaseText(): void {
    this.texts.retain((page) => this.isKept(page));
  }

  /** How many characters of page text are kept now. For tests and for diagnostics. */
  get keptTextChars(): number {
    return this.texts.keptChars;
  }

  /** The text of a page, if it is loaded. For tests and for the receiver of a copy. */
  textOf(page: number): PageText | undefined {
    return this.texts.get(page);
  }
}

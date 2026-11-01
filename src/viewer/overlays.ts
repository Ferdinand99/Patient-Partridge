// What sits on top of the drawn pages: the selection, the search highlights and the links.
//
// The engine sends the text of a page, and the links, as plain data. Here that data is turned into
// boxes placed over the page with percentages of the page size, so they stay right at any zoom
// without being placed again. PDF text and addresses are data: they are never put in the page as
// markup, only as text content, and nothing here follows a link by itself.

import { shortAddress } from "../lib/linkPolicy";
import type { PageSizePt } from "./layout";
import type { SearchMatch } from "./search";
import {
  TextStore,
  isEmptySelection,
  lineRange,
  lineRects,
  offsetAt,
  orderSelection,
  parsePageText,
  rangeOnPage,
  selectionText,
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
}

/** Most pages whose text is loaded in the background after Select all. */
export const MAX_SELECT_ALL_PAGES = 600;
/** How far the pointer must move before a press becomes a drag, in pixels. */
const DRAG_DISTANCE = 4;
/** How close to the edge of the view the pointer must be to scroll it while dragging, in pixels. */
const EDGE = 28;

function percent(value: number, of: number): string {
  return `${((of > 0 ? value / of : 0) * 100).toFixed(3)}%`;
}

export class PageOverlays {
  private readonly texts: TextStore;
  private readonly overlays = new Map<number, HTMLElement>();
  private readonly links = new Map<number, PageLinkInfo[]>();
  private readonly linksPending = new Set<number>();
  private selection: Selection | null = null;
  private find = new Map<number, [number, number][]>();
  private findCurrent: SearchMatch | null = null;
  private dragging = false;
  private pressed: { x: number; y: number; moved: boolean; detail: number } | null = null;
  private pointer: { x: number; y: number } | null = null;
  private destroyed = false;

  constructor(
    private readonly doc: number,
    private readonly pages: PageSizePt[],
    private readonly api: OverlayApi,
    private readonly host: OverlayHost,
    private readonly events: OverlayEvents,
  ) {
    this.texts = new TextStore(
      (page) => this.fetchText(page),
      (page) => this.isNeeded(page),
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
    this.paint(page);
  }

  unmount(page: number): void {
    this.overlays.get(page)?.remove();
    this.overlays.delete(page);
  }

  /** A page has been drawn: now is the time to ask for its text and its links. */
  pageDrawn(page: number): void {
    if (this.destroyed) return;
    void this.texts.load(page).then(() => this.paint(page));
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
    return parsePageText(wire.text, wire.rects);
  }

  private loadLinks(page: number): void {
    if (!this.api.getPageLinks || this.links.has(page) || this.linksPending.has(page)) return;
    this.linksPending.add(page);
    this.api
      .getPageLinks(this.doc, page)
      .then((links) => this.links.set(page, links))
      .catch(() => this.links.set(page, []))
      .then(() => {
        this.linksPending.delete(page);
        this.paint(page);
      });
  }

  /** A page whose text must be kept: it is inside the selection or holds a search match. */
  private isNeeded(page: number): boolean {
    if (this.selection && rangeOnPage(this.selection, page)) return true;
    return this.find.has(page);
  }

  // ---- painting ----------------------------------------------------------------

  private paint(page: number): void {
    const layer = this.overlays.get(page);
    if (!layer || this.destroyed) return;
    const [find, select, links] = [...layer.children] as HTMLElement[];
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

    // Search matches: the one picked out is drawn stronger
    const found = this.find.get(page);
    if (found && text) {
      const current = this.findCurrent;
      const elements: HTMLElement[] = [];
      for (const [start, len] of found) {
        const isCurrent = current !== null && current.page === page && current.start === start;
        for (const r of lineRects(text, start, start + len)) {
          elements.push(box(r, isCurrent ? "hl hl-find hl-current" : "hl hl-find"));
        }
      }
      find.replaceChildren(...elements);
    } else {
      find.replaceChildren();
      if (found && !text) void this.texts.load(page).then(() => this.paint(page));
    }

    // Selection
    const range = this.selection ? rangeOnPage(this.selection, page) : null;
    if (range && text) {
      select.replaceChildren(
        ...lineRects(text, range[0], Math.min(range[1], text.chars.length)).map((r) => box(r, "hl hl-select")),
      );
    } else {
      select.replaceChildren();
      if (range && !text) void this.texts.load(page).then(() => this.paint(page));
    }

    // Links
    const pageLinks = this.links.get(page) ?? [];
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
        const label =
          link.page !== null
            ? `Go to page ${link.page + 1}`
            : `Link to ${shortAddress(link.uri ?? "", 80)}`;
        b.setAttribute("aria-label", label);
        b.title = link.page !== null ? label : shortAddress(link.uri ?? "", 80);
        b.addEventListener("click", (e) => {
          e.preventDefault();
          e.stopPropagation();
          this.events.onLink(link, page);
        });
        return b;
      }),
    );
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
    this.paintAll();
    const now = this.hasSelection;
    if (was !== now) this.events.onSelectionChange?.(now);
  }

  /** Selects all the text of the document. Text is loaded in the background for copying. */
  selectAll(): void {
    const last = this.pages.length - 1;
    if (last < 0) return;
    this.setSelection({
      anchor: { page: 0, offset: 0 },
      focus: { page: last, offset: Number.MAX_SAFE_INTEGER },
    });
    void this.loadSelected();
  }

  /** The selected text from the pages that are loaded, and the pages that are not. */
  selectedText(): { text: string; missing: number[] } {
    if (!this.selection || isEmptySelection(this.selection)) return { text: "", missing: [] };
    return selectionText(this.selection, (page) => this.texts.get(page));
  }

  /** The selected text, loading the pages it still lacks. */
  async loadSelectedText(): Promise<string> {
    await this.loadSelected();
    return this.selectedText().text;
  }

  private async loadSelected(): Promise<void> {
    const sel = this.selection;
    if (!sel) return;
    const { start, end } = orderSelection(sel);
    const last = Math.min(end.page, start.page + MAX_SELECT_ALL_PAGES);
    for (let page = start.page; page <= last; page++) {
      if (this.destroyed || this.selection !== sel) return;
      if (!this.texts.get(page)) await this.texts.load(page);
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
    const caret = this.caretAt(p.clientX, p.clientY);
    this.pressed = { x: p.clientX, y: p.clientY, moved: false, detail: p.detail || 1 };
    this.pointer = { x: p.clientX, y: p.clientY };
    if (!caret) {
      this.clearSelection();
      return;
    }
    const text = this.texts.get(caret.page) as PageText;
    if (p.detail === 2 || p.detail === 3) {
      const [from, to] = p.detail === 2 ? wordRange(text, caret.offset) : lineRange(text, caret.offset);
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
    const before = new Set(this.find.keys());
    this.find = new Map([...matches].map(([page, list]) => [page, [...list] as [number, number][]]));
    this.findCurrent = current;
    for (const page of new Set([...before, ...this.find.keys()])) this.paint(page);
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
    this.texts.retain((page) => this.isNeeded(page));
  }

  /** The text of a page, if it is loaded. For tests and for the receiver of a copy. */
  textOf(page: number): PageText | undefined {
    return this.texts.get(page);
  }
}

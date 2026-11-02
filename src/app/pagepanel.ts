// The page panel: the thumbnails of the side panel with the page tools on top. Pages are picked
// with the mouse (a click, Ctrl and Shift) or the keyboard (the arrow keys, Shift, Ctrl+A, Space),
// moved by dragging them or with Alt and the arrow keys, and worked on with the buttons or with
// Delete and R. What a tool does to the document is in pageops.ts; this is the part you touch.

import {
  BLANK_SIZES,
  PageSelection,
  insertionForStep,
  isIdentity,
  orderAfterMove,
  pagesWord,
} from "../lib/pages";
import type { PageSizePt } from "../viewer/layout";
import { askCrop } from "./cropdialog";
import type { OpResult, PageOps, PageTarget } from "./pageops";
import type { ThumbnailList } from "./thumbnails";

export interface PagePanelHost {
  target(): PageTarget | null;
  /** The page in view in the main view (counted from 0). */
  currentPage(): number;
  goToPage(index: number): void;
  /** Draws a page as raw RGBA. */
  render(doc: number, page: number, width: number, height: number): Promise<ArrayBuffer>;
  /** The layer of the message boxes, for the crop box. */
  layer: HTMLElement;
  announce(text: string): void;
}

/** How far the pointer moves before a press becomes a drag, in pixels. */
const DRAG_DISTANCE = 5;

function byId<T extends HTMLElement>(root: Document, id: string): T {
  const el = root.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
}

export class PagePanel {
  private readonly selections = new Map<number, PageSelection>();
  private list: ThumbnailList | null = null;
  private listRoot: HTMLElement | null = null;
  private doc: number | null = null;
  private pages: PageSizePt[] = [];
  private press: { x: number; y: number; page: number; id: number; dragging: boolean } | null = null;
  private dropAt = 0;
  private suppressClick = false;

  private readonly tools: HTMLElement;
  private readonly status: HTMLElement;
  private readonly buttons: Record<"left" | "right" | "delete" | "crop" | "extract" | "blank" | "insert", HTMLButtonElement>;

  constructor(
    root: Document,
    private readonly ops: PageOps,
    private readonly host: PagePanelHost,
  ) {
    this.tools = byId(root, "pagetools");
    this.status = byId(root, "pt-status");
    this.buttons = {
      left: byId(root, "pt-left"),
      right: byId(root, "pt-right"),
      delete: byId(root, "pt-delete"),
      crop: byId(root, "pt-crop"),
      extract: byId(root, "pt-extract"),
      blank: byId(root, "pt-blank"),
      insert: byId(root, "pt-insert"),
    };
    this.buttons.left.addEventListener("click", () => void this.turn(3));
    this.buttons.right.addEventListener("click", () => void this.turn(1));
    this.buttons.delete.addEventListener("click", () => void this.remove());
    this.buttons.crop.addEventListener("click", () => void this.crop());
    this.buttons.extract.addEventListener("click", () => void this.extract());
    this.buttons.blank.addEventListener("click", () => void this.addBlank());
    this.buttons.insert.addEventListener("click", () => void this.addFiles());
  }

  // ---- what the side panel tells the panel ----------------------------------------------------

  /** The side panel built a list of thumbnails for a document. */
  attach(list: ThumbnailList, listRoot: HTMLElement, doc: number, pages: PageSizePt[]): void {
    this.detach();
    this.list = list;
    this.listRoot = listRoot;
    this.doc = doc;
    this.pages = pages;
    this.selection(doc).setCount(pages.length);
    listRoot.addEventListener("keydown", this.onKeyDown);
    listRoot.addEventListener("pointerdown", this.onPointerDown);
    listRoot.addEventListener("pointermove", this.onPointerMove);
    listRoot.addEventListener("pointerup", this.onPointerUp);
    listRoot.addEventListener("pointercancel", this.onPointerCancel);
    listRoot.setAttribute("role", "group");
    listRoot.setAttribute("aria-label", "Pages");
    this.sync();
  }

  detach(): void {
    const root = this.listRoot;
    if (root) {
      root.removeEventListener("keydown", this.onKeyDown);
      root.removeEventListener("pointerdown", this.onPointerDown);
      root.removeEventListener("pointermove", this.onPointerMove);
      root.removeEventListener("pointerup", this.onPointerUp);
      root.removeEventListener("pointercancel", this.onPointerCancel);
      root.removeAttribute("role");
      root.removeAttribute("aria-label");
    }
    this.list = null;
    this.listRoot = null;
    this.doc = null;
    this.press = null;
  }

  /** The tools are shown while the Pages panel is, with a document. */
  showTools(visible: boolean): void {
    this.tools.hidden = !visible;
    this.updateTools();
  }

  /** The pages of a document changed behind the panel's back (a tool, taking a change back): nothing is picked. */
  pagesChanged(doc: number): void {
    this.selections.get(doc)?.clear();
  }

  forget(doc: number): void {
    this.selections.delete(doc);
  }

  /** A tool starts or stops working: the buttons wait. */
  setBusy(busy: boolean): void {
    this.tools.setAttribute("aria-busy", String(busy));
    this.updateTools();
  }

  /** The element of a thumbnail gets its marks (the list calls this when it makes one). */
  decorate = (el: HTMLElement, index: number): void => {
    const sel = this.current();
    const picked = sel?.has(index) ?? false;
    el.setAttribute("aria-pressed", String(picked));
    el.toggleAttribute("data-picked", picked);
    const cursor = sel && sel.cursor >= 0 ? sel.cursor : Math.min(this.host.currentPage(), this.pages.length - 1);
    el.tabIndex = index === cursor ? 0 : -1;
    el.setAttribute("aria-label", picked ? `Page ${index + 1}, picked` : `Page ${index + 1}`);
  };

  /** A click on a thumbnail. Returns true when the panel dealt with it (the main view stays). */
  select(page: number, event?: MouseEvent): boolean {
    if (this.suppressClick) return true;
    const sel = this.current();
    if (!sel) return false;
    const shift = event?.shiftKey ?? false;
    const ctrl = (event?.ctrlKey || event?.metaKey) ?? false;
    sel.click(page, { shift, ctrl });
    this.afterSelect(page, false);
    return shift || ctrl;
  }

  // ---- selection -----------------------------------------------------------------------------

  private selection(doc: number): PageSelection {
    let sel = this.selections.get(doc);
    if (!sel) {
      sel = new PageSelection(this.pages.length);
      this.selections.set(doc, sel);
    }
    return sel;
  }

  private current(): PageSelection | null {
    return this.doc === null ? null : this.selection(this.doc);
  }

  /** Pages picked, in page order. */
  picked(): number[] {
    return this.current()?.toArray() ?? [];
  }

  /** Picks pages (after a tool). */
  pick(pages: number[]): void {
    const sel = this.current();
    if (!sel) return;
    sel.clear();
    pages.forEach((p, i) => (i === 0 ? sel.only(p) : sel.toggle(p)));
    this.sync();
  }

  private afterSelect(cursor: number, focus: boolean): void {
    this.sync();
    if (focus) this.focusCursor(cursor);
  }

  private focusCursor(page: number): void {
    this.list?.scrollToPage(page);
    this.list?.elementOf(page)?.focus({ preventScroll: true });
  }

  /** Brings the marks and the buttons in step with the selection. */
  private sync(): void {
    this.list?.redecorate();
    this.updateTools();
    const n = this.picked().length;
    this.status.textContent = n === 0 ? "" : `${pagesWord(n)} picked`;
  }

  private updateTools(): void {
    const busy = this.ops.busy;
    const has = this.picked().length > 0;
    const ready = this.doc !== null && !busy;
    for (const key of ["left", "right", "delete", "crop", "extract"] as const) {
      this.buttons[key].disabled = !(ready && has);
    }
    this.buttons.blank.disabled = !ready;
    this.buttons.insert.disabled = !ready;
  }

  // ---- the keyboard --------------------------------------------------------------------------

  private onKeyDown = (e: KeyboardEvent): void => {
    const sel = this.current();
    if (!sel || this.ops.busy) return;
    const ctrl = e.ctrlKey || e.metaKey;
    const page = (by: number, extend = e.shiftKey) => {
      sel.step(by, extend);
      this.afterSelect(sel.cursor, true);
      e.preventDefault();
    };
    if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
      e.preventDefault();
      void this.moveBy(e.key === "ArrowUp" ? -1 : 1);
      return;
    }
    switch (e.key) {
      case "ArrowDown":
      case "ArrowRight":
        if (ctrl) {
          sel.moveCursor(1);
          this.afterSelect(sel.cursor, true);
          e.preventDefault();
        } else page(1);
        break;
      case "ArrowUp":
      case "ArrowLeft":
        if (ctrl) {
          sel.moveCursor(-1);
          this.afterSelect(sel.cursor, true);
          e.preventDefault();
        } else page(-1);
        break;
      case "PageDown":
        page(5);
        break;
      case "PageUp":
        page(-5);
        break;
      case "Home":
        sel.edge(false, e.shiftKey);
        this.afterSelect(sel.cursor, true);
        e.preventDefault();
        break;
      case "End":
        sel.edge(true, e.shiftKey);
        this.afterSelect(sel.cursor, true);
        e.preventDefault();
        break;
      case "a":
      case "A":
        if (ctrl) {
          sel.all();
          this.afterSelect(sel.cursor, true);
          e.preventDefault();
          e.stopPropagation();
        }
        break;
      case " ":
        if (sel.cursor >= 0) {
          sel.toggle(sel.cursor);
          this.afterSelect(sel.cursor, true);
        }
        e.preventDefault();
        break;
      case "Enter":
        if (sel.cursor >= 0) this.host.goToPage(sel.cursor);
        e.preventDefault();
        break;
      case "Delete":
      case "Backspace":
        e.preventDefault();
        void this.remove();
        break;
      case "r":
      case "R":
        if (!ctrl && !e.altKey) {
          e.preventDefault();
          void this.turn(e.shiftKey ? 3 : 1);
        }
        break;
      case "Escape":
        if (sel.size > 0) {
          sel.clear();
          this.sync();
          e.preventDefault();
          e.stopPropagation();
        }
        break;
      default:
    }
  };

  // ---- dragging --------------------------------------------------------------------------------

  private onPointerDown = (e: PointerEvent): void => {
    if (e.button !== 0 || this.ops.busy) return;
    const item = (e.target as Element | null)?.closest<HTMLElement>(".thumb");
    const page = Number(item?.dataset.page);
    if (!item || !Number.isInteger(page)) return;
    this.press = { x: e.clientX, y: e.clientY, page, id: e.pointerId, dragging: false };
  };

  private onPointerMove = (e: PointerEvent): void => {
    const p = this.press;
    const sel = this.current();
    if (!p || !sel || !this.list) return;
    if (!p.dragging) {
      if (Math.hypot(e.clientX - p.x, e.clientY - p.y) < DRAG_DISTANCE) return;
      p.dragging = true;
      // Dragging a page that is not picked picks it, like a file in a folder.
      if (!sel.has(p.page)) sel.only(p.page);
      this.sync();
      this.listRoot?.setAttribute("data-dragging", "");
      try {
        this.listRoot?.setPointerCapture(p.id);
      } catch {
        // A pointer that is gone cannot be captured; the drag ends with it.
      }
    }
    this.list.autoScroll(e.clientY);
    this.dropAt = this.list.insertionAt(e.clientY);
    const order = orderAfterMove(this.pages.length, sel.toArray(), this.dropAt);
    this.list.showDrop(isIdentity(order) ? null : this.dropAt);
  };

  private onPointerUp = (e: PointerEvent): void => {
    const p = this.press;
    this.press = null;
    this.listRoot?.removeAttribute("data-dragging");
    if (!p?.dragging) return;
    const sel = this.current();
    this.list?.showDrop(null);
    try {
      this.listRoot?.releasePointerCapture(p.id);
    } catch {
      // Nothing was captured.
    }
    // The click that ends the press must not pick the page again.
    this.suppressClick = true;
    setTimeout(() => (this.suppressClick = false), 0);
    if (!sel) return;
    const moved = sel.toArray();
    const order = orderAfterMove(this.pages.length, moved, this.dropAt);
    if (isIdentity(order)) return;
    e.preventDefault();
    void this.apply(this.ops.reorder(order, moved));
  };

  private onPointerCancel = (): void => {
    this.press = null;
    this.list?.showDrop(null);
    this.listRoot?.removeAttribute("data-dragging");
  };

  // ---- the tools -------------------------------------------------------------------------------

  private async apply(work: Promise<OpResult | null>): Promise<void> {
    const result = await work;
    if (result) {
      this.pick(result.select);
      const first = result.select[0];
      if (first !== undefined) this.list?.scrollToPage(first);
    } else {
      this.sync();
    }
  }

  private async moveBy(by: -1 | 1): Promise<void> {
    const sel = this.current();
    if (!sel || sel.size === 0) return;
    const picked = sel.toArray();
    const order = orderAfterMove(this.pages.length, picked, insertionForStep(this.pages.length, picked, by));
    if (isIdentity(order)) return;
    await this.apply(this.ops.reorder(order, picked));
  }

  private async turn(turns: 1 | 3): Promise<void> {
    const picked = this.picked();
    if (picked.length === 0) return;
    await this.apply(this.ops.rotate(picked, turns));
  }

  private async remove(): Promise<void> {
    const picked = this.picked();
    if (picked.length === 0) return;
    await this.apply(this.ops.remove(picked));
  }

  private async extract(): Promise<void> {
    const picked = this.picked();
    if (picked.length === 0) return;
    await this.ops.extract(picked);
    this.sync();
  }

  /** Where inserted pages go: after the last picked page, or after the page in view. */
  private insertionPlace(): number {
    const picked = this.picked();
    if (picked.length > 0) return picked[picked.length - 1] + 1;
    return Math.min(this.host.currentPage() + 1, this.pages.length);
  }

  private async addBlank(): Promise<void> {
    const at = this.insertionPlace();
    // The size of the page before it, or Letter.
    const near = this.pages[Math.max(0, Math.min(at - 1, this.pages.length - 1))];
    const fallback = BLANK_SIZES[0];
    const width = near ? Math.round(near.width) : fallback.width;
    const height = near ? Math.round(near.height) : fallback.height;
    await this.apply(this.ops.insertBlank(at, width, height));
  }

  private async addFiles(): Promise<void> {
    await this.apply(this.ops.insertFiles(this.insertionPlace()));
  }

  private async crop(): Promise<void> {
    const t = this.host.target();
    const picked = this.picked();
    if (!t || picked.length === 0) return;
    const first = picked[0];
    const size = t.pages[first];
    if (!size) return;
    const all = Array.from({ length: t.pages.length }, (_, i) => i);
    const scopes = [
      { id: "picked", label: picked.length === 1 ? `Page ${first + 1}` : `The ${pagesWord(picked.length)} picked`, pages: picked },
      ...(picked.length < t.pages.length ? [{ id: "all", label: `All ${pagesWord(t.pages.length)}`, pages: all }] : []),
    ];
    const answer = await askCrop({
      layer: this.host.layer,
      name: t.name,
      page: first,
      size,
      scopes,
      render: (w, h) => this.host.render(t.doc, first, w, h),
    });
    if (!answer) return;
    await this.apply(this.ops.crop(answer.pages, answer.margins));
  }
}

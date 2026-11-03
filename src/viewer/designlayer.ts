// The form designer on the pages: a box over each field the engine lists, handles on the picked one
// to move and resize it, and a surface that takes the pointer when a tool that places a field is in
// use. The engine draws the fields into the page picture; what is here takes the pointer and the
// keyboard and says what each box is to a screen reader.
//
// Boxes are placed with percentages of the page, so they stay right at any zoom without being
// placed again. Everything a field says about itself (its name, its choices) came from a file: it is
// used as text, never as markup.
//
// A move or a resize is shown while the pointer is down and told to the controller when it ends; the
// controller decides what to ask the engine and shows the result with `show`.

import { clientToPage, percentOf } from "../lib/annotations";
import {
  HANDLES,
  HANDLE_LABELS,
  SNAP_PIXELS,
  describeWidget,
  fieldLines,
  joinLines,
  keepOnPage,
  moveBox,
  noLines,
  normalize,
  pageLines,
  placeBox,
  resizeBox,
  sameSizeHints,
  snapMove,
  snapResize,
  type DesignKind,
  type DesignWidgetInfo,
  type Handle,
  type Rect,
  type SnapLines,
  type Snapped,
} from "../lib/formdesign";
import type { PageSizePt } from "./layout";

export type DesignTool = "select" | DesignKind;

export interface WidgetKey {
  name: string;
  widget: number;
}

/** Where the keyboard focus is on the picked box: on the box (`handle` null) or on one of its handles. */
export interface DesignFocus {
  handle: Handle | null;
}

export function sameKey(a: WidgetKey | null, b: WidgetKey | null): boolean {
  return a !== null && b !== null && a.name === b.name && a.widget === b.widget;
}

export interface DesignLayerEvents {
  /** A page is on the screen: show its widgets with `show`. */
  pageReady(page: number): void;
  /** A box for a new field was drawn, or the page was clicked with the tool of a field. */
  place(page: number, rect: Rect): void;
  /** A box was picked, or (with null) nothing is. */
  select(page: number, key: WidgetKey | null): void;
  /** A box was moved or resized. */
  transform(page: number, key: WidgetKey, rect: Rect): void;
  /** Delete on a picked box. */
  remove(page: number, key: WidgetKey): void;
  /** Enter on a picked box: the panel takes the focus. */
  edit(page: number, key: WidgetKey): void;
  /** The lines the text of a page gives to snap to. */
  textLines(page: number): Promise<SnapLines>;
  /** What a drag is doing, in words (a box that has the size of another). */
  hint?(text: string): void;
}

interface PageState {
  el: HTMLElement;
  root: HTMLElement;
  boxes: HTMLElement;
  guides: HTMLElement;
  ghost: HTMLElement;
  capture: HTMLElement;
  items: DesignWidgetInfo[];
  text: SnapLines | null;
}

interface Drag {
  page: number;
  pointerId: number;
  start: [number, number];
  /** The pointer at the start, in screen pixels. */
  client: [number, number];
  moved: boolean;
  key: WidgetKey;
  from: Rect;
  rect: Rect;
  handle: Handle | null;
  box: HTMLElement;
}

interface Placing {
  page: number;
  pointerId: number;
  start: [number, number];
  client: [number, number];
  moved: boolean;
  rect: Rect;
}

/** How far the pointer must move before a press on a box becomes a drag, in screen pixels. */
const DRAG_DISTANCE = 4;

const KEY_STEP = 1;
const KEY_STEP_BIG = 10;

export class DesignLayer {
  private readonly pages = new Map<number, PageState>();
  private tool: DesignTool = "select";
  private selected: { page: number; key: WidgetKey } | null = null;
  private held = false;
  private destroyed = false;
  private drag: Drag | null = null;
  private placing: Placing | null = null;

  constructor(
    private readonly sizes: PageSizePt[],
    private readonly events: DesignLayerEvents,
  ) {}

  // ---- the pages ---------------------------------------------------------------------------

  mount(page: number, pageEl: HTMLElement): void {
    if (this.destroyed) return;
    const root = document.createElement("div");
    root.className = "design-layer";
    root.dataset.tool = this.tool;
    root.inert = this.held;
    const boxes = document.createElement("div");
    boxes.className = "design-boxes";
    const guides = document.createElement("div");
    guides.className = "design-guides";
    guides.setAttribute("aria-hidden", "true");
    const ghost = document.createElement("div");
    ghost.className = "design-ghost";
    ghost.hidden = true;
    ghost.setAttribute("aria-hidden", "true");
    const capture = document.createElement("div");
    capture.className = "design-capture";
    capture.hidden = this.tool === "select";
    root.append(boxes, guides, ghost, capture);
    pageEl.append(root);
    const state: PageState = { el: pageEl, root, boxes, guides, ghost, capture, items: [], text: null };
    this.pages.set(page, state);
    capture.addEventListener("pointerdown", (e) => this.onPlaceDown(e, page));
    capture.addEventListener("pointermove", (e) => this.onPlaceMove(e, page));
    capture.addEventListener("pointerup", (e) => this.onPlaceUp(e, page));
    capture.addEventListener("pointercancel", () => this.cancelPlacing());
    this.events.pageReady(page);
    void this.events
      .textLines(page)
      .then((lines) => {
        if (!this.destroyed && this.pages.get(page) === state) state.text = lines;
      })
      .catch(() => undefined);
  }

  unmount(page: number): void {
    if (this.drag?.page === page) this.drag = null;
    if (this.placing?.page === page) this.placing = null;
    this.pages.get(page)?.root.remove();
    this.pages.delete(page);
  }

  /** Shows the widgets of a page. */
  show(page: number, items: DesignWidgetInfo[]): void {
    const state = this.pages.get(page);
    if (!state || this.destroyed) return;
    state.items = items;
    this.paint(page);
  }

  /** The widgets last shown on a page. */
  itemsOf(page: number): DesignWidgetInfo[] {
    return this.pages.get(page)?.items ?? [];
  }

  /** The pages that are on the screen. */
  get mountedPages(): number[] {
    return [...this.pages.keys()];
  }

  // ---- state -------------------------------------------------------------------------------

  setTool(tool: DesignTool): void {
    this.tool = tool;
    for (const state of this.pages.values()) {
      state.root.dataset.tool = tool;
      state.capture.hidden = tool === "select";
    }
    this.cancelPlacing();
  }

  get currentTool(): DesignTool {
    return this.tool;
  }

  /** A change to the document is under way: nothing is picked, moved or placed. */
  setHeld(held: boolean): void {
    this.held = held;
    for (const state of this.pages.values()) state.root.inert = held;
    if (held) {
      this.drag = null;
      this.cancelPlacing();
    }
  }

  get picked(): { page: number; key: WidgetKey } | null {
    return this.selected;
  }

  /** Picks a box (or, with null, none). Does not tell the controller. */
  select(page: number | null, key: WidgetKey | null): void {
    this.selected = page !== null && key !== null ? { page, key } : null;
    this.applyPicks();
  }

  /**
   * Puts the marks of the pick (and the handles) on the boxes as they are. The boxes themselves stay,
   * because a press on a box that is being dragged must not lose its element.
   */
  private applyPicks(): void {
    for (const [page, state] of this.pages) {
      for (const el of state.boxes.children) {
        if (!(el instanceof HTMLElement)) continue;
        const key: WidgetKey = { name: el.dataset.name ?? "", widget: Number(el.dataset.widget ?? "0") };
        const item = state.items.find((w) => w.name === key.name && w.widget === key.widget);
        if (!item) continue;
        this.mark(el, page, item, this.selected?.page === page && sameKey(this.selected.key, key));
      }
    }
  }

  private mark(box: HTMLElement, page: number, item: DesignWidgetInfo, picked: boolean): void {
    box.classList.toggle("design-box-picked", picked);
    // The box holds the handles, which are buttons: a button inside a button is not shown to a screen
    // reader, so the box is a group, and a picked one says so with its state.
    if (picked) box.setAttribute("aria-current", "true");
    else box.removeAttribute("aria-current");
    const handles = box.querySelectorAll(".design-handle");
    if (!picked || !item.editable) {
      for (const h of handles) h.remove();
      return;
    }
    if (handles.length > 0) return;
    for (const handle of HANDLES) {
      const h = document.createElement("div");
      h.className = `design-handle design-handle-${handle}`;
      h.dataset.handle = handle;
      h.tabIndex = 0;
      h.setAttribute("role", "button");
      h.setAttribute("aria-label", `Resize ${item.name} from the ${HANDLE_LABELS[handle]}`);
      h.addEventListener("pointerdown", (e) => this.onBoxDown(e, page, item, box, handle));
      h.addEventListener("keydown", (e) => this.onHandleKey(e, page, item, handle));
      box.append(h);
    }
  }

  /** Takes over what the layer it replaces held: the tool, the pick and the hold. */
  inherit(old: DesignLayer): void {
    this.tool = old.tool;
    this.selected = old.selected;
    this.held = old.held;
  }

  /** Puts the keyboard focus on the picked box. */
  focusPicked(): void {
    if (!this.selected) return;
    this.boxElement(this.selected.page, this.selected.key)?.focus({ preventScroll: true });
  }

  /** Where the keyboard focus is on the picked box, or null when it is not there. */
  focusState(): DesignFocus | null {
    if (!this.selected) return null;
    const box = this.boxElement(this.selected.page, this.selected.key);
    const active = document.activeElement;
    if (!box || !(active instanceof HTMLElement) || !box.contains(active)) return null;
    const handle = active.dataset.handle as Handle | undefined;
    return { handle: handle ?? null };
  }

  /** Gives the focus back to the picked box (or the handle of it) after the boxes were made again. Resolves with whether it could. */
  restoreFocus(state: DesignFocus): boolean {
    // While a change is under way the layer is inert and takes no focus; it is asked again when it is not.
    if (!this.selected || this.held) return false;
    const box = this.boxElement(this.selected.page, this.selected.key);
    if (!box) return false;
    const target = (state.handle ? box.querySelector<HTMLElement>(`[data-handle="${state.handle}"]`) : null) ?? box;
    target.focus({ preventScroll: true });
    return document.activeElement === target;
  }

  destroy(): void {
    this.destroyed = true;
    for (const state of this.pages.values()) state.root.remove();
    this.pages.clear();
    this.drag = null;
    this.placing = null;
  }

  // ---- drawing -----------------------------------------------------------------------------

  private boxElement(page: number, key: WidgetKey): HTMLElement | null {
    const state = this.pages.get(page);
    if (!state) return null;
    for (const el of state.boxes.children) {
      if (el instanceof HTMLElement && el.dataset.name === key.name && el.dataset.widget === String(key.widget)) return el;
    }
    return null;
  }

  private place(el: HTMLElement, page: number, rect: Rect): void {
    const size = this.sizes[page];
    el.style.left = percentOf(rect[0], size.width);
    el.style.top = percentOf(rect[1], size.height);
    el.style.width = percentOf(rect[2] - rect[0], size.width);
    el.style.height = percentOf(rect[3] - rect[1], size.height);
  }

  private paint(page: number): void {
    const state = this.pages.get(page);
    if (!state) return;
    const active = document.activeElement;
    const hadFocus = active instanceof HTMLElement && state.boxes.contains(active);
    const focusKey = hadFocus
      ? {
          name: (active as HTMLElement).closest<HTMLElement>(".design-box")?.dataset.name ?? "",
          widget: Number((active as HTMLElement).closest<HTMLElement>(".design-box")?.dataset.widget ?? "0"),
          handle: (active as HTMLElement).dataset.handle ?? null,
        }
      : null;
    state.boxes.replaceChildren();
    for (const item of state.items) {
      const key: WidgetKey = { name: item.name, widget: item.widget };
      const picked = this.selected?.page === page && sameKey(this.selected.key, key);
      const box = document.createElement("div");
      box.className = `design-box design-kind-${item.kind}`;
      if (!item.editable) box.classList.add("design-box-locked");
      if (picked) box.classList.add("design-box-picked");
      box.dataset.name = item.name;
      box.dataset.widget = String(item.widget);
      box.dataset.kind = item.kind;
      box.tabIndex = 0;
      box.setAttribute("role", "group");
      box.setAttribute("aria-label", describeWidget(item));
      this.place(box, page, item.rect);
      const label = document.createElement("span");
      label.className = "design-name";
      label.textContent = item.name;
      label.setAttribute("aria-hidden", "true");
      box.append(label);
      box.addEventListener("pointerdown", (e) => this.onBoxDown(e, page, item, box, null));
      box.addEventListener("pointermove", (e) => this.onDragMove(e));
      box.addEventListener("pointerup", (e) => this.onDragUp(e));
      box.addEventListener("pointercancel", () => this.cancelDrag());
      box.addEventListener("keydown", (e) => this.onBoxKey(e, page, item));
      this.mark(box, page, item, picked);
      state.boxes.append(box);
    }
    // A drag whose box was made again (the page was shown again meanwhile) has nothing to follow any
    // more: it ends, and the pointer is let go, so that the next press starts a new one.
    if (this.drag && this.drag.page === page && !state.boxes.contains(this.drag.box)) {
      this.drag = null;
      this.clearGuides(page);
      this.events.hint?.("");
    }
    if (focusKey) {
      const el = this.boxElement(page, { name: focusKey.name, widget: focusKey.widget });
      const target = focusKey.handle ? el?.querySelector<HTMLElement>(`[data-handle="${focusKey.handle}"]`) : el;
      (target ?? el)?.focus({ preventScroll: true });
    }
  }

  // ---- the pointer on a box ----------------------------------------------------------------

  private scale(page: number): { box: DOMRect; size: PageSizePt } | null {
    const state = this.pages.get(page);
    if (!state) return null;
    const box = state.el.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) return null;
    return { box, size: this.sizes[page] };
  }

  private linesFor(page: number, skip: WidgetKey | null): SnapLines {
    const state = this.pages.get(page);
    if (!state) return noLines();
    const others = state.items.filter((w) => !(skip && w.name === skip.name && w.widget === skip.widget)).map((w) => w.rect);
    return joinLines(pageLines(this.sizes[page]), fieldLines(others), state.text ?? noLines());
  }

  private onBoxDown(e: PointerEvent, page: number, item: DesignWidgetInfo, box: HTMLElement, handle: Handle | null): void {
    if (this.held || e.button !== 0 || this.drag) return;
    const s = this.scale(page);
    if (!s) return;
    e.stopPropagation();
    const key: WidgetKey = { name: item.name, widget: item.widget };
    const wasPicked = this.selected?.page === page && sameKey(this.selected.key, key);
    if (!wasPicked) {
      this.selected = { page, key };
      this.applyPicks();
      this.events.select(page, key);
    }
    if (!item.editable) return;
    const at = clientToPage(s.box, s.size, e.clientX, e.clientY);
    this.drag = {
      page,
      pointerId: e.pointerId,
      start: at,
      client: [e.clientX, e.clientY],
      moved: false,
      key,
      from: [...item.rect],
      rect: [...item.rect],
      handle,
      box,
    };
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
    // A press on a handle must not start a move of the box under it.
    e.preventDefault();
  }

  private onDragMove(e: PointerEvent): void {
    const d = this.drag;
    if (!d || e.pointerId !== d.pointerId) return;
    if (!d.moved && Math.hypot(e.clientX - d.client[0], e.clientY - d.client[1]) < DRAG_DISTANCE) return;
    d.moved = true;
    this.dragTo(d, e.clientX, e.clientY, e.altKey);
  }

  private dragTo(d: Drag, clientX: number, clientY: number, noSnap: boolean): void {
    const s = this.scale(d.page);
    if (!s) return;
    const now = clientToPage(s.box, s.size, clientX, clientY);
    const dx = now[0] - d.start[0];
    const dy = now[1] - d.start[1];
    const limit = SNAP_PIXELS / (s.box.width / s.size.width);
    const lines = noSnap ? noLines() : this.linesFor(d.page, d.key);
    let snapped: Snapped;
    if (d.handle) {
      const resized = resizeBox(d.from, d.handle, dx, dy, s.size);
      snapped = snapResize(resized, d.handle, lines, limit);
      snapped.rect = resizeBox(snapped.rect, d.handle, 0, 0, s.size);
    } else {
      const moved = moveBox(d.from, dx, dy, s.size);
      snapped = snapMove(moved, lines, limit);
      snapped.rect = moveBox(snapped.rect, 0, 0, s.size);
    }
    d.rect = snapped.rect;
    this.place(d.box, d.page, d.rect);
    this.drawGuides(d.page, snapped);
    const others = (this.pages.get(d.page)?.items ?? []).filter((w) => !sameKey({ name: w.name, widget: w.widget }, d.key)).map((w) => w.rect);
    const hints = sameSizeHints(d.rect, others);
    this.events.hint?.(hints.length ? `Same ${hints.join(" and ")} as another field` : "");
  }

  private onDragUp(e: PointerEvent): void {
    const d = this.drag;
    if (!d || e.pointerId !== d.pointerId) return;
    this.drag = null;
    (e.currentTarget as HTMLElement).releasePointerCapture?.(e.pointerId);
    this.clearGuides(d.page);
    this.events.hint?.("");
    if (!d.moved) return;
    const same = d.rect.every((v, i) => Math.abs(v - d.from[i]) < 0.01);
    if (same) {
      this.place(d.box, d.page, d.from);
      return;
    }
    this.events.transform(d.page, d.key, d.rect);
  }

  private cancelDrag(): void {
    const d = this.drag;
    if (!d) return;
    this.drag = null;
    this.place(d.box, d.page, d.from);
    this.clearGuides(d.page);
    this.events.hint?.("");
  }

  private drawGuides(page: number, snapped: Snapped): void {
    const state = this.pages.get(page);
    if (!state) return;
    const size = this.sizes[page];
    state.guides.replaceChildren();
    for (const g of snapped.guides) {
      const line = document.createElement("div");
      line.className = `design-guide design-guide-${g.axis} design-guide-${g.source}`;
      if (g.axis === "x") line.style.left = percentOf(g.value, size.width);
      else line.style.top = percentOf(g.value, size.height);
      state.guides.append(line);
    }
  }

  private clearGuides(page: number): void {
    this.pages.get(page)?.guides.replaceChildren();
  }

  // ---- the keyboard on a box ---------------------------------------------------------------

  private onBoxKey(e: KeyboardEvent, page: number, item: DesignWidgetInfo): void {
    if (this.held || e.target !== e.currentTarget) return;
    const key: WidgetKey = { name: item.name, widget: item.widget };
    const arrows: Record<string, [number, number]> = {
      ArrowLeft: [-1, 0],
      ArrowRight: [1, 0],
      ArrowUp: [0, -1],
      ArrowDown: [0, 1],
    };
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      if (!(this.selected?.page === page && sameKey(this.selected.key, key))) {
        this.selected = { page, key };
        this.applyPicks();
        this.events.select(page, key);
      } else if (e.key === "Enter") {
        this.events.edit(page, key);
      }
      return;
    }
    if (e.key === "Escape") {
      // A tool that places fields is still on (a group of radio buttons being made): Escape ends the
      // tool first, which the window does, not the pick.
      if (this.tool !== "select") return;
      e.preventDefault();
      e.stopPropagation();
      this.selected = null;
      this.applyPicks();
      this.events.select(page, null);
      return;
    }
    if (!item.editable) return;
    if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      e.stopPropagation();
      this.events.remove(page, key);
      return;
    }
    const arrow = arrows[e.key];
    if (arrow && !e.ctrlKey && !e.metaKey) {
      e.preventDefault();
      e.stopPropagation();
      const step = e.shiftKey ? KEY_STEP_BIG : KEY_STEP;
      const size = this.sizes[page];
      // Alt turns the arrows into a resize from the bottom right corner.
      const rect = e.altKey
        ? resizeBox(item.rect, "se", arrow[0] * step, arrow[1] * step, size)
        : moveBox(item.rect, arrow[0] * step, arrow[1] * step, size);
      if (rect.some((v, i) => Math.abs(v - item.rect[i]) > 0.001)) this.events.transform(page, key, rect);
    }
  }

  private onHandleKey(e: KeyboardEvent, page: number, item: DesignWidgetInfo, handle: Handle): void {
    if (this.held) return;
    const arrows: Record<string, [number, number]> = {
      ArrowLeft: [-1, 0],
      ArrowRight: [1, 0],
      ArrowUp: [0, -1],
      ArrowDown: [0, 1],
    };
    const arrow = arrows[e.key];
    if (!arrow) return;
    e.preventDefault();
    e.stopPropagation();
    const step = e.shiftKey ? KEY_STEP_BIG : KEY_STEP;
    const rect = resizeBox(item.rect, handle, arrow[0] * step, arrow[1] * step, this.sizes[page]);
    if (rect.some((v, i) => Math.abs(v - item.rect[i]) > 0.001)) {
      this.events.transform(page, { name: item.name, widget: item.widget }, rect);
    }
  }

  // ---- placing a new field -----------------------------------------------------------------

  private onPlaceDown(e: PointerEvent, page: number): void {
    if (this.held || e.button !== 0 || this.tool === "select" || this.placing) return;
    const s = this.scale(page);
    if (!s) return;
    e.preventDefault();
    e.stopPropagation();
    const at = clientToPage(s.box, s.size, e.clientX, e.clientY);
    this.placing = { page, pointerId: e.pointerId, start: at, client: [e.clientX, e.clientY], moved: false, rect: [at[0], at[1], at[0], at[1]] };
    (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
  }

  private onPlaceMove(e: PointerEvent, page: number): void {
    const p = this.placing;
    if (!p || p.page !== page || e.pointerId !== p.pointerId || this.tool === "select") return;
    const s = this.scale(page);
    if (!s) return;
    if (!p.moved && Math.hypot(e.clientX - p.client[0], e.clientY - p.client[1]) < DRAG_DISTANCE) return;
    p.moved = true;
    const now = clientToPage(s.box, s.size, e.clientX, e.clientY);
    const limit = SNAP_PIXELS / (s.box.width / s.size.width);
    const lines = e.altKey ? noLines() : this.linesFor(page, null);
    const a = snapPoint(p.start, lines, limit);
    const b = snapPoint(now, lines, limit);
    p.rect = normalize(a, b);
    const state = this.pages.get(page);
    if (!state) return;
    state.ghost.hidden = false;
    this.place(state.ghost, page, p.rect);
  }

  private onPlaceUp(e: PointerEvent, page: number): void {
    const p = this.placing;
    if (!p || p.page !== page || e.pointerId !== p.pointerId || this.tool === "select") return;
    this.placing = null;
    const state = this.pages.get(page);
    if (state) state.ghost.hidden = true;
    const s = this.scale(page);
    if (!s) return;
    const kind: DesignKind = this.tool;
    const end = clientToPage(s.box, s.size, e.clientX, e.clientY);
    const limit = SNAP_PIXELS / (s.box.width / s.size.width);
    const lines = e.altKey ? noLines() : this.linesFor(page, null);
    let rect: Rect;
    if (p.moved) {
      rect = placeBox(kind, snapPoint(p.start, lines, limit), snapPoint(end, lines, limit), s.size);
    } else {
      const placed = placeBox(kind, p.start, p.start, s.size);
      rect = keepOnPage(snapMove(placed, lines, limit).rect, s.size);
    }
    this.events.place(page, rect);
  }

  private cancelPlacing(): void {
    const p = this.placing;
    this.placing = null;
    if (p) {
      const state = this.pages.get(p.page);
      if (state) state.ghost.hidden = true;
    }
  }
}

/** A point moved onto the nearest line on each axis, within `limit`. */
export function snapPoint(point: [number, number], lines: SnapLines, limit: number): [number, number] {
  const pull = (v: number, set: { value: number }[]): number => {
    let best = v;
    let distance = limit + 1;
    for (const line of set) {
      const d = Math.abs(line.value - v);
      if (d <= limit && d < distance) {
        best = line.value;
        distance = d;
      }
    }
    return best;
  };
  return [pull(point[0], lines.xs), pull(point[1], lines.ys)];
}

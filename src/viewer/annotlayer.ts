// The annotations on the pages: a box over each one that can be pressed, picked and moved, the
// line that is being drawn with the pen, and the surface that takes the clicks of the note and stamp
// tools. The annotations themselves are drawn by the engine, into the page picture; what is here is
// the part that takes the pointer and the keyboard, and says what each one is to a screen reader.
//
// Everything about an annotation came from a file. Its text is used as text only: in labels, as
// the text of a tooltip, never as markup. Boxes are placed with percentages of the page, so they
// stay right at any zoom without being placed again.

import {
  type Annotation,
  type Rgba,
  type Tool,
  clientToPage,
  coversPage,
  describeAnnotation,
  onPage,
  percentOf,
  rgbaCss,
  simplifyStroke,
  snippet,
} from "../lib/annotations";
import type { PageSizePt } from "./layout";

export interface AnnotLayerEvents {
  /** A box was picked (or, with null, nothing is). */
  onSelect(page: number, id: number | null): void;
  /** The note or stamp tool was clicked on a page: the place in points, and where on the screen. */
  onPlace(page: number, x: number, y: number, client: { x: number; y: number }): void;
  /** A stroke of the pen is finished. The layer keeps showing it until `endStroke` is called. */
  onStroke(page: number, points: [number, number][]): void;
  /** A box was dragged by `dx`, `dy` points. */
  onMove(page: number, id: number, dx: number, dy: number): void;
  /** A box was double clicked. */
  onEdit(page: number, id: number): void;
  /** The list of a page could not be had. */
  onLoadFailed?(page: number, error: unknown): void;
}

interface PageLayer {
  root: HTMLElement;
  boxes: HTMLElement;
  svg: SVGSVGElement;
  line: SVGPolylineElement;
  capture: HTMLElement;
  el: HTMLElement;
}

/** How far the pointer must move before a press becomes a drag, in CSS pixels. */
const DRAG_DISTANCE = 4;

const SVG_NS = "http://www.w3.org/2000/svg";

export class AnnotationLayer {
  private readonly layers = new Map<number, PageLayer>();
  private readonly data = new Map<number, Annotation[]>();
  private readonly loading = new Map<number, Promise<void>>();
  private tool: Tool = "select";
  /** Reading: the boxes take no clicks and no focus, so links and text work as they do without them. */
  private reading = false;
  private selected: { page: number; id: number } | null = null;
  private ink: { color: Rgba; width: number } = { color: [20, 20, 22, 255], width: 3 };
  private stroke: { page: number; pointerId: number; points: [number, number][] } | null = null;
  private placing: { page: number; pointerId: number; x: number; y: number } | null = null;
  private destroyed = false;
  /** A change to the pages is under way: nothing is asked for and nothing is picked or drawn. */
  private held = false;

  constructor(
    private readonly pages: PageSizePt[],
    private readonly load: ((page: number) => Promise<Annotation[]>) | null,
    private readonly events: AnnotLayerEvents,
  ) {}

  // ---- the pages ---------------------------------------------------------------

  mount(page: number, pageEl: HTMLElement): void {
    const size = this.pages[page];
    const root = document.createElement("div");
    root.className = "annot-layer";
    root.dataset.tool = this.tool;
    const boxes = document.createElement("div");
    boxes.className = "annot-boxes";
    boxes.inert = this.reading || this.held;
    const svg = document.createElementNS(SVG_NS, "svg");
    svg.setAttribute("class", "annot-ink");
    svg.setAttribute("viewBox", `0 0 ${size.width} ${size.height}`);
    svg.setAttribute("preserveAspectRatio", "none");
    svg.setAttribute("aria-hidden", "true");
    const line = document.createElementNS(SVG_NS, "polyline");
    line.setAttribute("fill", "none");
    line.setAttribute("stroke-linecap", "round");
    line.setAttribute("stroke-linejoin", "round");
    svg.append(line);
    const capture = document.createElement("div");
    capture.className = "annot-capture";
    capture.inert = this.held;
    root.append(boxes, svg, capture);
    pageEl.append(root);
    const layer: PageLayer = { root, boxes, svg, line, capture, el: pageEl };
    this.layers.set(page, layer);
    capture.addEventListener("pointerdown", (e) => this.onCaptureDown(e, page));
    capture.addEventListener("pointermove", (e) => this.onCaptureMove(e, page));
    capture.addEventListener("pointerup", (e) => this.onCaptureUp(e, page));
    capture.addEventListener("pointercancel", (e) => this.onCaptureCancel(e, page));
    this.paint(page);
  }

  unmount(page: number): void {
    if (this.stroke?.page === page) this.stroke = null;
    if (this.placing?.page === page) this.placing = null;
    this.layers.get(page)?.root.remove();
    this.layers.delete(page);
    // What was fetched for a page that is off screen is not kept: it is fetched again when the page
    // comes back, so a long document does not pile it up.
    this.data.delete(page);
  }

  /** The picture of a page was drawn: now the list of its annotations is asked for. */
  pageDrawn(page: number): void {
    if (this.destroyed || this.held || !this.load || this.data.has(page) || this.loading.has(page)) return;
    void this.reload(page);
  }

  /** Asks for the list of a page again and shows it. Resolves when it is shown. */
  reload(page: number): Promise<void> {
    if (!this.load || this.destroyed || this.held) return Promise.resolve();
    const load = this.load;
    const pending = load(page)
      .then((list) => {
        // The page may have scrolled away while the engine was answering.
        if (this.destroyed || !this.layers.has(page)) return;
        this.data.set(page, list);
        if (this.selected?.page === page && !list.some((a) => a.id === this.selected?.id)) {
          this.selected = null;
          this.events.onSelect(page, null);
        }
        this.paint(page);
      })
      .catch((error) => this.events.onLoadFailed?.(page, error))
      .finally(() => {
        if (this.loading.get(page) === pending) this.loading.delete(page);
      });
    this.loading.set(page, pending);
    return pending;
  }

  list(page: number): readonly Annotation[] {
    return this.data.get(page) ?? [];
  }

  find(page: number, id: number): Annotation | undefined {
    return this.data.get(page)?.find((a) => a.id === id);
  }

  /** The box of an annotation, if its page is on screen. */
  boxElement(page: number, id: number): HTMLElement | null {
    return this.layers.get(page)?.boxes.querySelector<HTMLElement>(`[data-id="${id}"]`) ?? null;
  }

  destroy(): void {
    this.destroyed = true;
    for (const layer of this.layers.values()) layer.root.remove();
    this.layers.clear();
    this.data.clear();
    this.loading.clear();
  }

  // ---- state from outside ------------------------------------------------------

  setTool(tool: Tool): void {
    this.tool = tool;
    for (const layer of this.layers.values()) layer.root.dataset.tool = tool;
    if (this.stroke) this.cancelStroke();
  }

  /**
   * Takes the tool, the reading mode and the pen of the layer this one replaces (the pages changed).
   * The pick is not taken: it names a page and an id that may stand for something else now.
   */
  inherit(from: AnnotationLayer): void {
    this.setTool(from.tool);
    this.setReading(from.reading);
    this.ink = from.ink;
  }

  /**
   * Lets the layer wait while the pages of the document are being changed: the boxes and the page
   * take no pointer, a stroke in progress is dropped, and no list is asked for (page numbers may
   * mean other pages by the time the engine answers). Going on asks for what was left.
   */
  setHeld(held: boolean): void {
    if (this.held === held) return;
    this.held = held;
    for (const layer of this.layers.values()) {
      layer.boxes.inert = this.reading || held;
      layer.capture.inert = held;
    }
    if (held) {
      if (this.stroke) this.cancelStroke();
      this.placing = null;
    } else {
      for (const page of this.layers.keys()) this.pageDrawn(page);
    }
  }

  /** While reading, the boxes are inert: no pointer, no Tab stop, nothing for a screen reader to land on. */
  setReading(reading: boolean): void {
    this.reading = reading;
    for (const layer of this.layers.values()) layer.boxes.inert = reading || this.held;
    if (reading && this.stroke) this.cancelStroke();
  }

  setSelected(selected: { page: number; id: number } | null): void {
    this.selected = selected;
    for (const [page, layer] of this.layers) {
      for (const box of layer.boxes.children) {
        const el = box as HTMLElement;
        const on = selected !== null && selected.page === page && el.dataset.id === String(selected.id);
        el.setAttribute("aria-pressed", String(on));
      }
    }
  }

  setInk(color: Rgba, width: number): void {
    this.ink = { color, width };
  }

  /** The drawing of a stroke is over (the engine has it now, or it failed): the line goes. */
  endStroke(page: number): void {
    const layer = this.layers.get(page);
    layer?.line.removeAttribute("points");
  }

  // ---- painting ----------------------------------------------------------------

  private paint(page: number): void {
    const layer = this.layers.get(page);
    if (!layer || this.destroyed) return;
    const size = this.pages[page];
    const focused = document.activeElement instanceof HTMLElement ? document.activeElement.dataset.id : undefined;
    const hadFocus = focused !== undefined && layer.boxes.contains(document.activeElement);
    const boxes = (this.data.get(page) ?? []).map((a) => this.box(page, a, size));
    layer.boxes.replaceChildren(...boxes);
    if (hadFocus) layer.boxes.querySelector<HTMLElement>(`[data-id="${focused}"]`)?.focus({ preventScroll: true });
  }

  private box(page: number, a: Annotation, size: PageSizePt): HTMLElement {
    const [l, t, r, b] = onPage(a.rect, size);
    const el = document.createElement("button");
    el.type = "button";
    el.className = "annot";
    el.dataset.id = String(a.id);
    el.dataset.kind = a.kind;
    // A box over most of the page takes no pointer: it stays reachable from the keyboard.
    if (coversPage([l, t, r, b], size)) el.dataset.big = "true";
    el.style.left = percentOf(l, size.width);
    el.style.top = percentOf(t, size.height);
    el.style.width = percentOf(Math.max(r - l, 0), size.width);
    el.style.height = percentOf(Math.max(b - t, 0), size.height);
    const label = describeAnnotation(a, page);
    el.setAttribute("aria-label", label);
    el.setAttribute("aria-pressed", String(this.selected?.page === page && this.selected.id === a.id));
    // The tooltip is the text of the annotation, cut. It is set as text, never as markup.
    el.title = snippet(a.text) || label;
    el.addEventListener("focus", () => this.events.onSelect(page, a.id));
    el.addEventListener("pointerdown", (e) => this.onBoxDown(e, page, a, el));
    el.addEventListener("dblclick", () => this.events.onEdit(page, a.id));
    return el;
  }

  // ---- picking and moving a box ------------------------------------------------

  private onBoxDown(e: PointerEvent, page: number, a: Annotation, el: HTMLElement): void {
    if (e.button !== 0 || this.tool !== "select") return;
    // The press is for the annotation: it must not start a selection of text under it.
    e.stopPropagation();
    this.events.onSelect(page, a.id);
    el.focus({ preventScroll: true });
    if (!a.can.move) return;
    const layer = this.layers.get(page);
    if (!layer) return;
    const start = { x: e.clientX, y: e.clientY };
    let moved = false;
    const move = (m: PointerEvent) => {
      const dx = m.clientX - start.x;
      const dy = m.clientY - start.y;
      if (!moved && Math.hypot(dx, dy) < DRAG_DISTANCE) return;
      moved = true;
      el.style.transform = `translate(${dx}px, ${dy}px)`;
    };
    const done = (u: PointerEvent, cancelled: boolean) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      el.style.transform = "";
      if (!moved || cancelled) return;
      const box = layer.el.getBoundingClientRect();
      const size = this.pages[page];
      const [x0, y0] = clientToPage(box, size, start.x, start.y);
      const [x1, y1] = clientToPage(box, size, u.clientX, u.clientY);
      this.events.onMove(page, a.id, x1 - x0, y1 - y0);
    };
    const up = (u: PointerEvent) => done(u, false);
    const cancel = (u: PointerEvent) => done(u, true);
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
  }

  // ---- the pen, and the note and stamp tools -------------------------------------

  private toPage(page: number, e: PointerEvent): [number, number] {
    const layer = this.layers.get(page);
    if (!layer) return [0, 0];
    return clientToPage(layer.el.getBoundingClientRect(), this.pages[page], e.clientX, e.clientY);
  }

  private onCaptureDown(e: PointerEvent, page: number): void {
    if (e.button !== 0 || this.stroke || this.placing) return;
    const layer = this.layers.get(page);
    if (!layer) return;
    e.preventDefault();
    e.stopPropagation();
    try {
      layer.capture.setPointerCapture(e.pointerId);
    } catch {
      // The pointer is followed while it stays over the page.
    }
    if (this.tool === "pen") {
      this.stroke = { page, pointerId: e.pointerId, points: [this.toPage(page, e)] };
      layer.line.setAttribute("stroke", rgbaCss(this.ink.color));
      layer.line.setAttribute("stroke-width", String(this.ink.width));
      this.showStroke(layer);
    } else if (this.tool === "note" || this.tool === "stamp") {
      this.placing = { page, pointerId: e.pointerId, x: e.clientX, y: e.clientY };
    }
  }

  private onCaptureMove(e: PointerEvent, page: number): void {
    const stroke = this.stroke;
    if (!stroke || stroke.page !== page || stroke.pointerId !== e.pointerId) return;
    const layer = this.layers.get(page);
    if (!layer) return;
    // A fast pen reports many positions between two events; they are all part of the line.
    const batch = typeof e.getCoalescedEvents === "function" ? e.getCoalescedEvents() : [];
    for (const c of batch.length > 0 ? batch : [e]) stroke.points.push(this.toPage(page, c));
    this.showStroke(layer);
  }

  private showStroke(layer: PageLayer): void {
    if (!this.stroke) return;
    const pts = this.stroke.points;
    // A single point is drawn as a dot, by a line of no length with round caps.
    const shown = pts.length === 1 ? [pts[0], [pts[0][0] + 0.01, pts[0][1]]] : pts;
    layer.line.setAttribute("points", shown.map((p) => `${p[0].toFixed(2)},${p[1].toFixed(2)}`).join(" "));
  }

  private onCaptureUp(e: PointerEvent, page: number): void {
    const layer = this.layers.get(page);
    try {
      layer?.capture.releasePointerCapture(e.pointerId);
    } catch {
      // Nothing was captured.
    }
    const stroke = this.stroke;
    if (stroke && stroke.page === page && stroke.pointerId === e.pointerId) {
      this.stroke = null;
      this.events.onStroke(page, simplifyStroke(stroke.points));
      return;
    }
    const placing = this.placing;
    if (placing && placing.page === page && placing.pointerId === e.pointerId) {
      this.placing = null;
      // A press that moved is a drag over the page, not a click.
      if (Math.hypot(e.clientX - placing.x, e.clientY - placing.y) >= DRAG_DISTANCE * 2) return;
      const [x, y] = this.toPage(page, e);
      this.events.onPlace(page, x, y, { x: e.clientX, y: e.clientY });
    }
  }

  private onCaptureCancel(e: PointerEvent, page: number): void {
    if (this.stroke?.pointerId === e.pointerId) this.cancelStroke();
    if (this.placing?.pointerId === e.pointerId) this.placing = null;
    void page;
  }

  private cancelStroke(): void {
    const page = this.stroke?.page;
    this.stroke = null;
    if (page !== undefined) this.endStroke(page);
  }

  /** The user pressed Escape in the middle of a stroke. */
  abort(): boolean {
    if (!this.stroke && !this.placing) return false;
    this.placing = null;
    this.cancelStroke();
    return true;
  }
}

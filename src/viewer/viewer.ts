// The continuous page view of one document.
//
// Every page gets a box of the right size from the start, so the scroll bar is correct at once.
// Only the pages near the viewport get a canvas, and only those are asked from the engine. Pages
// that scroll far away lose their canvas again, so memory stays flat however long the document is.
// One DocumentView is one document; the app keeps one per tab. A view that is not shown frees all
// its canvases and asks the engine for nothing, and remembers where the reader was.

import { type AppErrorLike, isEngineLost, toAppError } from "../lib/errors";
import { type Action, pageScrollDistance } from "./keys";
import {
  type Anchor,
  type Layout,
  type PageSizePt,
  anchorAt,
  bitmapSize,
  computeLayout,
  currentPage,
  scrollTopForAnchor,
  scrollTopForPage,
  visibleRange,
} from "./layout";
import { PageOverlays, type CopyResult, type OverlayApi, type OverlayLimits, type PageLinkInfo } from "./overlays";
import { RenderScheduler, type RenderJob } from "./scheduler";
import type { SearchMatch } from "./search";
import type { Rect } from "./textlayer";
import { type ZoomMode, clampZoom, fitWidthZoom, stepZoom } from "./zoom";

export interface ViewerApi extends OverlayApi {
  render(doc: number, page: number, width: number, height: number): Promise<ArrayBuffer>;
}

export interface ViewerEvents {
  onPageChange(page: number): void;
  onZoomChange(zoom: number, mode: ZoomMode): void;
  /** The engine was lost: the document cannot be drawn any more. */
  onFatal(error: AppErrorLike): void;
  /** The first page has been drawn. */
  onFirstPaint(): void;
  /** The reading position changed (scrolled or zoomed). Called often: the receiver should wait. */
  onViewChange?(): void;
  /** A link on a page was pressed. Nothing has been opened: the receiver decides what to do. */
  onLink?(link: PageLinkInfo, page: number): void;
  /** Text was selected, or the selection was cleared. */
  onSelectionChange?(hasSelection: boolean): void;
  /** The selection is final for now (the pointer was released): its text can be taken. */
  onSelectionSettled?(): void;
}

/** Where the reader was: a page, how far down it, and the zoom (null means fit width). */
export interface ViewPosition {
  page: number;
  /** 0 at the top of the page, 1 at the bottom. */
  offset: number;
  zoom: number | null;
}

export interface ViewerOptions {
  /** Where to start. Without it the view starts at the top, fitted to the width. */
  start?: ViewPosition | null;
  /** Limits on the text kept for the document. For tests; the defaults are meant for the app. */
  limits?: Partial<OverlayLimits>;
}

export interface DocumentInfo {
  doc: number;
  pages: PageSizePt[];
}

interface Mounted {
  el: HTMLElement;
  canvas: HTMLCanvasElement;
  /** The bitmap size the page wants now. */
  wantKey: string;
  /** The bitmap size on the canvas, or null while the canvas is empty. */
  drawnKey: string | null;
  failedKey: string | null;
}

const WHEEL_ZOOM_THRESHOLD = 40;

export class DocumentView {
  readonly element: HTMLElement;
  private readonly sizer: HTMLElement;
  private readonly mounted = new Map<number, Mounted>();
  private readonly scheduler: RenderScheduler;
  private readonly overlays: PageOverlays;
  private readonly resizeObserver?: ResizeObserver;
  private layout: Layout;
  private zoom = 1;
  private mode: ZoomMode = { kind: "fit-width" };
  private frame = 0;
  private lastPage = -1;
  private wheelAccumulator = 0;
  private painted = false;
  private destroyed = false;
  private fatal = false;
  private active = true;
  private started = false;
  /** Where the reader was when the view was hidden, and how far to the side. */
  private hiddenAnchor: Anchor = { page: 0, fraction: 0 };
  private hiddenLeft = 0;

  constructor(
    private readonly info: DocumentInfo,
    private readonly api: ViewerApi,
    private readonly events: ViewerEvents,
    private readonly options: ViewerOptions = {},
  ) {
    this.element = document.createElement("div");
    this.element.className = "viewer";
    this.element.tabIndex = 0;
    this.element.setAttribute("role", "document");
    this.sizer = document.createElement("div");
    this.sizer.className = "viewer-pages";
    this.element.append(this.sizer);

    this.layout = computeLayout(info.pages, this.zoom);
    this.scheduler = new RenderScheduler({
      concurrency: 2,
      run: (job) => this.renderJob(job),
      onError: (job, error) => this.renderFailed(job, error),
    });

    this.overlays = new PageOverlays(
      info.doc,
      info.pages,
      api,
      {
        element: this.element,
        pageElement: (page) => this.mounted.get(page)?.el ?? null,
        mountedPages: () => [...this.mounted.keys()],
        scrollToRect: (page, rect) => this.scrollToRect(page, rect),
      },
      {
        onLink: (link, page) => events.onLink?.(link, page),
        onSelectionChange: (has) => events.onSelectionChange?.(has),
        onSelectionSettled: () => events.onSelectionSettled?.(),
      },
      options.limits,
    );

    this.element.addEventListener("scroll", this.onScroll, { passive: true });
    this.element.addEventListener("wheel", this.onWheel, { passive: false });
    if (typeof ResizeObserver !== "undefined") {
      this.resizeObserver = new ResizeObserver(() => this.onResize());
      this.resizeObserver.observe(this.element);
    }
    window.addEventListener("resize", this.onWindowResize);
  }

  get pageCount(): number {
    return this.info.pages.length;
  }

  get currentPageIndex(): number {
    return Math.max(0, this.lastPage);
  }

  get zoomValue(): number {
    return this.zoom;
  }

  get zoomMode(): ZoomMode {
    return this.mode;
  }

  /** Call once the element is in the page, so its size is known. */
  start(): void {
    this.started = true;
    const start = this.options.start;
    if (start && this.pageCount > 0) {
      this.mode = start.zoom === null ? { kind: "fit-width" } : { kind: "custom", zoom: clampZoom(start.zoom) };
      this.applyMode();
      const page = Math.min(Math.max(0, Math.floor(start.page)), this.pageCount - 1);
      const offset = Number.isFinite(start.offset) ? Math.min(Math.max(start.offset, 0), 1) : 0;
      // The first pages asked from the engine are the ones at the saved place, not page 1.
      this.relayout({ page, fraction: offset });
    } else {
      this.applyMode();
      this.relayout();
    }
    this.element.focus({ preventScroll: true });
  }

  get isActive(): boolean {
    return this.active;
  }

  /** True once the first page has been drawn. */
  get hasPainted(): boolean {
    return this.painted;
  }

  /** True once the pages have been laid out (a view opened behind another waits until shown). */
  get hasStarted(): boolean {
    return this.started;
  }

  /** The reading position, to be saved and restored later. */
  getPosition(): ViewPosition {
    if (!this.started) {
      return this.options.start ?? { page: 0, offset: 0, zoom: null };
    }
    let anchor = this.active ? anchorAt(this.layout, this.element.scrollTop) : this.hiddenAnchor;
    // The gap between two pages belongs to the page below it.
    if (anchor.fraction >= 1 && anchor.page + 1 < this.pageCount) {
      anchor = { page: anchor.page + 1, fraction: 0 };
    }
    return {
      page: anchor.page,
      offset: Math.min(Math.max(anchor.fraction, 0), 1),
      zoom: this.mode.kind === "fit-width" ? null : this.zoom,
    };
  }

  /**
   * Shows or hides the view (a tab comes to the front or goes behind). A hidden view gives up its
   * canvases and asks the engine for nothing; when it is shown again it comes back where it was.
   */
  setActive(active: boolean): void {
    if (this.destroyed || active === this.active) return;
    this.active = active;
    if (!active) {
      if (this.started) {
        this.hiddenAnchor = anchorAt(this.layout, this.element.scrollTop);
        this.hiddenLeft = this.element.scrollLeft;
        this.scheduler.setWanted([]);
        for (const index of [...this.mounted.keys()]) this.unmount(index);
        this.overlays.releaseText();
      }
      this.element.hidden = true;
      return;
    }
    this.element.hidden = false;
    if (!this.started) {
      this.start();
      return;
    }
    this.applyMode();
    this.relayout(this.hiddenAnchor);
    this.element.scrollLeft = this.hiddenLeft;
    this.element.focus({ preventScroll: true });
  }

  destroy(): void {
    this.destroyed = true;
    this.scheduler.stop();
    cancelAnimationFrame(this.frame);
    this.resizeObserver?.disconnect();
    window.removeEventListener("resize", this.onWindowResize);
    for (const index of [...this.mounted.keys()]) this.unmount(index);
    this.overlays.destroy();
    this.element.remove();
  }

  // ---- commands ------------------------------------------------------------

  goToPage(index: number): void {
    if (this.pageCount === 0) return;
    this.element.scrollTop = scrollTopForPage(this.layout, index);
    this.refresh();
  }

  setZoom(zoom: number): void {
    this.mode = { kind: "custom", zoom: clampZoom(zoom) };
    this.applyMode();
    this.relayout();
  }

  setFitWidth(): void {
    this.mode = { kind: "fit-width" };
    this.applyMode();
    this.relayout();
  }

  stepZoom(direction: 1 | -1): void {
    this.setZoom(stepZoom(this.zoom, direction));
  }

  /** Runs a keyboard action that belongs to the viewer. Returns true when it was handled here. */
  handleAction(action: Action): boolean {
    const el = this.element;
    switch (action.type) {
      case "scroll-by":
        el.scrollLeft += action.x;
        el.scrollTop += action.y;
        return true;
      case "scroll-page":
        el.scrollTop += action.direction * pageScrollDistance(el.clientHeight);
        return true;
      case "go-first":
        el.scrollTop = 0;
        return true;
      case "go-last":
        el.scrollTop = this.layout.totalHeight;
        return true;
      case "zoom-in":
        this.stepZoom(1);
        return true;
      case "zoom-out":
        this.stepZoom(-1);
        return true;
      case "zoom-fit-width":
        this.setFitWidth();
        return true;
      case "zoom-actual":
        this.setZoom(1);
        return true;
      default:
        return false;
    }
  }

  // ---- selection, search and links -------------------------------------------

  get hasSelection(): boolean {
    return this.overlays.hasSelection;
  }

  selectAll(): void {
    this.overlays.selectAll();
  }

  clearSelection(): void {
    this.overlays.clearSelection();
  }

  /** The selected text from the pages that are loaded, and the pages that are not. */
  selectedText(): { text: string; missing: number[] } {
    return this.overlays.selectedText();
  }

  /** The selected text for a Copy, loading the pages it still lacks, and what it could not do. */
  loadSelectedText(): Promise<CopyResult> {
    return this.overlays.loadSelectedText();
  }

  /** Marks search matches on the pages. `current` is the one picked out. */
  setFind(matches: ReadonlyMap<number, readonly [number, number][]>, current: SearchMatch | null): void {
    this.overlays.setFind(matches, current);
  }

  clearFind(): void {
    this.overlays.clearFind();
  }

  /** Scrolls so that a match is in view. */
  revealMatch(match: SearchMatch): Promise<void> {
    return this.overlays.revealMatch(match);
  }

  /** Scrolls so that a box of a page (in points from its top left) is in view. */
  private scrollToRect(page: number, rect: Rect): void {
    if (!this.active || page < 0 || page >= this.pageCount) return;
    const size = this.info.pages[page];
    const w = size.width || 1;
    const h = size.height || 1;
    const top = this.layout.tops[page] + (rect.top / h) * this.layout.heights[page];
    const bottom = this.layout.tops[page] + (rect.bottom / h) * this.layout.heights[page];
    const containerWidth = Math.max(this.layout.contentWidth, this.element.clientWidth);
    const pageLeft = Math.round((containerWidth - this.layout.widths[page]) / 2);
    const left = pageLeft + (rect.left / w) * this.layout.widths[page];
    const right = pageLeft + (rect.right / w) * this.layout.widths[page];
    const el = this.element;
    const edge = 48;
    if (top < el.scrollTop + edge || bottom > el.scrollTop + el.clientHeight - edge) {
      el.scrollTop = Math.max(0, Math.round(top - el.clientHeight / 3));
    }
    if (left < el.scrollLeft + edge || right > el.scrollLeft + el.clientWidth - edge) {
      el.scrollLeft = Math.max(0, Math.round(left - el.clientWidth / 3));
    }
    this.refresh();
  }

  // ---- layout --------------------------------------------------------------

  private widestPagePt(): number {
    let widest = 0;
    for (const page of this.info.pages) widest = Math.max(widest, page.width);
    return widest;
  }

  /** Sets `zoom` from the zoom mode. */
  private applyMode(): void {
    if (this.mode.kind === "fit-width") {
      // The vertical scroll bar takes room away from the pages; clientWidth already excludes it.
      this.zoom = fitWidthZoom(this.widestPagePt(), this.element.clientWidth);
    } else {
      this.zoom = this.mode.zoom;
    }
  }

  /** Lays the pages out again at the current zoom, keeping the reading position (or going to `place`). */
  private relayout(place?: Anchor): void {
    const anchor: Anchor = place ?? anchorAt(this.layout, this.element.scrollTop);
    this.layout = computeLayout(this.info.pages, this.zoom);
    this.sizer.style.height = `${this.layout.totalHeight}px`;
    this.sizer.style.width = `${Math.max(this.layout.contentWidth, this.element.clientWidth)}px`;
    for (const [index, m] of this.mounted) this.placePage(index, m);
    this.element.scrollTop = scrollTopForAnchor(this.layout, anchor);
    this.events.onZoomChange(this.zoom, this.mode);
    this.refresh();
  }

  private placePage(index: number, m: Mounted): void {
    const width = this.layout.widths[index];
    const containerWidth = Math.max(this.layout.contentWidth, this.element.clientWidth);
    m.el.style.top = `${this.layout.tops[index]}px`;
    m.el.style.left = `${Math.round((containerWidth - width) / 2)}px`;
    m.el.style.width = `${width}px`;
    m.el.style.height = `${this.layout.heights[index]}px`;
  }

  private onScroll = (): void => {
    this.scheduleRefresh();
  };

  private onResize(): void {
    // A hidden view has no size; fitting to it would shrink the pages to nothing.
    if (this.destroyed || !this.active) return;
    if (this.mode.kind === "fit-width") {
      const before = this.zoom;
      this.applyMode();
      if (this.zoom !== before) {
        this.relayout();
        return;
      }
    }
    this.sizer.style.width = `${Math.max(this.layout.contentWidth, this.element.clientWidth)}px`;
    for (const [index, m] of this.mounted) this.placePage(index, m);
    this.scheduleRefresh();
  }

  /** The pixel ratio can change when the window moves to another screen. */
  private onWindowResize = (): void => {
    this.scheduleRefresh();
  };

  private onWheel = (e: WheelEvent): void => {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    this.wheelAccumulator += e.deltaY;
    if (Math.abs(this.wheelAccumulator) >= WHEEL_ZOOM_THRESHOLD) {
      this.stepZoom(this.wheelAccumulator < 0 ? 1 : -1);
      this.wheelAccumulator = 0;
    }
  };

  // ---- pages ---------------------------------------------------------------

  private scheduleRefresh(): void {
    if (this.frame || this.destroyed || !this.active) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.refresh();
    });
  }

  /** Mounts the pages near the viewport, unmounts the others, and queues what needs drawing. */
  private refresh(): void {
    if (this.destroyed || !this.active) return;
    const top = this.element.scrollTop;
    const height = this.element.clientHeight;
    // Keep one viewport of pages above and below ready, so a scroll rarely shows an empty page.
    const range = visibleRange(this.layout, top, height, height);

    for (const index of [...this.mounted.keys()]) {
      if (!range || index < range.first || index > range.last) this.unmount(index);
    }

    const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
    const jobs: { job: RenderJob; distance: number }[] = [];
    const center = top + height / 2;
    if (range) {
      for (let index = range.first; index <= range.last; index++) {
        const m = this.mounted.get(index) ?? this.mount(index);
        const size = bitmapSize(this.info.pages[index], this.zoom, dpr);
        const key = `${index}:${size.width}x${size.height}`;
        m.wantKey = key;
        if (m.drawnKey === key || m.failedKey === key) continue;
        const pageCenter = this.layout.tops[index] + this.layout.heights[index] / 2;
        jobs.push({
          job: { key, page: index, width: size.width, height: size.height },
          distance: Math.abs(pageCenter - center),
        });
      }
    }
    jobs.sort((a, b) => a.distance - b.distance);
    this.scheduler.setWanted(jobs.map((j) => j.job));

    const page = this.pageCount === 0 ? 0 : currentPage(this.layout, top, height);
    if (page !== this.lastPage) {
      this.lastPage = page;
      this.events.onPageChange(page);
    }
    this.events.onViewChange?.();
  }

  private mount(index: number): Mounted {
    const el = document.createElement("div");
    el.className = "viewer-page";
    el.dataset.page = String(index);
    const canvas = document.createElement("canvas");
    canvas.className = "viewer-canvas";
    el.append(canvas);
    this.sizer.append(el);
    const m: Mounted = { el, canvas, wantKey: "", drawnKey: null, failedKey: null };
    this.mounted.set(index, m);
    this.placePage(index, m);
    this.overlays.mount(index, el);
    return m;
  }

  private unmount(index: number): void {
    const m = this.mounted.get(index);
    if (!m) return;
    // Shrinking the canvas frees its bitmap right away instead of when the garbage collector runs.
    m.canvas.width = 0;
    m.canvas.height = 0;
    this.overlays.unmount(index);
    m.el.remove();
    this.mounted.delete(index);
  }

  private async renderJob(job: RenderJob): Promise<void> {
    if (this.destroyed) return;
    const buffer = await this.api.render(this.info.doc, job.page, job.width, job.height);
    const m = this.mounted.get(job.page);
    // The page scrolled away or changed size while the engine was drawing: drop the result.
    if (this.destroyed || !m || m.wantKey !== job.key) return;
    if (buffer.byteLength !== job.width * job.height * 4) {
      throw { code: "render_failed", message: "The engine returned a bitmap of the wrong size." };
    }
    const ctx = m.canvas.getContext("2d");
    if (!ctx) throw { code: "internal", message: "The canvas is not available." };
    m.canvas.width = job.width;
    m.canvas.height = job.height;
    ctx.putImageData(new ImageData(new Uint8ClampedArray(buffer), job.width, job.height), 0, 0);
    m.drawnKey = job.key;
    m.el.classList.remove("viewer-page-failed");
    // Text and links come after the picture, so they never delay the page the reader waits for.
    this.overlays.pageDrawn(job.page);
    if (!this.painted) {
      this.painted = true;
      this.events.onFirstPaint();
    }
  }

  private renderFailed(job: RenderJob, error: unknown): void {
    if (this.destroyed) return;
    const appError = toAppError(error);
    if (isEngineLost(appError)) {
      if (!this.fatal) {
        this.fatal = true;
        this.scheduler.stop();
        this.events.onFatal(appError);
      }
      return;
    }
    // One page failed to draw. Say so on that page and carry on with the rest.
    const m = this.mounted.get(job.page);
    if (m && m.wantKey === job.key) {
      m.failedKey = job.key;
      m.el.classList.add("viewer-page-failed");
      m.el.dataset.error = "This page could not be drawn.";
    }
  }
}

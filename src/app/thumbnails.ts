// The page thumbnails in the side panel.
//
// A long document can have thousands of pages, so thumbnails are never drawn up front. Every
// thumbnail has its box from the start (so the scroll bar is right), but only the ones near the
// visible part of the list exist as elements, and only those are asked from the engine, and only
// after the list has stopped moving for a moment and the main view has drawn its first page.

import { RenderScheduler, type RenderJob } from "../viewer/scheduler";
import { type Layout, type PageSizePt, visibleRange } from "../viewer/layout";

/** Width of a thumbnail in CSS pixels. */
export const THUMB_WIDTH = 148;
/** Height of the page number under a thumbnail. */
const LABEL_HEIGHT = 26;
const GAP = 10;
const MARGIN = 12;
/** A very tall page gets a thumbnail no taller than this many widths. */
const MAX_ASPECT = 2.5;
/** Largest bitmap side asked for a thumbnail, in device pixels. */
const MAX_BITMAP_SIDE = 480;

export interface ThumbLayout extends Layout {
  /** Height of the picture alone, without the label. */
  pictureHeights: number[];
}

/** Where every thumbnail sits in the list. Pure, so it can be tested without a page. */
export function thumbLayout(pages: PageSizePt[], width = THUMB_WIDTH): ThumbLayout {
  const tops: number[] = [];
  const widths: number[] = [];
  const heights: number[] = [];
  const pictureHeights: number[] = [];
  let y = MARGIN;
  for (const page of pages) {
    const ratio = page.width > 0 ? Math.min(page.height / page.width, MAX_ASPECT) : 1.4;
    const picture = Math.max(1, Math.round(width * ratio));
    tops.push(y);
    widths.push(width);
    pictureHeights.push(picture);
    heights.push(picture + LABEL_HEIGHT);
    y += picture + LABEL_HEIGHT + GAP;
  }
  const totalHeight = pages.length === 0 ? 2 * MARGIN : y - GAP + MARGIN;
  return { tops, widths, heights, pictureHeights, contentWidth: width + 2 * MARGIN, totalHeight };
}

/** The size of the bitmap to ask the engine for, in device pixels. Keeps the page's shape. */
export function thumbBitmapSize(
  cssWidth: number,
  cssHeight: number,
  devicePixelRatio: number,
): { width: number; height: number } {
  const ratio = Math.max(1, devicePixelRatio);
  let width = Math.max(1, Math.round(cssWidth * ratio));
  let height = Math.max(1, Math.round(cssHeight * ratio));
  const longest = Math.max(width, height);
  if (longest > MAX_BITMAP_SIDE) {
    const shrink = MAX_BITMAP_SIDE / longest;
    width = Math.max(1, Math.floor(width * shrink));
    height = Math.max(1, Math.floor(height * shrink));
  }
  return { width, height };
}

export interface ThumbApi {
  render(doc: number, page: number, width: number, height: number): Promise<ArrayBuffer>;
}

export interface ThumbOptions {
  /** How long the list must rest before thumbnails are asked for, in milliseconds. */
  settleMs?: number;
}

interface Mounted {
  el: HTMLElement;
  canvas: HTMLCanvasElement;
  drawn: boolean;
  wantKey: string;
}

export class ThumbnailList {
  private readonly layout: ThumbLayout;
  private readonly sizer: HTMLElement;
  private readonly mounted = new Map<number, Mounted>();
  private readonly scheduler: RenderScheduler;
  private readonly settleMs: number;
  private frame = 0;
  private settleTimer: ReturnType<typeof setTimeout> | undefined;
  private current = -1;
  private enabled = false;
  private destroyed = false;

  constructor(
    private readonly root: HTMLElement,
    private readonly doc: number,
    private readonly pages: PageSizePt[],
    private readonly api: ThumbApi,
    private readonly onSelect: (page: number) => void,
    options: ThumbOptions = {},
  ) {
    this.settleMs = options.settleMs ?? 120;
    this.layout = thumbLayout(pages);
    this.sizer = document.createElement("div");
    this.sizer.className = "thumbs-pages";
    this.sizer.style.height = `${this.layout.totalHeight}px`;
    this.root.replaceChildren(this.sizer);
    this.scheduler = new RenderScheduler({
      concurrency: 1,
      run: (job) => this.renderJob(job),
      onError: (job) => this.renderFailed(job),
    });
    this.root.addEventListener("scroll", this.onScroll, { passive: true });
    this.root.addEventListener("click", this.onClick);
    this.refresh();
  }

  /** Thumbnails are drawn only while enabled: the list is visible and the main view has painted. */
  setEnabled(enabled: boolean): void {
    if (this.destroyed || enabled === this.enabled) return;
    this.enabled = enabled;
    if (enabled) this.scheduleRender();
    else {
      clearTimeout(this.settleTimer);
      this.scheduler.setWanted([]);
    }
  }

  /**
   * Marks the page that is in view in the main view, and scrolls the list to it if it is out of
   * sight (unless `reveal` is false, which keeps the list where it is).
   */
  setCurrent(page: number, reveal = true): void {
    if (this.destroyed || page === this.current) return;
    const old = this.mounted.get(this.current);
    old?.el.removeAttribute("aria-current");
    this.current = page;
    this.mounted.get(page)?.el.setAttribute("aria-current", "page");
    if (!reveal || page < 0 || page >= this.pages.length) return;
    const top = this.layout.tops[page];
    const bottom = top + this.layout.heights[page];
    const view = this.root.scrollTop;
    const height = this.root.clientHeight;
    if (top < view || bottom > view + height) {
      this.root.scrollTop = Math.max(0, top - (height - this.layout.heights[page]) / 2);
      this.refresh();
    }
  }

  get scrollTop(): number {
    return this.root.scrollTop;
  }

  set scrollTop(value: number) {
    this.root.scrollTop = value;
    this.refresh();
  }

  /** The picture of a page changed (an annotation was added, say): its thumbnail is drawn again. */
  invalidate(page: number): void {
    const m = this.mounted.get(page);
    if (this.destroyed || !m) return;
    m.drawn = false;
    if (this.enabled) this.scheduleRender();
  }

  /** How many thumbnail elements exist right now. */
  get mountedCount(): number {
    return this.mounted.size;
  }

  destroy(): void {
    this.destroyed = true;
    clearTimeout(this.settleTimer);
    cancelAnimationFrame(this.frame);
    this.scheduler.stop();
    this.root.removeEventListener("scroll", this.onScroll);
    this.root.removeEventListener("click", this.onClick);
    for (const index of [...this.mounted.keys()]) this.unmount(index);
    this.root.replaceChildren();
  }

  private onScroll = (): void => {
    if (this.frame || this.destroyed) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      this.refresh();
    });
  };

  private onClick = (e: Event): void => {
    const target = e.target;
    if (!(target instanceof Element)) return;
    const item = target.closest<HTMLElement>(".thumb");
    const page = Number(item?.dataset.page);
    if (item && Number.isInteger(page)) this.onSelect(page);
  };

  /** Mounts the thumbnails near the visible part, unmounts the others. Draws nothing by itself. */
  private refresh(): void {
    if (this.destroyed) return;
    const height = this.root.clientHeight || 600;
    const range = visibleRange(this.layout, this.root.scrollTop, height, height / 2);
    for (const index of [...this.mounted.keys()]) {
      if (!range || index < range.first || index > range.last) this.unmount(index);
    }
    if (range) {
      for (let i = range.first; i <= range.last; i++) {
        if (!this.mounted.has(i)) this.mount(i);
      }
    }
    if (this.enabled) this.scheduleRender();
  }

  private mount(index: number): void {
    const item = document.createElement("button");
    item.type = "button";
    item.className = "thumb";
    item.dataset.page = String(index);
    item.setAttribute("aria-label", `Page ${index + 1}`);
    if (index === this.current) item.setAttribute("aria-current", "page");
    item.style.top = `${this.layout.tops[index]}px`;
    item.style.height = `${this.layout.heights[index]}px`;
    item.style.width = `${this.layout.widths[index]}px`;

    const paper = document.createElement("span");
    paper.className = "thumb-paper";
    paper.style.height = `${this.layout.pictureHeights[index]}px`;
    const canvas = document.createElement("canvas");
    canvas.className = "thumb-canvas";
    paper.append(canvas);

    const label = document.createElement("span");
    label.className = "thumb-label num";
    label.textContent = String(index + 1);
    item.append(paper, label);
    this.sizer.append(item);
    this.mounted.set(index, { el: item, canvas, drawn: false, wantKey: "" });
  }

  private unmount(index: number): void {
    const m = this.mounted.get(index);
    if (!m) return;
    // Shrinking the canvas frees its bitmap right away.
    m.canvas.width = 0;
    m.canvas.height = 0;
    m.el.remove();
    this.mounted.delete(index);
  }

  /** Waits until the list has rested, then asks for the thumbnails that are missing. */
  private scheduleRender(): void {
    clearTimeout(this.settleTimer);
    // What is wanted may have changed while the list moved: drop old requests at once.
    this.scheduler.setWanted([]);
    this.settleTimer = setTimeout(() => this.requestVisible(), this.settleMs);
  }

  private requestVisible(): void {
    if (this.destroyed || !this.enabled) return;
    const dpr = typeof window !== "undefined" ? window.devicePixelRatio || 1 : 1;
    const center = this.root.scrollTop + (this.root.clientHeight || 600) / 2;
    const jobs: { job: RenderJob; distance: number }[] = [];
    for (const [index, m] of this.mounted) {
      if (m.drawn) continue;
      const size = thumbBitmapSize(
        this.layout.widths[index],
        this.layout.pictureHeights[index],
        dpr,
      );
      const key = `${index}:${size.width}x${size.height}`;
      m.wantKey = key;
      const mid = this.layout.tops[index] + this.layout.heights[index] / 2;
      jobs.push({
        job: { key, page: index, width: size.width, height: size.height },
        distance: Math.abs(mid - center),
      });
    }
    jobs.sort((a, b) => a.distance - b.distance);
    this.scheduler.setWanted(jobs.map((j) => j.job));
  }

  private async renderJob(job: RenderJob): Promise<void> {
    if (this.destroyed) return;
    const buffer = await this.api.render(this.doc, job.page, job.width, job.height);
    const m = this.mounted.get(job.page);
    if (this.destroyed || !m || m.wantKey !== job.key) return;
    if (buffer.byteLength !== job.width * job.height * 4) return;
    const ctx = m.canvas.getContext("2d");
    if (!ctx) return;
    m.canvas.width = job.width;
    m.canvas.height = job.height;
    ctx.putImageData(new ImageData(new Uint8ClampedArray(buffer), job.width, job.height), 0, 0);
    m.drawn = true;
  }

  /** A thumbnail that cannot be drawn stays an empty sheet; nothing else is affected. */
  private renderFailed(job: RenderJob): void {
    const m = this.mounted.get(job.page);
    if (m && m.wantKey === job.key) m.drawn = true;
  }
}

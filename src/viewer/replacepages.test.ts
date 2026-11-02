// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MARGIN, PAGE_GAP } from "./layout";
import { DocumentView, type ViewerApi } from "./viewer";

const letter = { width: 612, height: 792 };
const wide = { width: 792, height: 612 };

beforeEach(() => {
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", (id: number) => clearTimeout(id));
  vi.stubGlobal(
    "ImageData",
    class {
      constructor(
        public data: Uint8ClampedArray,
        public width: number,
        public height: number,
      ) {}
    },
  );
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    () => ({ putImageData: vi.fn() }) as unknown as CanvasRenderingContext2D,
  );
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains("viewer") ? 800 : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains("viewer") ? 900 : 0;
    },
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

const flush = async () => {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0));
};

function open(pages: { width: number; height: number }[]) {
  const render = vi.fn((_d: number, _p: number, w: number, h: number) => Promise.resolve(new ArrayBuffer(w * h * 4)));
  const getPageText = vi.fn((_d: number, _p: number) => Promise.resolve({ text: "", rects: [] as number[] }));
  const api: ViewerApi = { render, getPageText } as unknown as ViewerApi;
  const events = { onPageChange: vi.fn(), onZoomChange: vi.fn(), onFatal: vi.fn(), onFirstPaint: vi.fn() };
  const view = new DocumentView({ doc: 7, pages }, api, events);
  document.body.append(view.element);
  view.start();
  return { view, render, events, getPageText };
}

describe("DocumentView.replacePages", () => {
  it("lays out the new list: the number of pages, their sizes and the height of the scroll area", async () => {
    const { view } = open([letter, letter, letter]);
    await flush();
    view.setZoom(1);
    const k = 96 / 72;
    const sizer = view.element.querySelector<HTMLElement>(".viewer-pages");
    view.replacePages([letter, wide]);
    await flush();
    expect(view.pageCount).toBe(2);
    const height = MARGIN * 2 + Math.round(792 * k) + PAGE_GAP + Math.round(612 * k);
    expect(sizer?.style.height).toBe(`${height}px`);
    const second = view.element.querySelector<HTMLElement>('.viewer-page[data-page="1"]');
    expect(second?.style.width).toBe(`${Math.round(792 * k)}px`);
  });

  it("draws the pages of the new list from the engine and not the old pictures", async () => {
    const { view, render } = open([letter, letter, letter]);
    await flush();
    const before = render.mock.calls.length;
    view.replacePages([wide, wide]);
    await flush();
    expect(render.mock.calls.length).toBeGreaterThan(before);
    const sizesAsked = render.mock.calls.slice(before).map((c) => [c[1], c[2] > c[3]]);
    // Both pages are wide now, so each bitmap is wider than it is high.
    expect(sizesAsked.length).toBeGreaterThanOrEqual(2);
    expect(sizesAsked.every(([, landscape]) => landscape === true)).toBe(true);
    expect(view.element.querySelectorAll(".viewer-page").length).toBe(2);
  });

  it("goes to the page it is told to, or stays near the page it was at, and never past the end", async () => {
    const { view, events } = open(Array.from({ length: 6 }, () => letter));
    await flush();
    view.goToPage(4);
    await flush();
    expect(view.currentPageIndex).toBe(4);
    view.replacePages(Array.from({ length: 3 }, () => letter));
    await flush();
    // Page 4 is gone: the view stays at the last page that is left.
    expect(view.currentPageIndex).toBe(2);
    view.replacePages(Array.from({ length: 6 }, () => letter), 1);
    await flush();
    expect(view.currentPageIndex).toBe(1);
    expect(events.onPageChange).toHaveBeenLastCalledWith(1);
    view.replacePages(Array.from({ length: 6 }, () => letter), 99);
    await flush();
    expect(view.currentPageIndex).toBe(5);
  });

  it("forgets the text kept for the old pages", async () => {
    const { view, getPageText } = open([letter, letter]);
    await flush();
    const before = getPageText.mock.calls.length;
    expect(before).toBeGreaterThan(0);
    view.replacePages([letter, letter]);
    await flush();
    // Page 0 means another page now, so its text is asked for again.
    expect(getPageText.mock.calls.length).toBeGreaterThan(before);
  });

  it("can end up with a different number of pages than it started with, and with none", async () => {
    const { view } = open([letter]);
    await flush();
    view.replacePages(Array.from({ length: 50 }, () => letter));
    await flush();
    expect(view.pageCount).toBe(50);
    view.replacePages([]);
    await flush();
    expect(view.pageCount).toBe(0);
    expect(view.element.querySelectorAll(".viewer-page").length).toBe(0);
  });

  it("does nothing to a view that was destroyed, and waits for a view that is not shown", async () => {
    const hidden = open([letter, letter, letter]);
    await flush();
    hidden.view.setActive(false);
    hidden.view.replacePages([letter]);
    expect(hidden.view.element.querySelectorAll(".viewer-page").length).toBe(0);
    hidden.view.setActive(true);
    await flush();
    expect(hidden.view.pageCount).toBe(1);
    expect(hidden.view.element.querySelectorAll(".viewer-page").length).toBe(1);

    const gone = open([letter, letter]);
    gone.view.destroy();
    gone.view.replacePages([letter, letter, letter]);
    expect(gone.view.pageCount).toBe(2);
  });
});

describe("a picture that is on its way when the pages change", () => {
  it("is not kept for the new page that has the number of the old one", async () => {
    const held: (() => void)[] = [];
    const calls: number[] = [];
    const render = vi.fn((_d: number, p: number, w: number, h: number) => {
      calls.push(p);
      return new Promise<ArrayBuffer>((res) => held.push(() => res(new ArrayBuffer(w * h * 4))));
    });
    const getPageText = vi.fn(() => Promise.resolve({ text: "", rects: [] as number[] }));
    const events = { onPageChange: vi.fn(), onZoomChange: vi.fn(), onFatal: vi.fn(), onFirstPaint: vi.fn() };
    const view = new DocumentView({ doc: 7, pages: [letter, letter, letter] }, { render, getPageText } as unknown as ViewerApi, events);
    document.body.append(view.element);
    view.start();
    await flush();
    const before = calls.filter((p) => p === 0).length;
    expect(before).toBe(1);
    // Page 0 was deleted: the old page 1 is the new page 0, of the same size.
    view.replacePages([letter, letter]);
    await flush();
    for (const go of held.splice(0)) go();
    await flush();
    await flush();
    // The new page 0 asked for a picture of its own, and the old one was not put on it.
    expect(calls.filter((p) => p === 0).length).toBe(before + 1);
    const canvas = view.element.querySelector<HTMLElement>('.viewer-page[data-page="0"] canvas');
    expect(canvas).not.toBeNull();
  });

  it("is dropped, and the page shows only what was asked for after the change", async () => {
    const held: { page: number; go: () => void }[] = [];
    const drawn: number[] = [];
    const render = vi.fn((_d: number, p: number, w: number, h: number) => new Promise<ArrayBuffer>((res) => held.push({ page: p, go: () => res(new ArrayBuffer(w * h * 4)) })));
    const getPageText = vi.fn(() => Promise.resolve({ text: "", rects: [] as number[] }));
    const events = { onPageChange: vi.fn(), onZoomChange: vi.fn(), onFatal: vi.fn(), onFirstPaint: vi.fn(), onPageDrawn: (p: number) => drawn.push(p) };
    const view = new DocumentView({ doc: 7, pages: [letter, letter] }, { render, getPageText } as unknown as ViewerApi, events);
    document.body.append(view.element);
    view.start();
    await flush();
    const old = held.splice(0);
    view.replacePages([letter, letter]);
    await flush();
    // The pictures of the old pages arrive first: none of them is taken for a page of the new list.
    for (const h of old) h.go();
    await flush();
    expect(drawn).toEqual([]);
    for (const h of held.splice(0)) h.go();
    await flush();
    expect(drawn.sort()).toEqual([0, 1]);
  });
});

// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MARGIN, PAGE_GAP } from "./layout";
import { DocumentView, type ViewerApi, type ViewerEvents } from "./viewer";

const VIEWPORT = { width: 900, height: 800 };
const letter = { width: 612, height: 792 };

const putImageData = vi.fn();

beforeEach(() => {
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) =>
    setTimeout(() => cb(0), 0),
  );
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
  putImageData.mockReset();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    () => ({ putImageData }) as unknown as CanvasRenderingContext2D,
  );
  // jsdom does no layout: tell it how big the scroll area is.
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains("viewer") ? VIEWPORT.height : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains("viewer") ? VIEWPORT.width : 0;
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

interface Harness {
  view: DocumentView;
  render: ReturnType<typeof vi.fn>;
  events: { [K in keyof ViewerEvents]: ReturnType<typeof vi.fn> };
}

function open(
  pageCount: number,
  render?: (doc: number, page: number, w: number, h: number) => Promise<ArrayBuffer>,
): Harness {
  const fake = vi.fn(
    render ?? ((_d: number, _p: number, w: number, h: number) => Promise.resolve(new ArrayBuffer(w * h * 4))),
  );
  const api: ViewerApi = { render: fake };
  const events = {
    onPageChange: vi.fn(),
    onZoomChange: vi.fn(),
    onFatal: vi.fn(),
    onFirstPaint: vi.fn(),
  };
  const view = new DocumentView(
    { doc: 7, pages: Array.from({ length: pageCount }, () => letter) },
    api,
    events,
  );
  document.body.append(view.element);
  view.start();
  return { view, render: fake, events };
}

const mountedPages = (view: DocumentView) =>
  [...view.element.querySelectorAll<HTMLElement>(".viewer-page")].map((el) => Number(el.dataset.page));

describe("DocumentView", () => {
  it("creates elements only for the pages near the viewport", async () => {
    const { view, render } = open(500);
    await flush();
    const pages = mountedPages(view);
    expect(pages.length).toBeLessThanOrEqual(3);
    expect(pages).toContain(0);
    // Only those pages were asked from the engine.
    const asked = new Set(render.mock.calls.map((c) => c[1]));
    expect([...asked].every((p) => pages.includes(p as number))).toBe(true);
    expect(asked.has(400)).toBe(false);
  });

  it("asks the engine for a bitmap that matches the screen", async () => {
    const { render } = open(3);
    await flush();
    // Fit width in a 900 px wide viewport: (900 - 32) / 816 of 100 %, so about 868 px wide.
    const [doc, page, width, height] = render.mock.calls[0];
    expect(doc).toBe(7);
    expect(page).toBe(0);
    expect(width).toBe(868);
    expect(height).toBe(Math.round(868 * (792 / 612)));
  });

  it("starts with the pages fitted to the width", () => {
    const { view, events } = open(3);
    expect(view.zoomMode.kind).toBe("fit-width");
    expect(view.zoomValue).toBeCloseTo((900 - 2 * MARGIN) / 816, 3);
    expect(events.onZoomChange).toHaveBeenCalled();
  });

  it("draws the bitmap on the canvas and reports the first paint once", async () => {
    const { events } = open(3);
    await flush();
    expect(putImageData).toHaveBeenCalled();
    expect(events.onFirstPaint).toHaveBeenCalledTimes(1);
  });

  it("replaces pages as the user scrolls, so memory stays flat", async () => {
    const { view } = open(500);
    await flush();
    view.element.scrollTop = 300_000;
    view.element.dispatchEvent(new Event("scroll"));
    await flush();
    const pages = mountedPages(view);
    expect(pages.length).toBeLessThanOrEqual(4);
    expect(pages).not.toContain(0);
    const top = Math.min(...pages);
    expect(top).toBeGreaterThan(200);
  });

  it("reports the page that is in view", async () => {
    const { view, events } = open(50);
    await flush();
    view.goToPage(20);
    await flush();
    expect(events.onPageChange).toHaveBeenLastCalledWith(20);
    expect(view.currentPageIndex).toBe(20);
  });

  it("jumps to a page and clamps a bad index", async () => {
    const { view } = open(50);
    await flush();
    view.goToPage(1000);
    await flush();
    expect(view.currentPageIndex).toBe(49);
    view.goToPage(-5);
    await flush();
    expect(view.currentPageIndex).toBe(0);
  });

  it("keeps the reading position when the zoom changes", async () => {
    const { view } = open(100);
    await flush();
    view.goToPage(30);
    await flush();
    view.setZoom(2);
    await flush();
    expect(view.zoomMode).toEqual({ kind: "custom", zoom: 2 });
    expect(view.currentPageIndex).toBe(30);
    view.stepZoom(1);
    expect(view.zoomValue).toBe(3);
    view.stepZoom(-1);
    view.stepZoom(-1);
    expect(view.zoomValue).toBe(1.5);
  });

  it("fit width undoes a custom zoom", () => {
    const { view } = open(5);
    view.setZoom(3);
    expect(view.zoomValue).toBe(3);
    view.setFitWidth();
    expect(view.zoomMode.kind).toBe("fit-width");
    expect(view.zoomValue).toBeLessThan(1.1);
  });

  it("scrolls with keyboard actions", () => {
    const { view } = open(50);
    expect(view.handleAction({ type: "scroll-by", x: 0, y: 48 })).toBe(true);
    expect(view.element.scrollTop).toBe(48);
    view.handleAction({ type: "scroll-page", direction: 1 });
    expect(view.element.scrollTop).toBe(48 + 720);
    view.handleAction({ type: "scroll-page", direction: -1 });
    expect(view.element.scrollTop).toBe(48);
    view.handleAction({ type: "go-first" });
    expect(view.element.scrollTop).toBe(0);
    expect(view.handleAction({ type: "open-file" })).toBe(false);
  });

  it("zooms with Ctrl and the mouse wheel, and leaves a plain wheel alone", () => {
    const { view } = open(5);
    const plain = new WheelEvent("wheel", { deltaY: 100, cancelable: true });
    view.element.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(false);

    const zoomIn = new WheelEvent("wheel", { deltaY: -100, ctrlKey: true, cancelable: true });
    view.element.dispatchEvent(zoomIn);
    expect(zoomIn.defaultPrevented).toBe(true);
    expect(view.zoomMode.kind).toBe("custom");
    // Fit width was about 106 %: the next step up is 125 %.
    expect(view.zoomValue).toBe(1.25);
  });

  it("drops a bitmap that arrives after its page scrolled away", async () => {
    const resolvers: (() => void)[] = [];
    const { view } = open(500, (_d, _p, w, h) => {
      return new Promise<ArrayBuffer>((resolve) => {
        resolvers.push(() => resolve(new ArrayBuffer(w * h * 4)));
      });
    });
    await flush();
    view.element.scrollTop = 300_000;
    view.element.dispatchEvent(new Event("scroll"));
    await flush();
    // The first batch of renders (pages near the top) finishes only now.
    resolvers.splice(0).forEach((r) => r());
    await flush();
    // Nothing was drawn on a page that is no longer on screen.
    expect(putImageData).not.toHaveBeenCalled();
  });

  it("tells the app when the engine is lost, once, and stops asking", async () => {
    const { render, events } = open(10, () =>
      Promise.reject({ code: "engine_stopped", message: "it exited" }),
    );
    await flush();
    expect(events.onFatal).toHaveBeenCalledTimes(1);
    expect(events.onFatal.mock.calls[0][0].code).toBe("engine_stopped");
    const calls = render.mock.calls.length;
    await flush();
    expect(render.mock.calls.length).toBe(calls);
  });

  it("marks one broken page and keeps showing the rest", async () => {
    const { view, events } = open(3, (_d, page, w, h) =>
      page === 0
        ? Promise.reject({ code: "render_failed", message: "no" })
        : Promise.resolve(new ArrayBuffer(w * h * 4)),
    );
    await flush();
    const first = view.element.querySelector<HTMLElement>('[data-page="0"]');
    expect(first?.classList.contains("viewer-page-failed")).toBe(true);
    expect(first?.dataset.error).toBeTruthy();
    expect(events.onFatal).not.toHaveBeenCalled();
    expect(putImageData).toHaveBeenCalled();
  });

  it("rejects a bitmap of the wrong size instead of drawing garbage", async () => {
    const { view } = open(1, () => Promise.resolve(new ArrayBuffer(16)));
    await flush();
    expect(putImageData).not.toHaveBeenCalled();
    expect(view.element.querySelector(".viewer-page-failed")).not.toBeNull();
  });

  it("copes with a document that has no pages", async () => {
    const { view, render } = open(0);
    await flush();
    expect(mountedPages(view)).toEqual([]);
    expect(render).not.toHaveBeenCalled();
    view.goToPage(3);
    expect(view.currentPageIndex).toBe(0);
  });

  it("sizes the scrollable area for all pages from the start", () => {
    const { view } = open(500);
    const sizer = view.element.querySelector<HTMLElement>(".viewer-pages");
    const height = parseFloat(sizer?.style.height ?? "0");
    expect(height).toBeGreaterThan(500 * 800);
    expect(height).toBeGreaterThan(MARGIN + PAGE_GAP);
  });

  it("removes everything when destroyed", async () => {
    const { view } = open(5);
    await flush();
    view.destroy();
    expect(document.body.querySelector(".viewer")).toBeNull();
  });
});

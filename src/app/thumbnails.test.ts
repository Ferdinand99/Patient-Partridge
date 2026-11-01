// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { THUMB_WIDTH, ThumbnailList, thumbBitmapSize, thumbLayout } from "./thumbnails";

const letter = { width: 612, height: 792 };
const putImageData = vi.fn();

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
  putImageData.mockReset();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    () => ({ putImageData }) as unknown as CanvasRenderingContext2D,
  );
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.id === "list" ? 600 : 0;
    },
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

const wait = (ms = 0) => new Promise((r) => setTimeout(r, ms));
const flush = async () => {
  for (let i = 0; i < 8; i++) await wait();
};

function make(pageCount: number, settleMs = 5) {
  const root = document.createElement("div");
  root.id = "list";
  document.body.append(root);
  const render = vi.fn((_d: number, _p: number, w: number, h: number) =>
    Promise.resolve(new ArrayBuffer(w * h * 4)),
  );
  const onSelect = vi.fn();
  const list = new ThumbnailList(
    root,
    9,
    Array.from({ length: pageCount }, () => letter),
    { render },
    onSelect,
    { settleMs },
  );
  return { root, list, render, onSelect };
}

const mountedPages = (root: HTMLElement) =>
  [...root.querySelectorAll<HTMLElement>(".thumb")].map((el) => Number(el.dataset.page));

describe("thumbLayout", () => {
  it("gives every page a box from the start, one after the other", () => {
    const layout = thumbLayout([letter, letter, { width: 100, height: 1000 }]);
    expect(layout.tops).toHaveLength(3);
    expect(layout.tops[1]).toBeGreaterThan(layout.tops[0] + layout.heights[0]);
    expect(layout.widths).toEqual([THUMB_WIDTH, THUMB_WIDTH, THUMB_WIDTH]);
    expect(layout.totalHeight).toBeGreaterThan(layout.tops[2] + layout.heights[2]);
  });

  it("keeps the shape of the page, and cuts a very tall page", () => {
    const layout = thumbLayout([letter, { width: 100, height: 5000 }, { width: 0, height: 0 }]);
    expect(layout.pictureHeights[0]).toBe(Math.round(THUMB_WIDTH * (792 / 612)));
    expect(layout.pictureHeights[1]).toBe(Math.round(THUMB_WIDTH * 2.5));
    expect(layout.pictureHeights[2]).toBeGreaterThan(0);
  });

  it("copes with a document without pages", () => {
    const layout = thumbLayout([]);
    expect(layout.tops).toEqual([]);
    expect(layout.totalHeight).toBeGreaterThan(0);
  });
});

describe("thumbBitmapSize", () => {
  it("asks for the size on screen, sharper on a dense screen, and never a big bitmap", () => {
    expect(thumbBitmapSize(148, 192, 1)).toEqual({ width: 148, height: 192 });
    expect(thumbBitmapSize(148, 192, 2)).toEqual({ width: 296, height: 384 });
    const big = thumbBitmapSize(148, 192, 8);
    expect(Math.max(big.width, big.height)).toBeLessThanOrEqual(480);
    expect(big.width / big.height).toBeCloseTo(148 / 192, 1);
    expect(thumbBitmapSize(148, 192, 0.5)).toEqual({ width: 148, height: 192 });
  });
});

describe("ThumbnailList", () => {
  it("makes elements only for the thumbnails near the visible part, however long the document", () => {
    const { root, list } = make(5000);
    expect(list.mountedCount).toBeLessThan(15);
    expect(mountedPages(root)).toContain(0);
    expect(mountedPages(root)).not.toContain(2000);
    // The scroll bar is right from the start.
    const height = Number.parseInt((root.firstElementChild as HTMLElement).style.height, 10);
    expect(height).toBeGreaterThan(5000 * 150);
  });

  it("asks the engine for nothing until it is enabled", async () => {
    const { list, render } = make(200);
    await wait(30);
    expect(render).not.toHaveBeenCalled();
    list.setEnabled(true);
    await wait(30);
    await flush();
    expect(render).toHaveBeenCalled();
  });

  it("draws only the thumbnails that are shown, the middle ones first", async () => {
    const { root, list, render } = make(2000);
    list.setEnabled(true);
    await wait(30);
    await flush();
    const asked = new Set(render.mock.calls.map((c) => c[1] as number));
    expect(asked.size).toBeLessThan(15);
    expect(asked.has(0)).toBe(true);
    expect(asked.has(1500)).toBe(false);
    expect(putImageData).toHaveBeenCalled();
    expect(root.querySelector("canvas")?.width).toBeGreaterThan(0);
  });

  it("does not ask for the pages it scrolls past, only where it comes to rest", async () => {
    const { root, list, render } = make(3000);
    list.setEnabled(true);
    await wait(30);
    await flush();
    render.mockClear();
    // A quick fling through the list: many scroll events within the settle time.
    for (let y = 0; y <= 300_000; y += 20_000) {
      root.scrollTop = y;
      root.dispatchEvent(new Event("scroll"));
      await wait(1);
    }
    await wait(60);
    await flush();
    const asked = [...new Set(render.mock.calls.map((c) => c[1] as number))];
    expect(asked.length).toBeGreaterThan(0);
    // Only pages near the final place were drawn; none of the ones passed on the way.
    const final = Math.min(...asked);
    expect(final).toBeGreaterThan(1000);
    expect(asked.every((p) => p >= final && p <= final + 14)).toBe(true);
  });

  it("frees the canvases of thumbnails that scroll away", async () => {
    const { root, list } = make(500);
    list.setEnabled(true);
    await wait(30);
    await flush();
    const first = root.querySelector<HTMLCanvasElement>(".thumb canvas") as HTMLCanvasElement;
    expect(first.width).toBeGreaterThan(0);
    root.scrollTop = 60_000;
    root.dispatchEvent(new Event("scroll"));
    await wait(30);
    expect(mountedPages(root)).not.toContain(0);
    expect(first.width).toBe(0);
  });

  it("stops asking when it is disabled", async () => {
    const { list, render } = make(100);
    list.setEnabled(true);
    list.setEnabled(false);
    await wait(40);
    await flush();
    expect(render).not.toHaveBeenCalled();
  });

  it("selects the page of the thumbnail that is clicked", () => {
    const { root, onSelect } = make(10);
    root.querySelector<HTMLElement>('[data-page="3"]')?.click();
    expect(onSelect).toHaveBeenCalledWith(3);
    // A click on the picture inside counts too.
    root.querySelector<HTMLElement>('[data-page="2"] .thumb-paper')?.click();
    expect(onSelect).toHaveBeenLastCalledWith(2);
  });

  it("marks the current page and scrolls to it when it is far away", () => {
    const { root, list } = make(500);
    list.setCurrent(0);
    expect(root.querySelector('[aria-current="page"]')?.getAttribute("data-page")).toBe("0");
    list.setCurrent(300);
    expect(root.scrollTop).toBeGreaterThan(40_000);
    const current = root.querySelector('[aria-current="page"]') as HTMLElement;
    expect(current.dataset.page).toBe("300");
    expect(root.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
  });

  it("labels every thumbnail with its page number", () => {
    const { root } = make(3);
    const labels = [...root.querySelectorAll(".thumb-label")].map((l) => l.textContent);
    expect(labels).toEqual(["1", "2", "3"]);
    expect(root.querySelector(".thumb")?.getAttribute("aria-label")).toBe("Page 1");
  });

  it("leaves a thumbnail empty when its page cannot be drawn, and carries on", async () => {
    const root = document.createElement("div");
    root.id = "list";
    document.body.append(root);
    const render = vi.fn((_d: number, page: number, w: number, h: number) =>
      page === 0 ? Promise.reject(new Error("no")) : Promise.resolve(new ArrayBuffer(w * h * 4)),
    );
    const list = new ThumbnailList(root, 1, [letter, letter, letter], { render }, vi.fn(), { settleMs: 5 });
    list.setEnabled(true);
    await wait(30);
    await flush();
    expect(render.mock.calls.map((c) => c[1])).toEqual(expect.arrayContaining([0, 1, 2]));
    expect(putImageData).toHaveBeenCalledTimes(2);
  });

  it("removes its elements and timers when it is destroyed", async () => {
    const { root, list, render } = make(50);
    list.setEnabled(true);
    list.destroy();
    await wait(30);
    expect(root.children.length).toBe(0);
    expect(render).not.toHaveBeenCalled();
  });
});

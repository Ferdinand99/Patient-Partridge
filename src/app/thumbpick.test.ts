// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ThumbnailList, thumbLayout } from "./thumbnails";

const letter = { width: 612, height: 792 };

beforeEach(() => {
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
  vi.stubGlobal("cancelAnimationFrame", (id: number) => clearTimeout(id));
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    () => ({ putImageData: vi.fn() }) as unknown as CanvasRenderingContext2D,
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

function make(count: number, options: ConstructorParameters<typeof ThumbnailList>[5] = {}) {
  const root = document.createElement("div");
  root.id = "list";
  root.getBoundingClientRect = () =>
    ({ top: 100, bottom: 700, left: 0, right: 200, width: 200, height: 600, x: 0, y: 100, toJSON: () => ({}) }) as DOMRect;
  document.body.append(root);
  const onSelect = vi.fn();
  const list = new ThumbnailList(
    root,
    1,
    Array.from({ length: count }, () => letter),
    { render: () => Promise.resolve(new ArrayBuffer(0)) },
    onSelect,
    options,
  );
  return { root, list, onSelect, layout: thumbLayout(Array.from({ length: count }, () => letter)) };
}

describe("thumbnails for the page tools", () => {
  it("lets the page tools mark every thumbnail as it appears, and again on request", () => {
    const decorate = vi.fn((el: HTMLElement, index: number) => el.setAttribute("data-mark", String(index)));
    const { root, list } = make(6, { decorate });
    expect(decorate).toHaveBeenCalled();
    expect(root.querySelector('.thumb[data-page="2"]')?.getAttribute("data-mark")).toBe("2");
    const calls = decorate.mock.calls.length;
    list.redecorate();
    expect(decorate.mock.calls.length).toBe(calls * 2);
    expect(list.elementOf(2)).toBe(root.querySelector('[data-page="2"]'));
    expect(list.elementOf(99)).toBeNull();
  });

  it("gives a click to the page tools first, and goes to the page when they decline", () => {
    const pick = vi.fn((page: number) => page === 1);
    const { root, onSelect } = make(6, { pick });
    root.querySelector<HTMLElement>('[data-page="1"]')?.click();
    expect(pick).toHaveBeenCalledWith(1, expect.any(MouseEvent));
    expect(onSelect).not.toHaveBeenCalled();
    root.querySelector<HTMLElement>('[data-page="2"]')?.click();
    expect(onSelect).toHaveBeenCalledWith(2);
  });

  it("says where a dropped page would go, from the height of the pointer", () => {
    const { list, layout } = make(6);
    const at = (y: number) => list.insertionAt(100 + y);
    expect(at(0)).toBe(0);
    expect(at(layout.tops[1] + layout.heights[1] / 2 - 1)).toBe(1);
    expect(at(layout.tops[1] + layout.heights[1] / 2 + 1)).toBe(2);
    expect(at(100000)).toBe(6);
  });

  it("shows one line where a page would go, moves it, and takes it away", () => {
    const { root, list, layout } = make(6);
    list.showDrop(2);
    const lines = root.querySelectorAll<HTMLElement>(".thumb-drop");
    expect(lines).toHaveLength(1);
    const top2 = parseFloat(lines[0].style.top);
    list.showDrop(4);
    expect(root.querySelectorAll(".thumb-drop")).toHaveLength(1);
    expect(parseFloat(root.querySelector<HTMLElement>(".thumb-drop")?.style.top ?? "0")).toBeGreaterThan(top2);
    // Before the first page, between the pages, and after the last.
    list.showDrop(0);
    expect(parseFloat(root.querySelector<HTMLElement>(".thumb-drop")?.style.top ?? "-1")).toBeLessThan(layout.tops[0]);
    list.showDrop(6);
    expect(parseFloat(root.querySelector<HTMLElement>(".thumb-drop")?.style.top ?? "0")).toBeGreaterThan(
      layout.tops[5] + layout.heights[5],
    );
    list.showDrop(null);
    expect(root.querySelector(".thumb-drop")).toBeNull();
  });

  it("scrolls by the least that brings a page into view", () => {
    const { root, list, layout } = make(20);
    list.scrollToPage(1);
    expect(root.scrollTop).toBe(0);
    list.scrollToPage(10);
    expect(root.scrollTop).toBeGreaterThan(layout.tops[10] + layout.heights[10] - 600 - 1);
    expect(root.scrollTop).toBeLessThan(layout.tops[10]);
    const there = root.scrollTop;
    list.scrollToPage(10);
    expect(root.scrollTop).toBe(there);
    list.scrollToPage(0);
    expect(root.scrollTop).toBeLessThan(there);
    list.scrollToPage(-3);
    list.scrollToPage(500);
  });

  it("scrolls on its own near the top and the bottom edge while a page is dragged", () => {
    const { root, list } = make(20);
    expect(list.autoScroll(400)).toBe(false);
    expect(list.autoScroll(695)).toBe(true);
    expect(root.scrollTop).toBeGreaterThan(0);
    const there = root.scrollTop;
    expect(list.autoScroll(105)).toBe(true);
    expect(root.scrollTop).toBeLessThan(there);
    // At the very top, there is nowhere to go.
    root.scrollTop = 0;
    expect(list.autoScroll(100)).toBe(false);
  });

  it("does the same without any of the new options as it always did", () => {
    const { root, onSelect } = make(4);
    root.querySelector<HTMLElement>('[data-page="3"]')?.click();
    expect(onSelect).toHaveBeenCalledWith(3);
  });
});

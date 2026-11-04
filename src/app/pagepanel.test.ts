// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { orderAfterMove } from "../lib/pages";
import { PageOps, type PageTarget } from "./pageops";
import { PagePanel, type PagePanelHost } from "./pagepanel";
import { Sidebar, type SidebarDocument } from "./sidebar";
import { thumbLayout } from "./thumbnails";

const letter = { width: 612, height: 792 };
const pages = (n: number) => Array.from({ length: n }, () => letter);

beforeEach(() => {
  const html = readFileSync("index.html", "utf8");
  const body = /<body>([\s\S]*)<\/body>/.exec(html)?.[1] ?? "";
  document.body.innerHTML = body.replace(/<script[\s\S]*?<\/script>/g, "");
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
      return this.id === "thumbs" || this.id === "outline" ? 1000 : 0;
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
  for (let i = 0; i < 6; i++) await wait();
};

function fakeOps() {
  const ok = (select: number[]) => Promise.resolve({ select, show: select[0] });
  return {
    busy: false,
    remove: vi.fn((p: number[]) => ok([Math.min(...p)])),
    reorder: vi.fn((order: number[], moved: number[]) => ok(moved.map((m) => order.indexOf(m)))),
    rotate: vi.fn((p: number[]) => ok(p)),
    crop: vi.fn((p: number[]) => ok(p)),
    insertBlank: vi.fn((at: number) => ok([at])),
    insertFiles: vi.fn((at: number) => ok([at, at + 1])),
    extract: vi.fn(() => Promise.resolve(null)),
  };
}

function setup(count = 10) {
  const ops = fakeOps();
  const goToPage = vi.fn();
  let current = 0;
  const target: PageTarget = { doc: 1, name: "book.pdf", history: undefined as never, pages: pages(count) };
  const host: PagePanelHost = {
    target: () => target,
    currentPage: () => current,
    goToPage,
    render: () => Promise.resolve(new ArrayBuffer(0)),
    layer: document.getElementById("notice-layer") as HTMLElement,
    announce: vi.fn(),
  };
  const panel = new PagePanel(document, ops as unknown as PageOps, host);
  const sidebar = new Sidebar(document, {
    api: { render: () => Promise.resolve(new ArrayBuffer(0)), getOutline: () => Promise.resolve([]) },
    storage: null,
    pagePanel: panel,
  });
  const doc: SidebarDocument = {
    doc: 1,
    pages: pages(count),
    goToPage,
    hasPainted: () => true,
    currentPage: () => current,
  };
  sidebar.setDocument(doc);
  sidebar.setOpen(true);
  const thumbs = document.getElementById("thumbs") as HTMLElement;
  // jsdom has no layout: the list is 1000 high and starts at the top of the window.
  thumbs.getBoundingClientRect = () =>
    ({ top: 0, bottom: 1000, left: 0, right: 240, width: 240, height: 1000, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
  const thumb = (i: number) => thumbs.querySelector<HTMLElement>(`.thumb[data-page="${i}"]`) as HTMLElement;
  const picked = () =>
    [...thumbs.querySelectorAll<HTMLElement>('.thumb[aria-pressed="true"]')].map((e) => Number(e.dataset.page));
  const key = (el: Element, k: string, init: KeyboardEventInit = {}) =>
    el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));
  const pointer = (el: Element, type: string, init: MouseEventInit = {}) =>
    el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, ...init }));
  return { ops, goToPage, panel, sidebar, thumbs, thumb, picked, key, pointer, setCurrent: (n: number) => (current = n), doc };
}

describe("the page panel", () => {
  it("shows its tools with the Pages panel and a document, and not otherwise", () => {
    const t = setup();
    const tools = document.getElementById("pagetools") as HTMLElement;
    expect(tools.hidden).toBe(false);
    t.sidebar.setPanel("contents");
    expect(tools.hidden).toBe(true);
    t.sidebar.setPanel("pages");
    expect(tools.hidden).toBe(false);
    t.sidebar.setDocument(null);
    expect(tools.hidden).toBe(true);
  });

  it("the tools that need pages wait until pages are picked", async () => {
    const t = setup();
    const btn = (id: string) => document.getElementById(id) as HTMLButtonElement;
    for (const id of ["pt-left", "pt-right", "pt-delete", "pt-crop", "pt-extract"]) expect(btn(id).disabled).toBe(true);
    expect(btn("pt-blank").disabled).toBe(false);
    expect(btn("pt-insert").disabled).toBe(false);
    t.thumb(1).click();
    for (const id of ["pt-left", "pt-right", "pt-delete", "pt-crop", "pt-extract"]) expect(btn(id).disabled).toBe(false);
    t.ops.busy = true;
    t.panel.setBusy(true);
    for (const id of ["pt-right", "pt-blank", "pt-insert"]) expect(btn(id).disabled).toBe(true);
    expect(document.getElementById("pagetools")?.getAttribute("aria-busy")).toBe("true");
  });

  it("a click picks a page and goes to it; Ctrl and Shift pick without going", () => {
    const t = setup();
    t.thumb(1).click();
    expect(t.picked()).toEqual([1]);
    expect(t.goToPage).toHaveBeenLastCalledWith(1);
    t.goToPage.mockClear();
    t.thumb(2).dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    expect(t.picked()).toEqual([1, 2]);
    t.thumb(0).dispatchEvent(new MouseEvent("click", { bubbles: true, shiftKey: true }));
    expect(t.picked()).toEqual([0, 1, 2]);
    expect(t.goToPage).not.toHaveBeenCalled();
    t.thumb(1).click();
    expect(t.picked()).toEqual([1]);
    expect(document.getElementById("pt-status")?.textContent).toBe("1 page picked");
  });

  it("every picked page says so to a screen reader, and only the cursor is in the tab order", () => {
    const t = setup();
    t.thumb(1).click();
    t.thumb(2).dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    expect(t.thumb(2).getAttribute("aria-label")).toBe("Page 3, picked");
    expect(t.thumb(0).getAttribute("aria-label")).toBe("Page 1");
    const stops = [...t.thumbs.querySelectorAll<HTMLElement>(".thumb")].filter((e) => e.tabIndex === 0);
    expect(stops.map((e) => e.dataset.page)).toEqual(["2"]);
  });

  it("the arrow keys pick and move the focus, Shift extends, Ctrl+A picks all, Escape lets go", async () => {
    const t = setup();
    t.thumb(0).click();
    t.thumb(0).focus();
    t.key(t.thumb(0), "ArrowDown");
    expect(t.picked()).toEqual([1]);
    expect(document.activeElement).toBe(t.thumb(1));
    t.key(document.activeElement as Element, "ArrowDown", { shiftKey: true });
    expect(t.picked()).toEqual([1, 2]);
    t.key(document.activeElement as Element, "ArrowUp", { shiftKey: true });
    expect(t.picked()).toEqual([1]);
    t.key(document.activeElement as Element, "End");
    expect(t.picked()).toEqual([9]);
    expect(document.activeElement).toBe(t.thumb(9));
    t.key(document.activeElement as Element, "Home");
    expect(t.picked()).toEqual([0]);
    t.key(document.activeElement as Element, "a", { ctrlKey: true });
    expect(t.picked()).toHaveLength(t.thumbs.querySelectorAll(".thumb").length);
    expect(document.getElementById("pt-status")?.textContent).toBe("10 pages picked");
    t.key(document.activeElement as Element, "Escape");
    expect(t.picked()).toEqual([]);
    await flush();
  });

  it("Space picks the page the cursor is on, Ctrl and an arrow move the cursor alone, Enter goes to it", () => {
    const t = setup();
    t.thumb(0).click();
    t.thumb(0).focus();
    t.key(document.activeElement as Element, "ArrowDown", { ctrlKey: true });
    t.key(document.activeElement as Element, "ArrowDown", { ctrlKey: true });
    expect(t.picked()).toEqual([0]);
    expect(document.activeElement).toBe(t.thumb(2));
    t.key(document.activeElement as Element, " ");
    expect(t.picked()).toEqual([0, 2]);
    t.goToPage.mockClear();
    t.key(document.activeElement as Element, "Enter");
    expect(t.goToPage).toHaveBeenCalledWith(2);
  });

  it("Delete, R and Shift+R work on the picked pages; nothing picked, nothing happens", async () => {
    const t = setup();
    t.thumb(1).focus();
    t.key(t.thumb(1), "Delete");
    expect(t.ops.remove).not.toHaveBeenCalled();
    t.thumb(1).click();
    t.thumb(2).dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    t.key(t.thumb(2), "Delete");
    expect(t.ops.remove).toHaveBeenCalledWith([1, 2]);
    await flush();
    t.thumb(1).click();
    t.thumb(2).dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    t.key(t.thumb(2), "r");
    expect(t.ops.rotate).toHaveBeenLastCalledWith([1, 2], 1);
    t.key(t.thumb(2), "R", { shiftKey: true });
    expect(t.ops.rotate).toHaveBeenLastCalledWith([1, 2], 3);
    t.key(t.thumb(2), "r", { ctrlKey: true });
    expect(t.ops.rotate).toHaveBeenCalledTimes(2);
  });

  it("Alt and an arrow move the picked pages one place and pick them where they went", async () => {
    const t = setup();
    t.thumb(2).click();
    t.thumb(3).dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    t.key(t.thumb(3), "ArrowDown", { altKey: true });
    expect(t.ops.reorder).toHaveBeenLastCalledWith([0, 1, 4, 2, 3, 5, 6, 7, 8, 9], [2, 3]);
    await flush();
    expect(t.picked()).toEqual([3, 4]);
    t.key(t.thumb(3), "ArrowUp", { altKey: true });
    expect(t.ops.reorder).toHaveBeenLastCalledWith(orderAfterMove(10, [3, 4], 2), [3, 4]);
    // The first page cannot go further up.
    t.ops.reorder.mockClear();
    t.thumb(0).click();
    t.key(t.thumb(0), "ArrowUp", { altKey: true });
    expect(t.ops.reorder).not.toHaveBeenCalled();
  });

  it("the buttons do what the keys do, and the insert goes after the last picked page", async () => {
    const t = setup();
    const click = (id: string) => (document.getElementById(id) as HTMLButtonElement).click();
    t.thumb(1).click();
    t.thumb(2).dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    click("pt-insert");
    expect(t.ops.insertFiles).toHaveBeenLastCalledWith(3);
    click("pt-left");
    expect(t.ops.rotate).toHaveBeenLastCalledWith([1, 2], 3);
    click("pt-right");
    expect(t.ops.rotate).toHaveBeenLastCalledWith([1, 2], 1);
    click("pt-delete");
    expect(t.ops.remove).toHaveBeenLastCalledWith([1, 2]);
    click("pt-extract");
    expect(t.ops.extract).toHaveBeenLastCalledWith([1, 2]);
    await flush();
    // Nothing picked: the new page goes after the page in view.
    (document.getElementById("thumbs") as HTMLElement).dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    t.panel.pick([]);
    t.setCurrent(4);
    click("pt-blank");
    expect(t.ops.insertBlank).toHaveBeenLastCalledWith(5, 612, 792);
  });

  it("a page that a tool made is picked afterwards", async () => {
    const t = setup();
    t.thumb(1).click();
    (document.getElementById("pt-delete") as HTMLButtonElement).click();
    await flush();
    expect(t.picked()).toEqual([1]);
  });

  it("the pages changed behind the panel's back: nothing is picked", () => {
    const t = setup();
    t.thumb(1).click();
    expect(t.picked()).toEqual([1]);
    t.doc.pages = pages(7);
    t.sidebar.pagesChanged(1);
    expect(t.picked()).toEqual([]);
    expect(t.thumbs.querySelectorAll(".thumb").length).toBeGreaterThan(0);
    expect(document.getElementById("pt-delete")).toHaveProperty("disabled", true);
  });
});

describe("dragging pages", () => {
  const layout = thumbLayout(pages(10));
  const midOf = (i: number) => layout.tops[i] + layout.heights[i] / 2;

  it("drags the picked pages to the place under the pointer", () => {
    const t = setup();
    t.thumb(0).click();
    // Press on page 0, move to the lower half of page 2: the pages land before page 3.
    t.pointer(t.thumb(0), "pointerdown", { clientX: 10, clientY: 50 });
    t.pointer(t.thumbs, "pointermove", { clientX: 10, clientY: midOf(2) + 20 });
    expect(t.thumbs.hasAttribute("data-dragging")).toBe(true);
    expect(t.thumbs.querySelector(".thumb-drop")).not.toBeNull();
    t.pointer(t.thumbs, "pointerup", { clientX: 10, clientY: midOf(2) + 20 });
    expect(t.thumbs.querySelector(".thumb-drop")).toBeNull();
    expect(t.thumbs.hasAttribute("data-dragging")).toBe(false);
    expect(t.ops.reorder).toHaveBeenCalledWith([1, 2, 0, 3, 4, 5, 6, 7, 8, 9], [0]);
  });

  it("dragging a page that is not picked picks it alone, and a group moves together", async () => {
    const t = setup();
    t.thumb(0).click();
    t.thumb(1).dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    t.pointer(t.thumb(0), "pointerdown", { clientX: 10, clientY: 50 });
    t.pointer(t.thumbs, "pointermove", { clientX: 10, clientY: midOf(3) - 5 });
    t.pointer(t.thumbs, "pointerup", { clientX: 10, clientY: midOf(3) - 5 });
    expect(t.ops.reorder).toHaveBeenLastCalledWith(orderAfterMove(10, [0, 1], 3), [0, 1]);
    t.ops.reorder.mockClear();
    await flush();
    t.thumb(0).click();
    // Page 2 is not picked now: pressing it and dragging picks it alone.
    t.pointer(t.thumb(2), "pointerdown", { clientX: 10, clientY: midOf(2) });
    t.pointer(t.thumbs, "pointermove", { clientX: 10, clientY: midOf(0) - 20 });
    t.pointer(t.thumbs, "pointerup", { clientX: 10, clientY: midOf(0) - 20 });
    expect(t.ops.reorder).toHaveBeenLastCalledWith([2, 0, 1, 3, 4, 5, 6, 7, 8, 9], [2]);
  });

  it("a press that hardly moves is a click, and a drop where the pages already are does nothing", () => {
    const t = setup();
    t.pointer(t.thumb(1), "pointerdown", { clientX: 10, clientY: midOf(1) });
    t.pointer(t.thumbs, "pointermove", { clientX: 12, clientY: midOf(1) + 2 });
    t.pointer(t.thumbs, "pointerup", { clientX: 12, clientY: midOf(1) + 2 });
    expect(t.thumbs.hasAttribute("data-dragging")).toBe(false);
    expect(t.ops.reorder).not.toHaveBeenCalled();
    // Drag page 1 and drop it where it is (the upper half of page 2 is before page 2).
    t.pointer(t.thumb(1), "pointerdown", { clientX: 10, clientY: midOf(1) });
    t.pointer(t.thumbs, "pointermove", { clientX: 10, clientY: midOf(1) + 30 });
    expect(t.thumbs.querySelector(".thumb-drop")).toBeNull();
    t.pointer(t.thumbs, "pointerup", { clientX: 10, clientY: midOf(1) + 30 });
    expect(t.ops.reorder).not.toHaveBeenCalled();
  });

  it("the click that ends a drag does not pick the page under the pointer", async () => {
    const t = setup();
    t.thumb(0).click();
    t.pointer(t.thumb(0), "pointerdown", { clientX: 10, clientY: 50 });
    t.pointer(t.thumbs, "pointermove", { clientX: 10, clientY: midOf(2) + 20 });
    t.pointer(t.thumbs, "pointerup", { clientX: 10, clientY: midOf(2) + 20 });
    t.thumb(2).click();
    await flush();
    // The drop moved page 0 to place 2, and the panel picked it there; the stray click did not.
    expect(t.picked()).toEqual([2]);
    expect(t.goToPage).toHaveBeenCalledTimes(1);
  });

  it("a cancelled drag (the pointer is taken away) leaves the pages alone", () => {
    const t = setup();
    t.pointer(t.thumb(0), "pointerdown", { clientX: 10, clientY: 50 });
    t.pointer(t.thumbs, "pointermove", { clientX: 10, clientY: midOf(3) });
    t.pointer(t.thumbs, "pointercancel", {});
    expect(t.thumbs.querySelector(".thumb-drop")).toBeNull();
    expect(t.thumbs.hasAttribute("data-dragging")).toBe(false);
    t.pointer(t.thumbs, "pointerup", {});
    expect(t.ops.reorder).not.toHaveBeenCalled();
  });

  it("nothing is dragged while a tool works", () => {
    const t = setup();
    t.ops.busy = true;
    t.pointer(t.thumb(0), "pointerdown", { clientX: 10, clientY: 50 });
    t.pointer(t.thumbs, "pointermove", { clientX: 10, clientY: midOf(3) });
    t.pointer(t.thumbs, "pointerup", { clientX: 10, clientY: midOf(3) });
    expect(t.ops.reorder).not.toHaveBeenCalled();
    t.key(t.thumb(0), "Delete");
    expect(t.ops.remove).not.toHaveBeenCalled();
  });
});

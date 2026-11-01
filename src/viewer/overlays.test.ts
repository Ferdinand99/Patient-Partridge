// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DocumentView, type ViewerApi, type ViewerEvents } from "./viewer";
import type { PageLinkInfo } from "./overlays";

const letter = { width: 612, height: 792 };
const putImageData = vi.fn();
const W = 6;
const H = 10;

/** The text of a test page, laid out like the engine reports it. */
function wire(text: string, left = 50, top = 100) {
  const rects: number[] = [];
  let x = left;
  let y = top;
  for (const ch of Array.from(text)) {
    if (ch === "\n") {
      rects.push(0, 0, 0, 0);
      x = left;
      y += 14;
    } else {
      rects.push(x, y, x + W, y + H);
      x += W;
    }
  }
  return { text, rects };
}

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
      return this.classList.contains("viewer") ? 800 : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains("viewer") ? 900 : 0;
    },
  });
  // jsdom does no layout: work out where a page is on screen from the style the view gave it.
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    if (this.classList.contains("viewer-page")) {
      const viewer = this.closest(".viewer") as HTMLElement;
      const left = Number.parseFloat(this.style.left);
      const top = Number.parseFloat(this.style.top) - viewer.scrollTop;
      const width = Number.parseFloat(this.style.width);
      const height = Number.parseFloat(this.style.height);
      return { left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON() {} };
    }
    if (this.classList.contains("viewer")) {
      return { left: 0, top: 0, width: 900, height: 800, right: 900, bottom: 800, x: 0, y: 0, toJSON() {} };
    }
    return { left: 0, top: 0, width: 0, height: 0, right: 0, bottom: 0, x: 0, y: 0, toJSON() {} };
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

const wait = (ms = 0) => new Promise((r) => setTimeout(r, ms));
const flush = async () => {
  for (let i = 0; i < 10; i++) await wait();
};

const TEXTS: Record<number, string> = {
  0: "Hello world\nsecond line",
  1: "Page two text",
  2: "Third page",
};

function open(links: Record<number, PageLinkInfo[]> = {}, pageCount = 3) {
  const getPageText = vi.fn((_doc: number, page: number) => Promise.resolve(wire(TEXTS[page] ?? "")));
  const getPageLinks = vi.fn((_doc: number, page: number) => Promise.resolve(links[page] ?? []));
  const api: ViewerApi = {
    render: (_d, _p, w, h) => Promise.resolve(new ArrayBuffer(w * h * 4)),
    getPageText,
    getPageLinks,
  };
  const events = {
    onPageChange: vi.fn(),
    onZoomChange: vi.fn(),
    onFatal: vi.fn(),
    onFirstPaint: vi.fn(),
    onLink: vi.fn(),
    onSelectionChange: vi.fn(),
    onSelectionSettled: vi.fn(),
  } satisfies ViewerEvents;
  const view = new DocumentView({ doc: 5, pages: Array.from({ length: pageCount }, () => letter) }, api, events);
  document.body.append(view.element);
  view.start();
  return { view, events, getPageText, getPageLinks };
}

/** Screen coordinates of a point on a page, given in points. */
function screen(view: DocumentView, page: number, x: number, y: number) {
  const el = view.element.querySelector<HTMLElement>(`.viewer-page[data-page="${page}"]`) as HTMLElement;
  const rect = el.getBoundingClientRect();
  return { clientX: rect.left + (x / 612) * rect.width, clientY: rect.top + (y / 792) * rect.height };
}

function pointer(view: DocumentView, type: string, at: { clientX: number; clientY: number }, detail = 1) {
  view.element.dispatchEvent(new MouseEvent(type, { ...at, detail, button: 0, bubbles: true, cancelable: true }));
}

const highlights = (view: DocumentView, cls: string) => [...view.element.querySelectorAll<HTMLElement>(`.${cls}`)];

describe("text and links are asked for after the page is drawn", () => {
  it("loads the text and the links of the pages that were drawn, and no others", async () => {
    const { getPageText, getPageLinks } = open({}, 500);
    await flush();
    const pages = new Set(getPageText.mock.calls.map((c) => c[1]));
    expect(pages.has(0)).toBe(true);
    expect(pages.has(300)).toBe(false);
    expect(new Set(getPageLinks.mock.calls.map((c) => c[1]))).toEqual(pages);
    // Each page once, however often it is drawn.
    expect(getPageText.mock.calls.length).toBe(pages.size);
  });

  it("works when the app gives no text or links at all", async () => {
    const view = new DocumentView({ doc: 1, pages: [letter] }, { render: (_d, _p, w, h) => Promise.resolve(new ArrayBuffer(w * h * 4)) }, {
      onPageChange: vi.fn(),
      onZoomChange: vi.fn(),
      onFatal: vi.fn(),
      onFirstPaint: vi.fn(),
    });
    document.body.append(view.element);
    view.start();
    await flush();
    pointer(view, "pointerdown", screen(view, 0, 100, 100));
    expect(view.hasSelection).toBe(false);
  });
});

describe("selecting text with the pointer", () => {
  it("selects what is dragged over and draws a box for each line", async () => {
    const { view, events } = open();
    await flush();
    // From before 'H' to after 'world' on the first line.
    pointer(view, "pointerdown", screen(view, 0, 51, 105));
    pointer(view, "pointermove", screen(view, 0, 50 + W * 11 - 1, 105));
    pointer(view, "pointerup", screen(view, 0, 50 + W * 11 - 1, 105));
    expect(view.hasSelection).toBe(true);
    expect(view.selectedText()).toEqual({ text: "Hello world", missing: [] });
    const boxes = highlights(view, "hl-select");
    expect(boxes).toHaveLength(1);
    expect(Number.parseFloat(boxes[0].style.left)).toBeCloseTo((50 / 612) * 100, 2);
    expect(events.onSelectionChange).toHaveBeenLastCalledWith(true);
    expect(events.onSelectionSettled).toHaveBeenCalled();
  });

  it("selects across lines and across pages, and the other way round too", async () => {
    const { view } = open();
    await flush();
    pointer(view, "pointerdown", screen(view, 0, 51, 105));
    pointer(view, "pointermove", screen(view, 0, 100, 119));
    pointer(view, "pointerup", screen(view, 0, 100, 119));
    expect(view.selectedText().text.startsWith("Hello world\nsecond")).toBe(true);
    expect(highlights(view, "hl-select")).toHaveLength(2);

    view.clearSelection();
    pointer(view, "pointerdown", screen(view, 0, 70, 119));
    // Drag down onto the next page, which is on screen too.
    view.element.scrollTop = 0;
    const second = screen(view, 1, 50 + W * 4, 105);
    pointer(view, "pointermove", second);
    pointer(view, "pointerup", second);
    const got = view.selectedText();
    expect(got.missing).toEqual([]);
    expect(got.text).toBe("ond line\nPage");
  });

  it("selects a word on a double click and a line on a triple click", async () => {
    const { view } = open();
    await flush();
    const press = (at: { clientX: number; clientY: number }) => {
      pointer(view, "pointerdown", at);
      pointer(view, "pointerup", at);
    };
    const word = screen(view, 0, 70, 105);
    press(word);
    expect(view.hasSelection).toBe(false);
    press(word);
    expect(view.selectedText().text).toBe("Hello");
    // A third press at the same place selects the line.
    press(word);
    expect(view.selectedText().text).toBe("Hello world");
    // A press somewhere else starts counting again: it is a single click and clears the selection.
    press(screen(view, 0, 70, 119));
    expect(view.hasSelection).toBe(false);
  });

  it("does not take two slow clicks, or two clicks far apart, for a double click", async () => {
    const { view } = open();
    await flush();
    const press = (at: { clientX: number; clientY: number }) => {
      pointer(view, "pointerdown", at);
      pointer(view, "pointerup", at);
    };
    press(screen(view, 0, 70, 105));
    press(screen(view, 0, 200, 105));
    expect(view.hasSelection).toBe(false);
    const first = screen(view, 0, 70, 105);
    view.element.dispatchEvent(new MouseEvent("pointerdown", { ...first, button: 0, bubbles: true }));
    view.element.dispatchEvent(new MouseEvent("pointerup", { ...first, button: 0, bubbles: true }));
    await wait(520);
    press(first);
    expect(view.hasSelection).toBe(false);
  });

  it("clears the selection on a click that does not drag, and with clearSelection", async () => {
    const { view, events } = open();
    await flush();
    pointer(view, "pointerdown", screen(view, 0, 51, 105));
    pointer(view, "pointermove", screen(view, 0, 90, 105));
    pointer(view, "pointerup", screen(view, 0, 90, 105));
    expect(view.hasSelection).toBe(true);
    pointer(view, "pointerdown", screen(view, 0, 300, 400));
    pointer(view, "pointerup", screen(view, 0, 300, 400));
    expect(view.hasSelection).toBe(false);
    expect(highlights(view, "hl-select")).toHaveLength(0);
    expect(events.onSelectionChange).toHaveBeenLastCalledWith(false);
    pointer(view, "pointerdown", screen(view, 0, 51, 105));
    pointer(view, "pointermove", screen(view, 0, 90, 105));
    pointer(view, "pointerup", screen(view, 0, 90, 105));
    view.clearSelection();
    expect(view.hasSelection).toBe(false);
  });

  it("ignores the other mouse buttons", async () => {
    const { view } = open();
    await flush();
    view.element.dispatchEvent(
      new MouseEvent("pointerdown", { ...screen(view, 0, 51, 105), button: 2, bubbles: true }),
    );
    expect(view.hasSelection).toBe(false);
  });

  it("scrolls while dragging near the bottom edge", async () => {
    const { view } = open();
    await flush();
    pointer(view, "pointerdown", screen(view, 0, 51, 105));
    const before = view.element.scrollTop;
    pointer(view, "pointermove", { clientX: 300, clientY: 795 });
    expect(view.element.scrollTop).toBeGreaterThan(before);
  });

  it("selects all the text of the document and can give it all, loading what is missing", async () => {
    const { view, getPageText } = open({}, 3);
    await flush();
    getPageText.mockClear();
    view.selectAll();
    expect(view.hasSelection).toBe(true);
    const text = await view.loadSelectedText();
    expect(text).toBe("Hello world\nsecond line\nPage two text\nThird page");
    expect(view.selectedText().missing).toEqual([]);
  });

  it("asks the engine for the pages of a long document only when they are selected", async () => {
    const { view, getPageText } = open({}, 400);
    await flush();
    const before = getPageText.mock.calls.length;
    expect(before).toBeLessThan(10);
    view.selectAll();
    // Pages that were not loaded are listed as missing until they are.
    expect(view.selectedText().missing.length).toBeGreaterThan(100);
  });
});

describe("search highlights", () => {
  it("draws the matches and picks out the current one", async () => {
    const { view } = open();
    await flush();
    view.setFind(new Map([[0, [[0, 5], [6, 5]]]]), { page: 0, start: 6, len: 5 });
    await flush();
    expect(highlights(view, "hl-find")).toHaveLength(2);
    expect(highlights(view, "hl-current")).toHaveLength(1);
    view.clearFind();
    expect(highlights(view, "hl-find")).toHaveLength(0);
  });

  it("scrolls to a match that is out of sight", async () => {
    const { view } = open({}, 3);
    await flush();
    const match = { page: 2, start: 0, len: 5 };
    expect(view.element.scrollTop).toBe(0);
    await view.revealMatch(match);
    expect(view.element.scrollTop).toBeGreaterThan(1000);
  });

  it("loads the text of a page with a match before it draws the match", async () => {
    const { view, getPageText } = open({}, 3);
    await flush();
    view.element.scrollTop = 0;
    getPageText.mockClear();
    view.setFind(new Map([[1, [[0, 4]]]]), null);
    await flush();
    expect(highlights(view, "hl-find").length).toBeGreaterThan(0);
  });
});

describe("links", () => {
  const web: PageLinkInfo = { rect: [72, 72, 172, 92], uri: "https://example.com/a", page: null };
  const inner: PageLinkInfo = { rect: [72, 120, 172, 140], uri: null, page: 2 };

  it("draws a button over each link, placed in percent of the page", async () => {
    const { view } = open({ 0: [web, inner] });
    await flush();
    const buttons = highlights(view, "page-link");
    expect(buttons).toHaveLength(2);
    expect(Number.parseFloat(buttons[0].style.left)).toBeCloseTo((72 / 612) * 100, 2);
    expect(Number.parseFloat(buttons[0].style.width)).toBeCloseTo((100 / 612) * 100, 2);
    expect(buttons[0].style.left.endsWith("%")).toBe(true);
    expect(buttons[0].getAttribute("aria-label")).toBe("Link to https://example.com/a");
    expect(buttons[1].getAttribute("aria-label")).toBe("Go to page 3");
  });

  it("only tells the receiver which link was pressed: nothing is opened here", async () => {
    const { view, events } = open({ 0: [web, inner] });
    await flush();
    highlights(view, "page-link")[0].click();
    expect(events.onLink).toHaveBeenCalledWith(web, 0);
    highlights(view, "page-link")[1].click();
    expect(events.onLink).toHaveBeenLastCalledWith(inner, 0);
  });

  it("does not start a selection when a link is pressed", async () => {
    const { view } = open({ 0: [web] });
    await flush();
    const button = highlights(view, "page-link")[0];
    button.dispatchEvent(new MouseEvent("pointerdown", { ...screen(view, 0, 100, 80), button: 0, bubbles: true }));
    expect(view.hasSelection).toBe(false);
  });

  it("shows an address only as text, never as markup", async () => {
    const evil: PageLinkInfo = { rect: [0, 0, 10, 10], uri: '"><img src=x onerror=alert(1)>', page: null };
    const { view } = open({ 0: [evil] });
    await flush();
    expect(view.element.querySelector("img")).toBeNull();
    expect(highlights(view, "page-link")[0].getAttribute("aria-label")).toContain("<img");
  });

  it("copes with links that cannot be read", async () => {
    const getPageLinks = vi.fn(() => Promise.reject(new Error("no")));
    const view = new DocumentView(
      { doc: 1, pages: [letter] },
      { render: (_d, _p, w, h) => Promise.resolve(new ArrayBuffer(w * h * 4)), getPageLinks },
      { onPageChange: vi.fn(), onZoomChange: vi.fn(), onFatal: vi.fn(), onFirstPaint: vi.fn() },
    );
    document.body.append(view.element);
    view.start();
    await flush();
    expect(highlights(view, "page-link")).toHaveLength(0);
    expect(getPageLinks).toHaveBeenCalledTimes(1);
  });
});

describe("hidden views", () => {
  it("let go of the layers and the text while hidden, and bring the highlights back", async () => {
    const { view } = open();
    await flush();
    view.setFind(new Map([[0, [[0, 5]]]]), null);
    await flush();
    expect(highlights(view, "hl-find")).toHaveLength(1);
    view.setActive(false);
    expect(view.element.querySelectorAll(".page-overlay")).toHaveLength(0);
    view.setActive(true);
    await flush();
    expect(highlights(view, "hl-find")).toHaveLength(1);
  });
});

// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedDocument, SavedPosition } from "../lib/api";
import { THEME_KEY } from "../lib/theme";
import { App } from "./app";

const putImageData = vi.fn();

function installPage(): void {
  const html = readFileSync("index.html", "utf8");
  const body = /<body>([\s\S]*)<\/body>/.exec(html)?.[1] ?? "";
  document.body.innerHTML = body.replace(/<script[\s\S]*?<\/script>/g, "");
  document.documentElement.removeAttribute("data-theme");
}

// Every App adds listeners to the document and the window. They are taken off after each test,
// so that an App of an earlier test does not answer the keys and events of the next one.
const addedListeners: [EventTarget, string, EventListenerOrEventListenerObject, unknown][] = [];

beforeEach(() => {
  for (const target of [document, window] as EventTarget[]) {
    const original = target.addEventListener.bind(target);
    vi.spyOn(target, "addEventListener").mockImplementation((type, listener, options) => {
      if (listener) addedListeners.push([target, type, listener, options]);
      original(type, listener, options);
    });
  }
  installPage();
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
});

afterEach(() => {
  for (const [target, type, listener, options] of addedListeners) {
    target.removeEventListener(type, listener, options as EventListenerOptions);
  }
  addedListeners.length = 0;
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

const flush = async () => {
  for (let i = 0; i < 8; i++) await new Promise((r) => setTimeout(r, 0));
};

const letter = { width: 612, height: 792 };

/** Boxes for a line of text, 6 x 10 points per character, as the engine reports them. */
function layoutRects(text: string): number[] {
  return Array.from(text).flatMap((_, i) => [50 + i * 6, 100, 56 + i * 6, 110]);
}
const opened = (
  doc: number,
  pages: number,
  name = "book.pdf",
  position: SavedPosition | null = null,
): OpenedDocument => ({
  doc,
  name,
  pages: Array.from({ length: pages }, () => letter),
  position,
});

class MemoryStorage {
  readonly data = new Map<string, string>();
  getItem(key: string): string | null {
    return this.data.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.data.set(key, value);
  }
}

function make(
  options: {
    measureOn?: boolean;
    storage?: MemoryStorage;
    /** Keeps the real delay before a link question can be answered. Tests wait for it otherwise. */
    realLinkDelay?: boolean;
    viewLimits?: { preloadChars?: number; copyChars?: number; keptChars?: number };
    /** What the app process says about the setting "Reopen tabs at start". On by default. */
    reopenTabs?: boolean;
    /** How long a tab of the last session may take to open at the start. */
    sessionOpenLimitMs?: number;
  } = {},
) {
  let nextDoc = 1;
  let urlCounter = 0;
  const api = {
    open: vi.fn((path: string, _password?: string) =>
      Promise.resolve(opened(nextDoc++, 3, path.split("/").pop())),
    ),
    render: vi.fn((_d: number, _p: number, w: number, h: number) =>
      Promise.resolve(new ArrayBuffer(w * h * 4)),
    ),
    close: vi.fn((_doc: number) => Promise.resolve()),
    setPosition: vi.fn((_path: string, _position: SavedPosition) => Promise.resolve()),
    clearPositions: vi.fn(() => Promise.resolve()),
    getReopenTabs: vi.fn(() => Promise.resolve(options.reopenTabs ?? true)),
    setReopenTabs: vi.fn((_enabled: boolean) => Promise.resolve()),
    saveSession: vi.fn((_paths: string[], _active: number | null, _failed: string[]) => Promise.resolve()),
    getOutline: vi.fn((_doc: number) =>
      Promise.resolve([
        { depth: 0, page: 0, title: "Start" },
        { depth: 0, page: 2, title: "End" },
      ]),
    ),
    getPageText: vi.fn((_doc: number, _page: number) =>
      Promise.resolve({ text: "Hello world", rects: layoutRects("Hello world") }),
    ),
    missedFonts: vi.fn((_doc: number): Promise<string[]> => Promise.resolve([])),
    getPageLinks: vi.fn((_doc: number, page: number) =>
      Promise.resolve(
        page === 0
          ? [
              { rect: [10, 10, 60, 30] as [number, number, number, number], uri: "https://example.com/a", page: null },
              { rect: [10, 40, 60, 60] as [number, number, number, number], uri: "file:///etc/passwd", page: null },
              { rect: [10, 70, 60, 90] as [number, number, number, number], uri: null, page: 2 },
            ]
          : [],
      ),
    ),
    searchPage: vi.fn((_doc: number, page: number, _query: string) =>
      Promise.resolve(page === 1 ? ([[0, 5]] as [number, number][]) : []),
    ),
    openLink: vi.fn((_doc: number, _url: string) => Promise.resolve()),
    measureMode: vi.fn(() => Promise.resolve(options.measureOn ?? false)),
    measureReport: vi.fn((_a: number, _b: number, _c: number) => Promise.resolve()),
  };
  const storage = options.storage ?? new MemoryStorage();
  const deps = {
    api,
    pickFiles: vi.fn(() => Promise.resolve<string[]>([])),
    setWindowTitle: vi.fn(),
    now: vi.fn(() => 0),
    storage,
    positionDelayMs: 5,
    thumbSettleMs: 5,
    findDebounceMs: 5,
    linkAnswerDelayMs: options.realLinkDelay ? undefined : 0,
    sessionOpenLimitMs: options.sessionOpenLimitMs,
    viewLimits: options.viewLimits,
    print: {
      encode: vi.fn(() => Promise.resolve(new Blob(["jpeg"]))),
      createUrl: vi.fn(() => `blob:test/${++urlCounter}`),
      revokeUrl: vi.fn(),
      print: vi.fn(() => Promise.resolve()),
    },
  };
  const app = new App(document, deps);
  app.init();
  return { app, api, deps, storage };
}

const q = <T extends HTMLElement>(selector: string) => document.querySelector(selector) as T;
const notice = () => document.querySelector<HTMLElement>(".notice");
const click = (el: Element | null) => (el as HTMLElement).click();
const button = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>(".notice button")].find((b) => b.textContent === label) ?? null;
const tabTitles = () => [...document.querySelectorAll(".tab-title")].map((t) => t.textContent);
const selectedTab = () => document.querySelector('.tab-title[aria-selected="true"]')?.textContent;
const tab = (title: string) =>
  [...document.querySelectorAll<HTMLElement>(".tab-title")].find((t) => t.textContent === title) as HTMLElement;
const key = (k: string, init: KeyboardEventInit = {}) =>
  document.body.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));

describe("App: opening a file", () => {
  it("shows the document, the page count, the tab and the title", async () => {
    const { app, deps } = make();
    await app.openPath("/docs/book.pdf");
    await flush();
    expect(app.currentView).not.toBeNull();
    expect(q("#empty").hidden).toBe(true);
    expect(q<HTMLInputElement>("#page-input").value).toBe("1");
    expect(q("#page-total").textContent).toBe("/ 3");
    expect(deps.setWindowTitle).toHaveBeenLastCalledWith("book.pdf - Patient Partridge");
    expect(q<HTMLInputElement>("#page-input").disabled).toBe(false);
    expect(q("#stage .viewer")).not.toBeNull();
    expect(tabTitles()).toEqual(["book.pdf"]);
    expect(selectedTab()).toBe("book.pdf");
  });

  it("opens from the file dialog, and does nothing when it is cancelled", async () => {
    const { app, deps, api } = make();
    await app.openDialog();
    expect(api.open).not.toHaveBeenCalled();
    deps.pickFiles.mockResolvedValueOnce(["/x/y.pdf"]);
    await app.openDialog();
    expect(api.open).toHaveBeenCalledWith("/x/y.pdf", undefined);
  });

  it("opens every file chosen in the dialog, and shows the first", async () => {
    const { app, deps, api } = make();
    deps.pickFiles.mockResolvedValueOnce(["/a.pdf", "/b.pdf", "/c.pdf"]);
    await app.openDialog();
    await flush();
    expect(api.open).toHaveBeenCalledTimes(3);
    expect(tabTitles()).toEqual(["a.pdf", "b.pdf", "c.pdf"]);
    expect(selectedTab()).toBe("a.pdf");
    expect(app.tabCount).toBe(3);
  });

  it("opens a second file in a second tab and keeps the first open", async () => {
    const { app, api, deps } = make();
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await flush();
    expect(api.close).not.toHaveBeenCalled();
    expect(tabTitles()).toEqual(["a.pdf", "b.pdf"]);
    expect(selectedTab()).toBe("b.pdf");
    expect(document.querySelectorAll(".viewer").length).toBe(2);
    expect(deps.setWindowTitle).toHaveBeenLastCalledWith("b.pdf - Patient Partridge");
  });

  it("shows the tab again instead of opening the same file twice", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await app.openPath("/a.pdf");
    await flush();
    expect(api.open).toHaveBeenCalledTimes(2);
    expect(app.tabCount).toBe(2);
    expect(selectedTab()).toBe("a.pdf");
  });

  it("opens a file once when it is asked for twice at the same moment", async () => {
    const { app, api } = make();
    await Promise.all([app.openPath("/a.pdf"), app.openPath("/a.pdf")]);
    await flush();
    expect(api.open).toHaveBeenCalledTimes(1);
    expect(app.tabCount).toBe(1);
  });

  it("explains it when too many documents are open", async () => {
    const { app, api } = make();
    api.open.mockRejectedValueOnce({ code: "invalid_request", message: "16 documents are open already; close one first" });
    const pending = app.openPath("/more.pdf");
    await flush();
    expect(notice()?.textContent).toContain("close one first");
    click(button("OK"));
    await pending;
    expect(app.tabCount).toBe(0);
  });
});

describe("App: tabs", () => {
  it("switches between documents and shows each one's page count", async () => {
    const { app, api, deps } = make();
    api.open.mockResolvedValueOnce(opened(1, 3, "a.pdf")).mockResolvedValueOnce(opened(2, 9, "b.pdf"));
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await flush();
    expect(q("#page-total").textContent).toBe("/ 9");
    click(tab("a.pdf"));
    await flush();
    expect(selectedTab()).toBe("a.pdf");
    expect(q("#page-total").textContent).toBe("/ 3");
    expect(deps.setWindowTitle).toHaveBeenLastCalledWith("a.pdf - Patient Partridge");
    const views = [...document.querySelectorAll<HTMLElement>(".viewer")];
    expect(views.map((v) => v.hidden)).toEqual([false, true]);
  });

  it("asks the engine for nothing while a document is behind another", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    await app.openPath("/b.pdf");
    await flush();
    // Only the document in front draws: the one behind gave up its canvases.
    expect(document.querySelectorAll<HTMLElement>(".viewer")[0].querySelector("canvas")).toBeNull();
    api.render.mockClear();
    await flush();
    expect(api.render).not.toHaveBeenCalled();
    // Coming back, it draws again.
    click(tab("a.pdf"));
    await flush();
    expect(api.render).toHaveBeenCalled();
    expect(api.render.mock.calls.every(([doc]) => doc === 1)).toBe(true);
  });

  it("does not draw a document opened behind another until its tab is selected", async () => {
    const { app, api, deps } = make();
    deps.pickFiles.mockResolvedValueOnce(["/a.pdf", "/b.pdf"]);
    await app.openDialog();
    await flush();
    expect(api.render.mock.calls.every(([doc]) => doc === 1)).toBe(true);
    api.render.mockClear();
    click(tab("b.pdf"));
    await flush();
    expect(api.render).toHaveBeenCalled();
    expect(api.render.mock.calls.every(([doc]) => doc === 2)).toBe(true);
  });

  it("closes a tab with its button, closes the document in the engine and shows a neighbour", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await app.openPath("/c.pdf");
    await flush();
    click(document.querySelector('[aria-label="Close b.pdf"]'));
    await flush();
    expect(tabTitles()).toEqual(["a.pdf", "c.pdf"]);
    expect(api.close).toHaveBeenCalledWith(2);
    expect(selectedTab()).toBe("c.pdf");
    click(document.querySelector('[aria-label="Close c.pdf"]'));
    await flush();
    expect(selectedTab()).toBe("a.pdf");
    expect(document.querySelectorAll(".viewer").length).toBe(1);
  });

  it("shows the empty state again when the last tab is closed", async () => {
    const { app, deps } = make();
    await app.openPath("/a.pdf");
    await flush();
    key("w", { ctrlKey: true });
    await flush();
    expect(app.tabCount).toBe(0);
    expect(app.currentView).toBeNull();
    expect(q("#empty").hidden).toBe(false);
    expect(q<HTMLInputElement>("#page-input").disabled).toBe(true);
    expect(deps.setWindowTitle).toHaveBeenLastCalledWith("Patient Partridge");
    // And Ctrl+W with nothing open does nothing.
    key("w", { ctrlKey: true });
  });

  it("moves between tabs with Ctrl+Tab, Ctrl+Shift+Tab and the arrow keys", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await app.openPath("/c.pdf");
    await flush();
    key("Tab", { ctrlKey: true });
    expect(selectedTab()).toBe("a.pdf");
    key("Tab", { ctrlKey: true, shiftKey: true });
    expect(selectedTab()).toBe("c.pdf");
    key("PageUp", { ctrlKey: true });
    expect(selectedTab()).toBe("b.pdf");
    tab("b.pdf").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    expect(selectedTab()).toBe("c.pdf");
  });

  it("closes a tab with a middle click", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await flush();
    tab("a.pdf").dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true, cancelable: true }));
    expect(tabTitles()).toEqual(["b.pdf"]);
  });

  it("keeps working on the other documents when the engine of one stops", async () => {
    const { app, api } = make();
    api.render.mockImplementation((doc: number, _p: number, w: number, h: number) =>
      doc === 2
        ? Promise.reject({ code: "engine_stopped", message: "it exited" })
        : Promise.resolve(new ArrayBuffer(w * h * 4)),
    );
    await app.openPath("/a.pdf");
    await flush();
    await app.openPath("/b.pdf");
    await flush();
    expect(notice()?.textContent).toContain("The PDF engine stopped");
    expect(notice()?.textContent).toContain('"b.pdf"');
    // The broken document is gone; the good one is back in front and still alive.
    expect(tabTitles()).toEqual(["a.pdf"]);
    expect(app.currentView).not.toBeNull();
    expect(api.close).toHaveBeenCalledWith(2);
    click(button("Close"));
    await flush();
    expect(notice()).toBeNull();
    expect(api.close).not.toHaveBeenCalledWith(1);
  });
});

describe("App: files that cannot be opened", () => {
  it("explains a damaged file and keeps the document that was open", async () => {
    const { app, api } = make();
    await app.openPath("/good.pdf");
    const before = app.currentView;
    api.open.mockRejectedValueOnce({ code: "corrupt", message: "bad" });
    const pending = app.openPath("/bad.pdf");
    await flush();
    expect(notice()?.textContent).toContain("damaged");
    expect(notice()?.textContent).toContain('"bad.pdf"');
    click(button("OK"));
    await pending;
    expect(notice()).toBeNull();
    expect(app.currentView).toBe(before);
    expect(app.tabCount).toBe(1);
  });

  it("explains a file that cannot be read", async () => {
    const { app, api } = make();
    api.open.mockRejectedValueOnce({ code: "io", message: "gone" });
    const pending = app.openPath("/missing.pdf");
    await flush();
    expect(notice()?.textContent).toContain("could not be read");
    click(button("OK"));
    await pending;
    expect(app.currentView).toBeNull();
    expect(q("#empty").hidden).toBe(false);
  });

  it("asks for a password, tells when it is wrong, and opens with the right one", async () => {
    const { app, api } = make();
    api.open.mockImplementation((_path: string, password?: string) => {
      if (password === undefined) return Promise.reject({ code: "password_required", message: "" });
      if (password !== "right") return Promise.reject({ code: "password_incorrect", message: "" });
      return Promise.resolve(opened(4, 2, "secret.pdf"));
    });
    const done = app.openPath("/secret.pdf");
    await flush();
    expect(notice()?.textContent).toContain("password protected");
    const input = q<HTMLInputElement>(".sheet-field");
    expect(input.type).toBe("password");

    input.value = "wrong";
    click(button("Open"));
    await flush();
    expect(notice()?.textContent).toContain("not correct");

    q<HTMLInputElement>(".sheet-field").value = "right";
    click(button("Open"));
    await done;
    await flush();
    expect(notice()).toBeNull();
    expect(app.currentView).not.toBeNull();
    expect(api.open).toHaveBeenLastCalledWith("/secret.pdf", "right");
  });

  it("gives up quietly when the password question is cancelled", async () => {
    const { app, api } = make();
    api.open.mockRejectedValue({ code: "password_required", message: "" });
    const done = app.openPath("/secret.pdf");
    await flush();
    click(button("Cancel"));
    await done;
    expect(notice()).toBeNull();
    expect(api.open).toHaveBeenCalledTimes(1);
    expect(app.currentView).toBeNull();
  });

  it("lets the user try again after the engine stopped during opening", async () => {
    const { app, api } = make();
    api.open.mockRejectedValueOnce({ code: "engine_stopped", message: "it exited" });
    const done = app.openPath("/crashy.pdf");
    await flush();
    expect(notice()?.textContent).toContain("still running");
    click(button("Try again"));
    await done;
    await flush();
    expect(api.open).toHaveBeenCalledTimes(2);
    expect(app.currentView).not.toBeNull();
  });

  it("tells about a document with no pages", async () => {
    const { app, api } = make();
    api.open.mockResolvedValueOnce(opened(1, 0, "empty.pdf"));
    await app.openPath("/empty.pdf");
    await flush();
    expect(notice()?.textContent).toContain("no pages");
  });
});

describe("App: the engine stops while a document is open", () => {
  it("shows an error, stays usable and can open the file again", async () => {
    const { app, api, deps } = make();
    let calls = 0;
    api.render.mockImplementation((_d: number, _p: number, w: number, h: number) => {
      calls += 1;
      return calls <= 2
        ? Promise.reject({ code: "engine_stopped", message: "it exited" })
        : Promise.resolve(new ArrayBuffer(w * h * 4));
    });
    await app.openPath("/docs/book.pdf");
    await flush();

    expect(notice()?.textContent).toContain("The PDF engine stopped");
    expect(notice()?.textContent).toContain("still running");
    expect(app.currentView).toBeNull();
    expect(q("#empty").hidden).toBe(false);
    expect(q<HTMLInputElement>("#page-input").disabled).toBe(true);
    expect(deps.setWindowTitle).toHaveBeenLastCalledWith("Patient Partridge");

    click(button("Open the file again"));
    await flush();
    expect(api.open).toHaveBeenCalledTimes(2);
    expect(api.open).toHaveBeenLastCalledWith("/docs/book.pdf", undefined);
    expect(app.currentView).not.toBeNull();
    expect(notice()).toBeNull();
  });
});

describe("App: toolbar and keyboard", () => {
  it("jumps to the page typed in the page box", async () => {
    const { app } = make();
    await app.openPath("/book.pdf");
    await flush();
    const input = q<HTMLInputElement>("#page-input");
    input.focus();
    input.value = "3";
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await flush();
    expect(app.currentView?.currentPageIndex).toBe(2);
    expect(input.value).toBe("3");
  });

  it("ignores text in the page box that is not a page", async () => {
    const { app } = make();
    await app.openPath("/book.pdf");
    await flush();
    const input = q<HTMLInputElement>("#page-input");
    input.focus();
    input.value = "banana";
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await flush();
    expect(app.currentView?.currentPageIndex).toBe(0);
    expect(input.value).toBe("1");
  });

  it("scrolls with the keyboard", async () => {
    const { app } = make();
    await app.openPath("/book.pdf");
    await flush();
    key("PageDown");
    expect(app.currentView?.element.scrollTop).toBe(720);
    key("ArrowUp");
    expect(app.currentView?.element.scrollTop).toBe(720 - 48);
  });

  it("does not steal keys from the page box", async () => {
    const { app } = make();
    await app.openPath("/book.pdf");
    await flush();
    const input = q<HTMLInputElement>("#page-input");
    input.focus();
    const event = new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true });
    input.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(app.currentView?.element.scrollTop).toBe(0);
  });

  it("opens the file dialog with Ctrl+O and focuses the page box with Ctrl+G", async () => {
    const { app, deps } = make();
    key("o", { ctrlKey: true });
    expect(deps.pickFiles).toHaveBeenCalledTimes(1);
    await app.openPath("/book.pdf");
    await flush();
    key("g", { ctrlKey: true });
    expect(document.activeElement).toBe(q("#page-input"));
  });

  it("zooms from the select box and the buttons", async () => {
    const { app } = make();
    await app.openPath("/book.pdf");
    await flush();
    const select = q<HTMLSelectElement>("#zoom-select");
    select.value = "2";
    select.dispatchEvent(new Event("change"));
    expect(app.currentView?.zoomValue).toBe(2);
    click(q("#zoom-in"));
    expect(app.currentView?.zoomValue).toBe(3);
    click(q("#zoom-out"));
    click(q("#zoom-out"));
    expect(app.currentView?.zoomValue).toBe(1.5);
    select.value = "fit";
    select.dispatchEvent(new Event("change"));
    expect(app.currentView?.zoomMode.kind).toBe("fit-width");
  });

  it("shows the drop hint while a file is dragged over the window", () => {
    const { app } = make();
    app.setDropActive(true);
    expect(q("#drop-overlay").hidden).toBe(false);
    app.setDropActive(false);
    expect(q("#drop-overlay").hidden).toBe(true);
  });
});

describe("App: remembering the position", () => {
  it("opens a file where it was left", async () => {
    const { app, api } = make();
    api.open.mockResolvedValueOnce(opened(1, 10, "long.pdf", { page: 4, offset: 0.5, zoom: 2 }));
    await app.openPath("/long.pdf");
    await flush();
    const view = app.currentView!;
    expect(view.zoomMode).toEqual({ kind: "custom", zoom: 2 });
    expect(view.currentPageIndex).toBe(4);
    expect(q<HTMLInputElement>("#page-input").value).toBe("5");
    // The first pages drawn are the ones at the saved place, not page 1.
    const firstPages = api.render.mock.calls.slice(0, 2).map(([, page]) => page);
    expect(firstPages.every((p) => p >= 3)).toBe(true);
  });

  it("starts at the top, fitted to the width, for a file it has not seen", async () => {
    const { app } = make();
    await app.openPath("/new.pdf");
    await flush();
    expect(app.currentView?.currentPageIndex).toBe(0);
    expect(app.currentView?.zoomMode.kind).toBe("fit-width");
  });

  it("saves the position when the reader rests after scrolling", async () => {
    const { app, api } = make();
    await app.openPath("/read.pdf");
    await flush();
    api.setPosition.mockClear();
    app.currentView!.goToPage(2);
    await flush();
    await new Promise((r) => setTimeout(r, 30));
    expect(api.setPosition).toHaveBeenCalled();
    const [path, position] = api.setPosition.mock.calls.at(-1)!;
    expect(path).toBe("/read.pdf");
    expect(position.page).toBe(2);
    expect(position.zoom).toBeNull();
    expect(position.offset).toBeGreaterThanOrEqual(0);
    expect(position.offset).toBeLessThanOrEqual(1);
  });

  it("saves the position of a tab when another tab is selected and when it is closed", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    app.currentView!.setZoom(2);
    await app.openPath("/b.pdf");
    await flush();
    const forA = api.setPosition.mock.calls.filter(([p]) => p === "/a.pdf");
    expect(forA.at(-1)?.[1].zoom).toBe(2);
    api.setPosition.mockClear();
    click(document.querySelector('[aria-label="Close b.pdf"]'));
    await flush();
    expect(api.setPosition.mock.calls.some(([p]) => p === "/b.pdf")).toBe(true);
  });

  it("keeps the place of a tab that is behind another one", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    app.currentView!.goToPage(2);
    await flush();
    await app.openPath("/b.pdf");
    await flush();
    click(tab("a.pdf"));
    await flush();
    expect(app.currentView?.currentPageIndex).toBe(2);
  });

  it("forgets the remembered positions only after a confirmation", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    click(q("#menu-button"));
    click(q("#forget-positions"));
    await flush();
    expect(notice()?.textContent).toContain("Forget remembered positions?");
    // The safe answer has the focus, so a quick Enter keeps everything.
    expect(document.activeElement).toBe(button("Keep them"));
    click(button("Keep them"));
    await flush();
    expect(api.clearPositions).not.toHaveBeenCalled();

    click(q("#forget-positions"));
    await flush();
    click(button("Forget positions"));
    await flush();
    expect(api.clearPositions).toHaveBeenCalledTimes(1);
    expect(app.tabCount).toBe(1);
  });
});

describe("App: the side panel", () => {
  it("is closed at start and costs nothing", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    expect(q("#sidebar").hidden).toBe(true);
    expect(document.querySelectorAll(".thumb").length).toBe(0);
    expect(api.getOutline).not.toHaveBeenCalled();
    // Only the main view drew pages.
    expect(api.render.mock.calls.every(([, , w]) => w > 300)).toBe(true);
  });

  it("shows thumbnails of the document in front, after the first page is drawn", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    key("b", { ctrlKey: true });
    expect(q("#sidebar").hidden).toBe(false);
    await new Promise((r) => setTimeout(r, 30));
    await flush();
    const thumbCalls = api.render.mock.calls.filter(([, , w]) => w <= 300);
    expect(thumbCalls.length).toBeGreaterThan(0);
    expect(thumbCalls.every(([doc]) => doc === 1)).toBe(true);
    expect(document.querySelectorAll(".thumb").length).toBe(3);
    key("b", { ctrlKey: true });
    expect(q("#sidebar").hidden).toBe(true);
  });

  it("goes to the page of a thumbnail and follows the page in view", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    click(q("#sidebar-toggle"));
    q('.thumb[data-page="2"]').click();
    await flush();
    expect(app.currentView?.currentPageIndex).toBe(2);
    expect(q('.thumb[aria-current="page"]').dataset.page).toBe("2");
  });

  it("shows the table of contents and goes to an entry", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    click(q("#sidebar-toggle"));
    click(q("#side-tab-contents"));
    await flush();
    expect(api.getOutline).toHaveBeenCalledWith(1);
    [...document.querySelectorAll<HTMLElement>(".outline-link")].find((l) => l.textContent === "End")?.click();
    await flush();
    expect(app.currentView?.currentPageIndex).toBe(2);
  });

  it("follows the tab in front", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await flush();
    click(q("#sidebar-toggle"));
    click(q("#side-tab-contents"));
    await flush();
    expect(api.getOutline).toHaveBeenLastCalledWith(2);
    click(tab("a.pdf"));
    await flush();
    expect(api.getOutline).toHaveBeenLastCalledWith(1);
    // A closed tab's contents are forgotten, and nothing is asked for a closed document.
    key("w", { ctrlKey: true });
    await flush();
    expect(api.getOutline).toHaveBeenCalledTimes(2);
  });

  it("is empty when no document is open", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    click(q("#sidebar-toggle"));
    await flush();
    key("w", { ctrlKey: true });
    await flush();
    expect(document.querySelectorAll(".thumb").length).toBe(0);
  });
});

describe("App: search", () => {
  const findInput = () => q<HTMLInputElement>("#find-input");

  async function search(text: string) {
    findInput().value = text;
    findInput().dispatchEvent(new Event("input"));
    await new Promise((r) => setTimeout(r, 120));
    await flush();
  }

  it("opens with Ctrl+F, searches the document in front and shows the result", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    expect(q<HTMLButtonElement>("#find-button").disabled).toBe(false);
    key("f", { ctrlKey: true });
    expect(q("#findbar").hidden).toBe(false);
    expect(document.activeElement).toBe(findInput());
    await search("hello");
    expect(api.searchPage).toHaveBeenCalledWith(1, 0, "hello");
    expect(q("#find-status").textContent).toBe("1 of 1");
    await flush();
    // The match is on page 2: the view went there.
    expect(app.currentView?.currentPageIndex).toBe(1);
  });

  it("closes with Escape and clears the highlights", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    key("f", { ctrlKey: true });
    await search("hello");
    findInput().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(q("#findbar").hidden).toBe(true);
    expect(document.querySelectorAll(".hl-find")).toHaveLength(0);
  });

  it("is off while no document is open and goes with the tab", async () => {
    const { app } = make();
    expect(q<HTMLButtonElement>("#find-button").disabled).toBe(true);
    key("f", { ctrlKey: true });
    expect(q("#findbar").hidden).toBe(true);
    await app.openPath("/a.pdf");
    await flush();
    key("w", { ctrlKey: true });
    await flush();
    expect(q<HTMLButtonElement>("#find-button").disabled).toBe(true);
  });

  it("keeps Ctrl+A for the find box", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    key("f", { ctrlKey: true });
    const event = new KeyboardEvent("keydown", { key: "a", ctrlKey: true, bubbles: true, cancelable: true });
    findInput().dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(app.currentView?.hasSelection).toBe(false);
  });
});

describe("App: selecting and copying text", () => {
  function copyEvent() {
    const setData = vi.fn();
    const event = new Event("copy", { bubbles: true, cancelable: true }) as Event & {
      clipboardData: { setData: typeof setData };
    };
    event.clipboardData = { setData };
    return { event, setData };
  }

  it("selects all with Ctrl+A and copies the text on Copy", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    expect(app.currentView?.hasSelection).toBe(true);
    const { event, setData } = copyEvent();
    document.body.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(setData).toHaveBeenCalledWith("text/plain", "Hello world\nHello world\nHello world");
    // The text is also real, selected text off screen, so the Copy of the system has something to copy.
    expect(q("#sel-proxy").textContent).toBe("Hello world\nHello world\nHello world");
    expect(String(document.getSelection())).toContain("Hello world");
  });

  it("clears the selection with Escape and leaves Copy alone when nothing is selected", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    key("Escape");
    expect(app.currentView?.hasSelection).toBe(false);
    expect(q("#sel-proxy").textContent).toBe("");
    const { event, setData } = copyEvent();
    document.body.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    expect(setData).not.toHaveBeenCalled();
  });

  it("leaves Copy in a text box to the box", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    const { event, setData } = copyEvent();
    q("#page-input").dispatchEvent(event);
    expect(setData).not.toHaveBeenCalled();
  });
});

describe("App: links in a document", () => {
  const links = () => [...document.querySelectorAll<HTMLElement>(".page-link")];

  it("asks before it opens a web address, shows the whole address, and does nothing on Cancel", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    links()[0].click();
    await flush();
    expect(notice()?.textContent).toContain("Open this link in your browser?");
    expect(notice()?.textContent).toContain("example.com");
    expect(document.querySelector(".sheet-url")?.textContent).toBe("https://example.com/a");
    // The safe answer has the focus.
    expect(document.activeElement).toBe(button("Cancel"));
    expect(api.openLink).not.toHaveBeenCalled();
    click(button("Cancel"));
    await flush();
    expect(api.openLink).not.toHaveBeenCalled();
  });

  it("opens the address only after the user says so", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    links()[0].click();
    await flush();
    click(button("Open in browser"));
    await flush();
    expect(api.openLink).toHaveBeenCalledWith(1, "https://example.com/a");
  });

  it("never opens anything but http and https: another kind is explained and not offered", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    links()[1].click();
    await flush();
    expect(notice()?.textContent).toContain("This link is not opened");
    expect(notice()?.textContent).toContain("file:");
    expect(button("Open in browser")).toBeNull();
    click(button("OK"));
    await flush();
    expect(api.openLink).not.toHaveBeenCalled();
  });

  it("goes to the page of a link inside the document without asking", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    links()[2].click();
    await flush();
    expect(notice()).toBeNull();
    expect(app.currentView?.currentPageIndex).toBe(2);
    expect(api.openLink).not.toHaveBeenCalled();
  });

  it("tells the user when the browser could not be started", async () => {
    const { app, api } = make();
    api.openLink.mockRejectedValueOnce({ code: "link_refused", message: "no" });
    await app.openPath("/a.pdf");
    await flush();
    links()[0].click();
    await flush();
    click(button("Open in browser"));
    await flush();
    expect(notice()?.textContent).toContain("could not be opened");
  });
});

describe("App: printing", () => {
  const printButton = () => q<HTMLButtonElement>("#print-button");
  const submitDialog = () => document.querySelector(".print-dialog")?.dispatchEvent(new Event("submit", { cancelable: true }));
  const radio = (value: string) => q<HTMLInputElement>(`.print-dialog input[value="${value}"]`);

  it("is off while no document is open", async () => {
    const { app } = make();
    expect(printButton().disabled).toBe(true);
    key("p", { ctrlKey: true });
    expect(document.querySelector(".print-dialog")).toBeNull();
    await app.openPath("/a.pdf");
    await flush();
    expect(printButton().disabled).toBe(false);
  });

  it("asks which pages, prepares them at print size and opens the print dialog", async () => {
    const { app, api, deps } = make();
    await app.openPath("/book.pdf");
    await flush();
    api.render.mockClear();
    key("p", { ctrlKey: true });
    expect(document.querySelector(".print-dialog")?.textContent).toContain("book.pdf has 3 pages");
    radio("current").checked = true;
    radio("current").dispatchEvent(new Event("change"));
    submitDialog();
    await flush();
    await flush();
    // One page, drawn for paper: 150 dpi, not for the screen.
    expect(api.render).toHaveBeenCalledTimes(1);
    expect(api.render.mock.calls[0]).toEqual([1, 0, Math.round((612 / 72) * 150), Math.round((792 / 72) * 150)]);
    expect(deps.print.print).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll("#print-root img")).toHaveLength(1);
    expect(notice()).toBeNull();
    // When printing is over, the pictures go.
    window.dispatchEvent(new Event("afterprint"));
    expect(document.querySelectorAll("#print-root img")).toHaveLength(0);
    expect(deps.print.revokeUrl).toHaveBeenCalledTimes(1);
  });

  it("prints all the pages", async () => {
    const { app, api, deps } = make();
    await app.openPath("/book.pdf");
    await flush();
    api.render.mockClear();
    click(printButton());
    submitDialog();
    await flush();
    await flush();
    expect(api.render.mock.calls.map((c) => c[1])).toEqual([0, 1, 2]);
    expect(document.querySelectorAll("#print-root img")).toHaveLength(3);
    expect(deps.print.print).toHaveBeenCalledTimes(1);
  });

  it("does nothing when the question is cancelled", async () => {
    const { app, api, deps } = make();
    await app.openPath("/book.pdf");
    await flush();
    api.render.mockClear();
    key("p", { ctrlKey: true });
    document.querySelector<HTMLElement>(".print-dialog .btn-secondary")?.click();
    await flush();
    expect(api.render).not.toHaveBeenCalled();
    expect(deps.print.print).not.toHaveBeenCalled();
    expect(document.querySelector(".print-dialog")).toBeNull();
  });

  it("says so when a page cannot be drawn, and sends nothing to the printer", async () => {
    const { app, api, deps } = make();
    await app.openPath("/book.pdf");
    await flush();
    api.render.mockRejectedValueOnce({ code: "engine_stopped", message: "it exited" });
    click(printButton());
    submitDialog();
    await flush();
    await flush();
    expect(notice()?.textContent).toContain("could not be printed");
    expect(notice()?.textContent).toContain("Nothing was sent to the printer");
    expect(deps.print.print).not.toHaveBeenCalled();
  });

  it("asks only once at a time", async () => {
    const { app } = make();
    await app.openPath("/book.pdf");
    await flush();
    key("p", { ctrlKey: true });
    key("p", { ctrlKey: true });
    expect(document.querySelectorAll(".print-dialog")).toHaveLength(1);
  });
});

describe("App: the colour theme", () => {
  const radio = (value: string) => q<HTMLInputElement>(`#theme-choice input[value="${value}"]`);

  it("follows the system when nothing is chosen", () => {
    make();
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    expect(radio("auto").checked).toBe(true);
  });

  it("forces dark or light when the user chooses, and remembers the choice", () => {
    const { storage } = make();
    radio("dark").checked = true;
    radio("dark").dispatchEvent(new Event("change"));
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(storage.data.get(THEME_KEY)).toBe("dark");
    radio("light").checked = true;
    radio("light").dispatchEvent(new Event("change"));
    expect(document.documentElement.getAttribute("data-theme")).toBe("light");
    radio("auto").checked = true;
    radio("auto").dispatchEvent(new Event("change"));
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    expect(storage.data.get(THEME_KEY)).toBe("auto");
  });

  it("applies the stored choice at start", () => {
    const storage = new MemoryStorage();
    storage.setItem(THEME_KEY, "dark");
    make({ storage });
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(radio("dark").checked).toBe(true);
  });

  it("opens and closes the settings menu", () => {
    make();
    const menu = q("#menu");
    expect(menu.hidden).toBe(true);
    click(q("#menu-button"));
    expect(menu.hidden).toBe(false);
    expect(q("#menu-button").getAttribute("aria-expanded")).toBe("true");
    menu.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(menu.hidden).toBe(true);
    click(q("#menu-button"));
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    expect(menu.hidden).toBe(true);
  });
});

describe("App: measurement mode", () => {
  it("reports the open and first paint times once", async () => {
    let t = 0;
    const { app, api, deps } = make({ measureOn: true });
    deps.now.mockImplementation(() => (t += 10));
    await flush();
    await app.openPath("/book.pdf");
    await flush();
    expect(api.measureReport).toHaveBeenCalledTimes(1);
    const [openMs, paintMs, pages] = api.measureReport.mock.calls[0];
    expect(pages).toBe(3);
    expect(paintMs).toBeGreaterThanOrEqual(openMs);
  });
});

describe("App: a link question cannot be answered by a press already on its way", () => {
  const links = () => [...document.querySelectorAll<HTMLElement>(".page-link")];

  it("keeps the primary button off for a moment, so the second press of a double click does nothing", async () => {
    const { app, api } = make({ realLinkDelay: true });
    await app.openPath("/a.pdf");
    await flush();
    links()[0].click();
    await flush();
    const open = button("Open in browser");
    expect(open?.disabled).toBe(true);
    // The second press of the double click lands on the button, a moment later.
    click(open);
    await flush();
    expect(api.openLink).not.toHaveBeenCalled();
    expect(notice()).not.toBeNull();
    // The safe answer is there at once.
    expect(button("Cancel")?.disabled).toBe(false);
    expect(document.activeElement).toBe(button("Cancel"));
    // A real answer, given after the delay, works.
    await new Promise((r) => setTimeout(r, 800));
    expect(button("Open in browser")?.disabled).toBe(false);
    click(button("Open in browser"));
    await flush();
    expect(api.openLink).toHaveBeenCalledWith(1, "https://example.com/a");
  });

  it("is safe with the keyboard: Enter and Space, held down, only reach Cancel", async () => {
    const { app, api } = make({ realLinkDelay: true });
    await app.openPath("/a.pdf");
    await flush();
    links()[0].click();
    await flush();
    for (const k of ["Enter", " "]) {
      const held = new KeyboardEvent("keydown", { key: k, repeat: true, bubbles: true, cancelable: true });
      document.activeElement?.dispatchEvent(held);
      expect(held.defaultPrevented, k).toBe(true);
    }
    expect(api.openLink).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(button("Cancel"));
  });
});

describe("App: forgetting positions also holds for the files that are open", () => {
  const closeButton = (name: string) => document.querySelector(`[aria-label="Close ${name}"]`);

  async function forget() {
    click(q("#menu-button"));
    click(q("#forget-positions"));
    await flush();
    click(button("Forget positions"));
    await flush();
  }

  it("does not write the place of an open file back when its tab is closed", async () => {
    const { app, api } = make();
    await app.openPath("/secret.pdf");
    await flush();
    app.currentView!.goToPage(1);
    await flush();
    await forget();
    api.setPosition.mockClear();
    click(closeButton("secret.pdf"));
    await flush();
    expect(api.setPosition).not.toHaveBeenCalled();
  });

  it("does not write it back when another tab is chosen, when the window is hidden, or on a repaint", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await flush();
    click(tab("a.pdf"));
    await flush();
    app.currentView!.goToPage(1);
    await flush();
    await forget();
    api.setPosition.mockClear();
    click(tab("b.pdf"));
    document.dispatchEvent(new Event("visibilitychange"));
    window.dispatchEvent(new Event("pagehide"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.setPosition).not.toHaveBeenCalled();
  });

  it("remembers the place again once the reader moves", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    await forget();
    api.setPosition.mockClear();
    app.currentView!.goToPage(2);
    await flush();
    await new Promise((r) => setTimeout(r, 30));
    expect(api.setPosition).toHaveBeenCalled();
    expect(api.setPosition.mock.calls.at(-1)?.[1].page).toBe(2);
  });

  it("treats a file opened after the forgetting like any other", async () => {
    const { app, api } = make();
    await forget();
    await app.openPath("/new.pdf");
    await flush();
    api.setPosition.mockClear();
    click(closeButton("new.pdf"));
    await flush();
    expect(api.setPosition).toHaveBeenCalledTimes(1);
  });
});

describe("App: copying says when it could not copy everything", () => {
  function clipboard(writeText: (text: string) => Promise<void> | undefined) {
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: { writeText } });
  }
  afterEach(() => {
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: undefined });
  });
  const copy = () => {
    const event = new Event("copy", { bubbles: true, cancelable: true }) as Event & { clipboardData: unknown };
    event.clipboardData = { setData: vi.fn() };
    document.body.dispatchEvent(event);
  };

  /** A document of 30 pages, so that most of its text is not loaded when everything is selected. */
  const longDoc = (api: { open: { mockImplementationOnce(f: () => Promise<OpenedDocument>): unknown } }) =>
    api.open.mockImplementationOnce(() => Promise.resolve(opened(1, 30, "long.pdf")));
  const manyLines = Array.from({ length: 30 }, () => "Hello world").join("\n");

  it("shows how many pages were copied when the copy was cut at the limit", async () => {
    const write = vi.fn(() => Promise.resolve());
    clipboard(write);
    // Room for one page only. Nothing is loaded ahead, so Copy has to fetch the pages itself.
    const { app, api } = make({ viewLimits: { preloadChars: 0, copyChars: 15 } });
    longDoc(api);
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copy();
    await flush();
    expect(write).toHaveBeenCalledWith("Hello world");
    expect(notice()?.textContent).toContain("Only part of the text was copied");
    expect(notice()?.textContent).toContain("1 of 30 pages");
    expect(notice()?.textContent).not.toContain("could not be copied");
  });

  it("says so when pages cannot be read", async () => {
    clipboard(vi.fn(() => Promise.resolve()));
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    longDoc(api);
    api.getPageText.mockImplementation((_d: number, p: number) =>
      p === 20
        ? Promise.reject(new Error("no"))
        : Promise.resolve({ text: "Hello world", rects: layoutRects("Hello world") }),
    );
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copy();
    await flush();
    expect(notice()?.textContent).toContain("Only part of the text was copied");
    expect(notice()?.textContent).toContain("could not be read");
  });

  it("says so when the clipboard refuses the text", async () => {
    clipboard(vi.fn(() => Promise.reject(new Error("not allowed"))));
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    longDoc(api);
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copy();
    await flush();
    expect(notice()?.textContent).toContain("could not be copied");
  });

  it("says so when there is no clipboard", async () => {
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    longDoc(api);
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copy();
    await flush();
    expect(notice()?.textContent).toContain("could not be copied");
  });

  it("is quiet when everything was copied", async () => {
    const write = vi.fn(() => Promise.resolve());
    clipboard(write);
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    longDoc(api);
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copy();
    await flush();
    expect(write).toHaveBeenCalledWith(manyLines);
    expect(notice()).toBeNull();
  });
});

describe("App: smaller behaviours", () => {
  it("moves between matches with F3 also while the find box has the focus", async () => {
    const { app, api } = make();
    // Two matches, so that stepping on shows.
    api.searchPage.mockImplementation((_d: number, page: number) =>
      Promise.resolve(page === 1 ? ([[0, 5], [6, 5]] as [number, number][]) : []),
    );
    await app.openPath("/a.pdf");
    await flush();
    key("f", { ctrlKey: true });
    const input = q<HTMLInputElement>("#find-input");
    input.value = "hello";
    input.dispatchEvent(new Event("input"));
    await new Promise((r) => setTimeout(r, 120));
    await flush();
    expect(q("#find-status").textContent).toBe("1 of 2");
    const f3 = new KeyboardEvent("keydown", { key: "F3", bubbles: true, cancelable: true });
    input.dispatchEvent(f3);
    expect(f3.defaultPrevented).toBe(true);
    expect(q("#find-status").textContent).toBe("2 of 2");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "F3", shiftKey: true, bubbles: true, cancelable: true }));
    expect(q("#find-status").textContent).toBe("1 of 2");
  });

  it("drops the print pictures of a document when its tab closes, also when other tabs stay open", async () => {
    const { app, api, deps } = make();
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await flush();
    click(tab("a.pdf"));
    await flush();
    api.render.mockClear();
    key("p", { ctrlKey: true });
    document.querySelector(".print-dialog")?.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    await flush();
    expect(document.querySelectorAll("#print-root img").length).toBeGreaterThan(0);
    key("w", { ctrlKey: true });
    await flush();
    expect(app.tabCount).toBe(1);
    expect(document.querySelectorAll("#print-root img")).toHaveLength(0);
    expect(deps.print.revokeUrl).toHaveBeenCalled();
  });

  it("keeps the file dialog to the same limit as a drop", async () => {
    const { app, deps, api } = make();
    deps.pickFiles.mockResolvedValueOnce(Array.from({ length: 20 }, (_, i) => `/f${i}.pdf`));
    await app.openDialog();
    await flush();
    expect(api.open).toHaveBeenCalledTimes(8);
  });
});

describe("App: pages with more text than the window keeps", () => {
  it("tells the reader once, and keeps the pages that fit", async () => {
    const { app } = make({ viewLimits: { keptChars: 20 } });
    await app.openPath("/a.pdf");
    await flush();
    expect(notice()?.textContent).toContain("Some pages have a lot of text");
    expect(notice()?.textContent).toContain("a.pdf");
    click(button("OK"));
    await flush();
    // Scrolling about does not bring the notice back.
    app.currentView!.goToPage(2);
    await flush();
    app.currentView!.goToPage(0);
    await flush();
    expect(notice()).toBeNull();
    expect(app.currentView!.keptTextChars).toBeLessThanOrEqual(20);
  });

  it("says nothing for an ordinary document", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    expect(notice()).toBeNull();
  });
});

describe("App: text in a script that needs a font", () => {
  const japanese = "かな日本語";

  function withJapanese(api: ReturnType<typeof make>["api"]) {
    api.getPageText.mockImplementation(() =>
      Promise.resolve({ text: japanese, rects: layoutRects(japanese) }),
    );
  }

  it("tells the reader once when the engine had to refuse a font for it", async () => {
    const { app, api } = make();
    withJapanese(api);
    api.missedFonts.mockResolvedValue(["japanese", "thai"]);
    await app.openPath("/a.pdf");
    await flush();
    expect(notice()?.textContent).toContain("Some text may not show");
    expect(notice()?.textContent).toContain("Japanese and Thai");
    expect(notice()?.textContent).toContain("a.pdf");
    expect(notice()?.textContent).toContain("open the file again");
    click(button("OK"));
    await flush();
    app.currentView!.goToPage(2);
    await flush();
    app.currentView!.goToPage(0);
    await flush();
    expect(notice()).toBeNull();
  });

  it("says nothing when the engine had every font it was asked for", async () => {
    const { app, api } = make();
    withJapanese(api);
    await app.openPath("/a.pdf");
    await flush();
    expect(notice()).toBeNull();
    // It did look: text in the script is a reason to ask.
    expect(api.missedFonts).toHaveBeenCalled();
  });

  it("asks again after a page is drawn, because the fonts are asked for while it is drawn", async () => {
    const { app, api } = make();
    withJapanese(api);
    api.missedFonts.mockResolvedValueOnce([]).mockResolvedValue(["korean"]);
    await app.openPath("/a.pdf");
    await flush();
    app.currentView!.goToPage(1);
    await flush();
    expect(notice()?.textContent).toContain("Korean");
  });

  it("says nothing for Latin text, and does not ask the engine", async () => {
    const { app, api } = make();
    await app.openPath("/a.pdf");
    await flush();
    expect(notice()).toBeNull();
    expect(api.missedFonts).not.toHaveBeenCalled();
  });

  it("does not make a message of an engine that cannot say", async () => {
    const { app, api } = make();
    withJapanese(api);
    api.missedFonts.mockRejectedValue(new Error("gone"));
    await app.openPath("/a.pdf");
    await flush();
    expect(notice()).toBeNull();
  });
});

describe("App: copying when a page was cut, closed on the way, or pressed twice", () => {
  function clipboard(writeText: (text: string) => Promise<void>) {
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: { writeText } });
  }
  afterEach(() => {
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: undefined });
  });
  const copyEvent = () => {
    const event = new Event("copy", { bubbles: true, cancelable: true }) as Event & { clipboardData: unknown };
    const setData = vi.fn();
    event.clipboardData = { setData };
    document.body.dispatchEvent(event);
    return setData;
  };
  const longDoc = (api: { open: { mockImplementationOnce(f: () => Promise<OpenedDocument>): unknown } }, pages = 30) =>
    api.open.mockImplementationOnce(() => Promise.resolve(opened(1, pages, "long.pdf")));

  it("tells the reader when the engine cut a page, also when everything was loaded", async () => {
    const { app, api } = make();
    api.getPageText.mockImplementation(() =>
      Promise.resolve({ text: "z".repeat(100_000), rects: Array.from({ length: 400_000 }, () => 1) }),
    );
    longDoc(api, 1);
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    const setData = copyEvent();
    await flush();
    expect(setData).toHaveBeenCalledTimes(1);
    expect(notice()?.textContent).toContain("Only part of the text was copied");
    expect(notice()?.textContent).toContain("more text than the reader can handle");
  });

  it("writes the clipboard once when Copy is pressed again while a copy is running", async () => {
    const write = vi.fn(() => Promise.resolve());
    clipboard(write);
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    longDoc(api);
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copyEvent();
    copyEvent();
    copyEvent();
    await flush();
    await flush();
    expect(write).toHaveBeenCalledTimes(1);
    expect(notice()).toBeNull();
  });

  it("shows how far a long copy has got, and takes the line away when it is over", async () => {
    clipboard(vi.fn(() => Promise.resolve()));
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    longDoc(api);
    let release: () => void = () => undefined;
    const gate = new Promise<void>((r) => (release = r));
    api.getPageText.mockImplementation(async (_d: number, p: number) => {
      if (p === 25) await gate;
      return { text: "Hello world", rects: layoutRects("Hello world") };
    });
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    expect(q("#copy-status").hidden).toBe(true);
    copyEvent();
    await flush();
    expect(q("#copy-status").hidden).toBe(false);
    expect(q("#copy-status").textContent).toMatch(/^Copying the text: page \d+ of 30$/);
    release();
    await flush();
    await flush();
    expect(q("#copy-status").hidden).toBe(true);
    expect(q("#copy-status").textContent).toBe("");
  });

  it("keeps the line while a second press finds the first copy still running", async () => {
    clipboard(vi.fn(() => Promise.resolve()));
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    longDoc(api);
    let release: () => void = () => undefined;
    const gate = new Promise<void>((r) => (release = r));
    api.getPageText.mockImplementation(async (_d: number, p: number) => {
      if (p === 25) await gate;
      return { text: "Hello world", rects: layoutRects("Hello world") };
    });
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copyEvent();
    await flush();
    copyEvent();
    await flush();
    expect(q("#copy-status").hidden).toBe(false);
    release();
    await flush();
    await flush();
    expect(q("#copy-status").hidden).toBe(true);
  });

  it("says so when the selection changed during the copy and nothing was copied", async () => {
    const write = vi.fn(() => Promise.resolve());
    clipboard(write);
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    longDoc(api);
    let release: () => void = () => undefined;
    const gate = new Promise<void>((r) => (release = r));
    api.getPageText.mockImplementation(async (_d: number, p: number) => {
      if (p === 25) await gate;
      return { text: "Hello world", rects: layoutRects("Hello world") };
    });
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copyEvent();
    await flush();
    key("Escape");
    release();
    await flush();
    await flush();
    expect(write).not.toHaveBeenCalled();
    expect(notice()?.textContent).toContain("The text was not copied");
    expect(notice()?.textContent).toContain("selection changed");
    expect(q("#copy-status").hidden).toBe(true);
  });

  it("leaves the clipboard as it was when the selection holds no text", async () => {
    const write = vi.fn(() => Promise.resolve());
    clipboard(write);
    const { app, api } = make();
    api.getPageText.mockImplementation(() => Promise.resolve({ text: "", rects: [] }));
    longDoc(api, 2);
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    const event = new Event("copy", { bubbles: true, cancelable: true }) as Event & { clipboardData: unknown };
    const setData = vi.fn();
    event.clipboardData = { setData };
    document.body.dispatchEvent(event);
    expect(setData).not.toHaveBeenCalled();
    expect(event.defaultPrevented).toBe(false);
    await flush();
    expect(notice()).toBeNull();
  });

  it("leaves the clipboard alone when the tab is closed during the copy", async () => {
    const write = vi.fn(() => Promise.resolve());
    clipboard(write);
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    longDoc(api);
    let release: () => void = () => undefined;
    const gate = new Promise<void>((r) => (release = r));
    api.getPageText.mockImplementation(async (_d: number, p: number) => {
      if (p === 25) await gate;
      return { text: "Hello world", rects: layoutRects("Hello world") };
    });
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copyEvent();
    await flush();
    key("w", { ctrlKey: true });
    await flush();
    release();
    await flush();
    await flush();
    expect(write).not.toHaveBeenCalled();
    expect(notice()).toBeNull();
  });
});

describe("App: the menu of the right mouse button", () => {
  function clipboard(writeText: (text: string) => Promise<void>) {
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: { writeText } });
  }
  afterEach(() => {
    Object.defineProperty(window.navigator, "clipboard", { configurable: true, value: undefined });
  });
  const rightClick = (target: Element) => {
    const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 50, clientY: 60 });
    target.dispatchEvent(e);
    return e;
  };

  it("offers Copy on selected text, and Copy puts the text on the clipboard", async () => {
    const write = vi.fn(() => Promise.resolve());
    clipboard(write);
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    const e = rightClick(app.currentView!.element);
    expect(e.defaultPrevented).toBe(true);
    expect(q("#context-menu").hidden).toBe(false);
    expect(document.activeElement).toBe(q("#context-copy"));
    click(q("#context-copy"));
    await flush();
    expect(write).toHaveBeenCalledWith("Hello world\nHello world\nHello world");
    expect(q("#context-menu").hidden).toBe(true);
  });

  it("writes nothing when Copy is chosen on a selection that holds no text", async () => {
    const write = vi.fn(() => Promise.resolve());
    clipboard(write);
    const { app, api } = make();
    api.getPageText.mockImplementation(() => Promise.resolve({ text: "", rects: [] }));
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    rightClick(app.currentView!.element);
    click(q("#context-copy"));
    await flush();
    expect(write).not.toHaveBeenCalled();
    expect(notice()).toBeNull();
  });

  it("offers nothing when no text is selected", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    rightClick(app.currentView!.element);
    expect(q("#context-menu").hidden).toBe(true);
  });

  it("leaves the menu of the web view to text boxes, and closes on Escape", async () => {
    const { app } = make();
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    expect(rightClick(q("#find-input")).defaultPrevented).toBe(false);
    expect(q("#context-menu").hidden).toBe(true);
    rightClick(app.currentView!.element);
    expect(q("#context-menu").hidden).toBe(false);
    q("#context-menu").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(q("#context-menu").hidden).toBe(true);
  });
});

describe("App: the files that come back after an update", () => {
  it("are all opened, up to the limit of the app and not the limit of the file dialog, and one without .pdf too", async () => {
    const { app, api } = make();
    const paths = [...Array.from({ length: 11 }, (_, i) => `/docs/f${i}.pdf`), "/docs/no-ending", "/docs/other.pdf"];
    await app.restorePaths(paths);
    await flush();
    expect(api.open).toHaveBeenCalledTimes(13);
    expect(app.tabCount).toBe(13);
    expect(notice()).toBeNull();
    // The first one is the one in view.
    expect(document.querySelector('.tab-title[aria-selected="true"]')?.textContent).toBe("f0.pdf");
  });

  it("do nothing when there is nothing to restore", async () => {
    const { app, api } = make();
    await app.restorePaths([]);
    expect(api.open).not.toHaveBeenCalled();
  });

  it("come back through the same open path as every file, so a protected file asks for its password", async () => {
    const { app, api } = make();
    api.open.mockRejectedValueOnce({ code: "password_required", message: "locked" });
    const done = app.restorePaths(["/docs/locked.pdf"]);
    await flush();
    expect(notice()?.textContent).toContain("password");
    click(button("Cancel"));
    await done;
  });
});

describe("App: the file dialog says when it leaves files out", () => {
  it("tells how many of 20 files were not opened", async () => {
    const { app, deps, api } = make();
    deps.pickFiles.mockResolvedValueOnce(Array.from({ length: 20 }, (_, i) => `/f${i}.pdf`));
    await app.openDialog();
    await flush();
    expect(api.open).toHaveBeenCalledTimes(8);
    expect(notice()?.textContent).toContain("Not all files were opened");
    expect(notice()?.textContent).toContain("12 files were left out");
  });

  it("says nothing when everything is opened", async () => {
    const { app, deps } = make();
    deps.pickFiles.mockResolvedValueOnce(["/a.pdf", "/b.pdf"]);
    await app.openDialog();
    await flush();
    expect(notice()).toBeNull();
  });

  it("does not count files that are not PDFs next to PDFs as left out by the limit", async () => {
    const { app, deps } = make();
    deps.pickFiles.mockResolvedValueOnce(["/a.pdf", "/notes.txt"]);
    await app.openDialog();
    await flush();
    expect(notice()).toBeNull();
  });
});

describe("App: reopening the tabs of the last session", () => {
  const saved = { paths: ["/docs/a.pdf", "/docs/b.pdf", "/docs/c.pdf"], active: 1 };

  it("opens the tabs in their old order with the same one in view, each at its remembered place", async () => {
    const { app, api } = make();
    const places: Record<string, SavedPosition> = {
      "/docs/a.pdf": { page: 1, offset: 0.25, zoom: null },
      "/docs/b.pdf": { page: 2, offset: 0, zoom: null },
      "/docs/c.pdf": { page: 0, offset: 0.5, zoom: 2 },
    };
    api.open.mockImplementation((path: string) =>
      // The slowest file is the first one: the order of the tabs must not follow the order of arrival.
      new Promise((resolve) =>
        setTimeout(() => resolve(opened(path.charCodeAt(6), 3, path.split("/").pop(), places[path])), path === "/docs/a.pdf" ? 20 : 0),
      ),
    );
    await app.start({ session: saved });
    await flush();
    expect(tabTitles()).toEqual(["a.pdf", "b.pdf", "c.pdf"]);
    expect(selectedTab()).toBe("b.pdf");
    expect(app.currentView?.currentPageIndex).toBe(2);
    click(tab("a.pdf"));
    await flush();
    expect(app.currentView?.currentPageIndex).toBe(1);
    expect(notice()).toBeNull();
  });

  it("opens the tab that was in view first and the others one after the other behind it", async () => {
    const { app, api } = make();
    let running = 0;
    let mostAtOnce = 0;
    api.open.mockImplementation(async (path: string) => {
      running++;
      mostAtOnce = Math.max(mostAtOnce, running);
      await new Promise((r) => setTimeout(r, 2));
      running--;
      return opened(path.charCodeAt(6), 3, path.split("/").pop());
    });
    await app.start({ session: saved });
    expect(api.open.mock.calls.map(([path]) => path)).toEqual(["/docs/b.pdf", "/docs/a.pdf", "/docs/c.pdf"]);
    expect(mostAtOnce).toBe(1);
  });

  it("opens the files named at the start too and gives them the focus", async () => {
    const { app } = make();
    await app.start({ session: saved, given: ["/docs/new.pdf"] });
    await flush();
    expect(tabTitles()).toEqual(["a.pdf", "b.pdf", "c.pdf", "new.pdf"]);
    expect(selectedTab()).toBe("new.pdf");
  });

  it("shows a named file that is also in the list instead of opening it twice", async () => {
    const { app, api } = make();
    await app.start({ session: saved, given: ["/docs/c.pdf"] });
    await flush();
    expect(tabTitles()).toEqual(["a.pdf", "b.pdf", "c.pdf"]);
    expect(selectedTab()).toBe("c.pdf");
    expect(api.open.mock.calls.filter(([path]) => path === "/docs/c.pdf")).toHaveLength(1);
  });

  it("puts a named file that is in the list back where it was, not at the end", async () => {
    const { app } = make();
    await app.start({ session: saved, given: ["/docs/a.pdf"] });
    await flush();
    expect(tabTitles()).toEqual(["a.pdf", "b.pdf", "c.pdf"]);
    expect(selectedTab()).toBe("a.pdf");
  });

  it("keeps all 16 tabs when a file named at the start is one of them", async () => {
    const { app, api } = make();
    const full = Array.from({ length: 16 }, (_, i) => `/docs/f${i}.pdf`);
    await app.start({ session: { paths: full, active: 0 }, given: ["/docs/f7.pdf"] });
    await flush();
    expect(app.tabCount).toBe(16);
    expect(api.open).toHaveBeenCalledTimes(16);
    expect(notice()).toBeNull();
    expect(selectedTab()).toBe("f7.pdf");
  });

  it("opens nothing, and says nothing, when there is nothing to reopen", async () => {
    const { app, api } = make();
    await app.start({ session: { paths: [], active: null } });
    await app.start({});
    expect(api.open).not.toHaveBeenCalled();
    expect(notice()).toBeNull();
    expect(q("#empty").hidden).toBe(false);
  });

  it("skips files that are gone with one calm notice that counts them", async () => {
    const { app, api } = make();
    api.open.mockImplementation((path: string) =>
      path === "/docs/b.pdf" || path === "/docs/c.pdf"
        ? Promise.reject({ code: "io", message: "no such file" })
        : Promise.resolve(opened(1, 3, path.split("/").pop())),
    );
    await app.start({ session: { paths: ["/docs/a.pdf", "/docs/b.pdf", "/docs/c.pdf"], active: 0 } });
    await flush();
    expect(tabTitles()).toEqual(["a.pdf"]);
    expect(document.querySelectorAll(".notice")).toHaveLength(1);
    expect(notice()?.textContent).toContain("2 tabs from your last session could not be opened");
    // One OK is the only thing to press: no question per file.
    expect(document.querySelectorAll(".notice button")).toHaveLength(1);
    click(button("OK"));
    expect(notice()).toBeNull();
  });

  it("says '1 tab' for one file and tells the same way for any other reason a file cannot be opened", async () => {
    const { app, api } = make();
    api.open.mockImplementation((path: string) =>
      path === "/docs/b.pdf"
        ? Promise.reject({ code: "corrupt", message: "damaged" })
        : Promise.resolve(opened(path.charCodeAt(6), 3, path.split("/").pop())),
    );
    await app.start({ session: saved });
    await flush();
    expect(notice()?.textContent).toContain("1 tab from your last session could not be opened");
    expect(tabTitles()).toEqual(["a.pdf", "c.pdf"]);
  });

  it("never asks for a password at the start: the file stays closed and the notice says so", async () => {
    const { app, api } = make();
    api.open.mockImplementation((path: string) =>
      path === "/docs/a.pdf"
        ? Promise.reject({ code: "password_required", message: "locked" })
        : Promise.resolve(opened(path.charCodeAt(6), 3, path.split("/").pop())),
    );
    await app.start({ session: saved });
    await flush();
    expect(document.querySelector('.notice input[type="password"]')).toBeNull();
    expect(tabTitles()).toEqual(["b.pdf", "c.pdf"]);
    expect(notice()?.textContent).toContain("1 password protected file was left closed");
    expect(notice()?.textContent).not.toContain("could not be opened");
    // The same file asks for its password as before when the reader opens it themselves.
    click(button("OK"));
    const done = app.openPath("/docs/a.pdf");
    await flush();
    expect(notice()?.textContent).toContain("needs a password");
    click(button("Cancel"));
    await done;
  });

  it("keeps to the document limit: it opens at most 16 tabs, counting the files named at the start", async () => {
    const { app, api } = make();
    const many = Array.from({ length: 20 }, (_, i) => `/docs/f${i}.pdf`);
    await app.start({ session: { paths: many, active: 19 }, given: ["/docs/x.pdf", "/docs/y.pdf"] });
    await flush();
    expect(app.tabCount).toBe(16);
    expect(api.open).toHaveBeenCalledTimes(16);
    // The one that was in view is among them, and the notice says that some were left out.
    expect(api.open.mock.calls.map(([path]) => path)).toContain("/docs/f19.pdf");
    expect(notice()?.textContent).toContain("6 tabs from your last session could not be opened");
    expect(notice()?.textContent).toContain("at most 16");
  });

  it("still brings back every file after an update, also with the setting off and a list that disagrees", async () => {
    const { app, api } = make({ reopenTabs: false });
    const restored = Array.from({ length: 12 }, (_, i) => `/docs/r${i}.pdf`);
    await app.start({ restored, session: saved });
    await flush();
    expect(app.tabCount).toBe(12);
    expect(api.open.mock.calls.map(([path]) => path)).not.toContain("/docs/a.pdf");
    expect(selectedTab()).toBe("r0.pdf");
  });
});

describe("App: remembering the tabs", () => {
  const lastSaved = (api: ReturnType<typeof make>["api"]) => api.saveSession.mock.calls.at(-1)?.slice(0, 2);

  it("writes the paths in tab order and the tab in view whenever a tab is opened, closed or brought to the front", async () => {
    const { app, api } = make();
    await app.openPath("/docs/a.pdf");
    expect(lastSaved(api)).toEqual([["/docs/a.pdf"], 0]);
    await app.openPath("/docs/b.pdf");
    expect(lastSaved(api)).toEqual([["/docs/a.pdf", "/docs/b.pdf"], 1]);
    click(tab("a.pdf"));
    expect(lastSaved(api)).toEqual([["/docs/a.pdf", "/docs/b.pdf"], 0]);
    app.closeTab(Number((tab("a.pdf").closest(".tab") as HTMLElement).dataset.id));
    expect(lastSaved(api)).toEqual([["/docs/b.pdf"], 0]);
    app.closeTab(Number((tab("b.pdf").closest(".tab") as HTMLElement).dataset.id));
    expect(lastSaved(api)).toEqual([[], null]);
  });

  it("sends the same list only once and nothing but paths", async () => {
    const { app, api } = make();
    await app.openPath("/docs/a.pdf");
    const before = api.saveSession.mock.calls.length;
    click(tab("a.pdf"));
    app.currentView?.goToPage(2);
    await flush();
    expect(api.saveSession.mock.calls.length).toBe(before);
    for (const call of api.saveSession.mock.calls) {
      expect(call).toHaveLength(3);
      expect(JSON.stringify(call)).not.toContain("Hello");
    }
  });

  it("writes nothing while the tabs of the last session are still coming back, then the list as it is", async () => {
    const { app, api } = make();
    await app.start({ session: { paths: ["/docs/a.pdf", "/docs/b.pdf", "/docs/c.pdf"], active: 2 } });
    // One write at the end of the start, not one for each tab and never a list that is still short.
    expect(api.saveSession.mock.calls).toEqual([[["/docs/a.pdf", "/docs/b.pdf", "/docs/c.pdf"], 2, []]]);
  });

  it("is left alone by the update path: a restore is not a reason to forget tabs either", async () => {
    const { app, api } = make();
    await app.start({ restored: ["/docs/a.pdf", "/docs/b.pdf"] });
    expect(lastSaved(api)).toEqual([["/docs/a.pdf", "/docs/b.pdf"], 0]);
  });

  it("keeps no list while the setting is off", async () => {
    const { app, api } = make({ reopenTabs: false });
    await flush();
    await app.start({});
    await app.openPath("/docs/a.pdf");
    await app.openPath("/docs/b.pdf");
    click(tab("a.pdf"));
    app.closeTab(Number((tab("b.pdf").closest(".tab") as HTMLElement).dataset.id));
    expect(api.saveSession).not.toHaveBeenCalled();
    expect(q("#reopen-tabs").getAttribute("aria-checked")).toBe("false");
  });

  it("shows the setting, turns it off at once, and writes the list again when it is turned on", async () => {
    const { app, api } = make();
    await app.openPath("/docs/a.pdf");
    await flush();
    const toggle = q<HTMLButtonElement>("#reopen-tabs");
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    api.saveSession.mockClear();

    click(toggle);
    await flush();
    expect(api.setReopenTabs).toHaveBeenLastCalledWith(false);
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    await app.openPath("/docs/b.pdf");
    expect(api.saveSession).not.toHaveBeenCalled();

    click(toggle);
    await flush();
    expect(api.setReopenTabs).toHaveBeenLastCalledWith(true);
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(lastSaved(api)).toEqual([["/docs/a.pdf", "/docs/b.pdf"], 1]);
  });

  it("says so when the setting could not be saved", async () => {
    const { app, api } = make();
    await app.openPath("/docs/a.pdf");
    api.setReopenTabs.mockRejectedValueOnce({ code: "internal", message: "disk full" });
    click(q("#reopen-tabs"));
    await flush();
    expect(notice()?.textContent).toContain("The setting was not saved");
    // For this run it is as the reader chose.
    expect(q("#reopen-tabs").getAttribute("aria-checked")).toBe("false");
    click(button("OK"));
  });

  it("does not write the list again after the positions were forgotten, until the tabs change", async () => {
    const { app, api } = make();
    await app.openPath("/docs/a.pdf");
    await flush();
    const before = api.saveSession.mock.calls.length;
    click(q("#forget-positions"));
    await flush();
    click(button("Forget positions"));
    await flush();
    expect(api.clearPositions).toHaveBeenCalledTimes(1);
    expect(api.saveSession.mock.calls.length).toBe(before);
    await app.openPath("/docs/b.pdf");
    expect(lastSaved(api)).toEqual([["/docs/a.pdf", "/docs/b.pdf"], 1]);
  });
});

describe("App: a start that goes wrong", () => {
  const saved = { paths: ["/docs/a.pdf", "/docs/b.pdf", "/docs/c.pdf"], active: 0 };
  const name = (path: string) => path.split("/").pop();

  it("opens the file named at the start at once, also when the tab in view never answers", async () => {
    const { app, api } = make({ sessionOpenLimitMs: 150 });
    let late: (doc: OpenedDocument) => void = () => undefined;
    api.open.mockImplementation((path: string) =>
      path === "/docs/a.pdf"
        ? new Promise<OpenedDocument>((resolve) => (late = resolve))
        : Promise.resolve(opened(path.charCodeAt(6), 3, name(path))),
    );
    const starting = app.start({ session: saved, given: ["/docs/new.pdf"] });
    await flush();
    // The limit has not passed yet: the named file is open and in view, and the list is held back.
    expect(tabTitles()).toEqual(["new.pdf"]);
    expect(selectedTab()).toBe("new.pdf");
    expect(api.saveSession).not.toHaveBeenCalled();

    await starting;
    // After the limit: the others came, the list was written, and one notice counts the lost tab.
    expect(tabTitles()).toEqual(["b.pdf", "c.pdf", "new.pdf"]);
    expect(selectedTab()).toBe("new.pdf");
    expect(api.saveSession).toHaveBeenCalled();
    expect(document.querySelectorAll(".notice")).toHaveLength(1);
    expect(notice()?.textContent).toContain("1 tab from your last session could not be opened");

    // The file answers at last: its document is closed and no tab appears.
    late(opened(77, 3, "a.pdf"));
    await flush();
    expect(api.close).toHaveBeenCalledWith(77);
    expect(tabTitles()).toEqual(["b.pdf", "c.pdf", "new.pdf"]);
    expect(selectedTab()).toBe("new.pdf");
  });

  it("writes the list as soon as the limit has passed, while other tabs are still being opened", async () => {
    const { app, api } = make({ sessionOpenLimitMs: 20 });
    api.open.mockImplementation((path: string) =>
      path === "/docs/a.pdf"
        ? new Promise<OpenedDocument>(() => undefined)
        : new Promise((resolve) => setTimeout(() => resolve(opened(path.charCodeAt(6), 3, name(path))), 60)),
    );
    const starting = app.start({ session: saved });
    expect(api.saveSession).not.toHaveBeenCalled();
    await new Promise((r) => setTimeout(r, 40));
    expect(api.saveSession).toHaveBeenCalled();
    await starting;
  });

  it("does not open a zero-page file with a dialog of its own: it counts in the one notice", async () => {
    const { app, api } = make();
    api.open.mockImplementation((path: string) =>
      Promise.resolve(opened(path.charCodeAt(6), path === "/docs/a.pdf" ? 3 : 0, name(path))),
    );
    await app.start({ session: saved });
    await flush();
    expect(tabTitles()).toEqual(["a.pdf"]);
    expect(document.querySelectorAll(".notice")).toHaveLength(1);
    expect(notice()?.textContent).toContain("2 tabs from your last session could not be opened");
    expect(api.close).toHaveBeenCalledTimes(2);
  });

  it("keeps a file that could not be reached in the list for one more start, and forgets it after that", async () => {
    const down = (path: string) =>
      path === "/docs/b.pdf"
        ? Promise.reject({ code: "io", message: "no route" })
        : path === "/docs/c.pdf"
          ? Promise.reject({ code: "corrupt", message: "damaged" })
          : Promise.resolve(opened(path.charCodeAt(6), 3, name(path)));

    const first = make();
    first.api.open.mockImplementation(down);
    await first.app.start({ session: saved });
    await flush();
    // The unreachable file stays, marked; the damaged one does not.
    expect(first.api.saveSession.mock.calls.at(-1)).toEqual([["/docs/a.pdf", "/docs/b.pdf"], 0, ["/docs/b.pdf"]]);
    click(button("OK"));
    // Later changes keep it too, but not once the file is open.
    await first.app.openPath("/docs/d.pdf");
    expect(first.api.saveSession.mock.calls.at(-1)).toEqual([
      ["/docs/a.pdf", "/docs/d.pdf", "/docs/b.pdf"],
      1,
      ["/docs/b.pdf"],
    ]);

    // The next start: it fails again, and now it is forgotten.
    document.body.replaceChildren();
    installPage();
    const second = make();
    second.api.open.mockImplementation(down);
    await second.app.start({
      session: { paths: ["/docs/a.pdf", "/docs/b.pdf"], active: 0, failed: ["/docs/b.pdf"] },
    });
    await flush();
    expect(second.api.saveSession.mock.calls.at(-1)).toEqual([["/docs/a.pdf"], 0, []]);
  });
});

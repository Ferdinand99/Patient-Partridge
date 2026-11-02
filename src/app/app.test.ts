// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedDocument, SavedFile, SavedPosition } from "../lib/api";
import type { PageToolsApi, PagesChanged } from "../lib/pagesapi";
import type { SignatureItem, SignaturesInfo } from "../lib/signatures";
import { THEME_KEY } from "../lib/theme";
import { App } from "./app";
import type { PageOps } from "./pageops";

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
    /** The page tools (the page panel and the tools are not there without them). */
    pages?: PageToolsApi;
    /** What the engine says about the signatures of every document; none is asked for without it. */
    signatures?: SignaturesInfo;
    /** How long a close, a quit, a save and a print wait for a page tool before they ask. */
    changeGraceMs?: number;
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
    setDirty: vi.fn((_doc: number, _dirty: boolean) => Promise.resolve()),
    save: vi.fn((_doc: number, _overwrite: boolean, _allowSigned: boolean) =>
      Promise.resolve<SavedFile>({ path: "/docs/book.pdf", name: "book.pdf" }),
    ),
    saveAs: vi.fn((_doc: number, _allowSigned: boolean) =>
      Promise.resolve<SavedFile | null>({ path: "/docs/copy.pdf", name: "copy.pdf" }),
    ),
    quitApp: vi.fn(() => Promise.resolve()),
    stayOpen: vi.fn(() => Promise.resolve()),
    ...(options.signatures
      ? { getSignatures: vi.fn((_doc: number) => Promise.resolve(options.signatures as SignaturesInfo)) }
      : {}),
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
    signaturesDelayMs: 0,
    viewLimits: options.viewLimits,
    pages: options.pages,
    pageBusyDelayMs: 1000,
    changeGraceMs: options.changeGraceMs,
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

  it("keeps the line of a copy that is still running when the copy of another tab ends", async () => {
    clipboard(vi.fn(() => Promise.resolve()));
    const { app, api } = make({ viewLimits: { preloadChars: 0 } });
    api.open.mockImplementationOnce(() => Promise.resolve(opened(1, 30, "a.pdf")));
    api.open.mockImplementationOnce(() => Promise.resolve(opened(2, 30, "b.pdf")));
    const gates: Record<number, () => void> = {};
    const held: Record<number, Promise<void>> = {};
    for (const doc of [1, 2]) held[doc] = new Promise<void>((r) => (gates[doc] = r));
    api.getPageText.mockImplementation(async (doc: number, p: number) => {
      if (p === 25) await held[doc];
      return { text: "Hello world", rects: layoutRects("Hello world") };
    });
    await app.openPath("/a.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copyEvent();
    await flush();
    await app.openPath("/b.pdf");
    await flush();
    key("a", { ctrlKey: true });
    await flush();
    copyEvent();
    await flush();
    expect(q("#copy-status").hidden).toBe(false);
    gates[2]();
    await flush();
    await flush();
    // The copy of the other tab is still running: its line stays.
    expect(q("#copy-status").hidden).toBe(false);
    expect(q("#copy-status").textContent).toMatch(/^Copying the text: page \d+ of 30$/);
    gates[1]();
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

  it("asks for 16 tabs at most when several named files are in a full list at the end of it", async () => {
    const { app, api } = make();
    const full = Array.from({ length: 16 }, (_, i) => `/docs/f${i}.pdf`);
    await app.start({ session: { paths: full, active: 0 }, given: ["/docs/new.pdf", "/docs/f14.pdf", "/docs/f15.pdf"] });
    await flush();
    expect(app.tabCount).toBe(16);
    expect(api.open).toHaveBeenCalledTimes(16);
    // One saved tab is left out, and the notice counts it.
    expect(notice()?.textContent).toContain("1 tab from your last session could not be opened");
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

describe("App: changes that are not saved", () => {
  /** Opens a file and makes one change to it, as a feature that edits the document will. */
  async function openDirty(options: Parameters<typeof make>[0] = {}, path = "/docs/book.pdf") {
    const m = make(options);
    await m.app.openPath(path);
    await flush();
    const state = { value: 0 };
    m.app.recordEdit({
      label: "Change",
      undo: () => {
        state.value--;
      },
      redo: () => {
        state.value++;
      },
    });
    state.value++;
    return { ...m, state };
  }
  const dot = () => document.querySelector(".tab-dirty");
  const lastTitle = (deps: { setWindowTitle: ReturnType<typeof vi.fn> }) => deps.setWindowTitle.mock.calls.at(-1)?.[0];

  it("marks the tab and the window title, and tells the app process, when a document has a change", async () => {
    const { api, deps } = await openDirty();
    expect(dot()).not.toBeNull();
    expect(dot()?.getAttribute("aria-label")).toBe("Unsaved changes");
    // The name of the tab is still just the name.
    expect(tabTitles()).toEqual(["book.pdf"]);
    expect(lastTitle(deps)).toBe("• book.pdf - Patient Partridge");
    expect(api.setDirty).toHaveBeenCalledTimes(1);
    expect(api.setDirty).toHaveBeenLastCalledWith(1, true);
  });

  it("is clean again when the change is taken back, and tells the app process", async () => {
    const { app, api, deps, state } = await openDirty();
    key("z", { ctrlKey: true });
    await flush();
    expect(state.value).toBe(0);
    expect(dot()).toBeNull();
    expect(lastTitle(deps)).toBe("book.pdf - Patient Partridge");
    expect(api.setDirty).toHaveBeenLastCalledWith(1, false);
    key("y", { ctrlKey: true });
    await flush();
    expect(state.value).toBe(1);
    expect(app.unsavedCount).toBe(1);
    key("z", { ctrlKey: true });
    await flush();
    expect(state.value).toBe(0);
    key("Z", { ctrlKey: true, shiftKey: true });
    await flush();
    expect(state.value).toBe(1);
  });

  it("leaves undo and redo to a text box that has the focus", async () => {
    const { state } = await openDirty();
    const box = q<HTMLInputElement>("#page-input");
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, bubbles: true, cancelable: true }));
    await flush();
    expect(state.value).toBe(1);
  });

  it("does not mark a document that has no change", async () => {
    const m = make();
    await m.app.openPath("/docs/a.pdf");
    await flush();
    expect(dot()).toBeNull();
    expect(m.api.setDirty).not.toHaveBeenCalled();
    expect(m.app.unsavedCount).toBe(0);
  });

  describe("closing a tab", () => {
    it("asks first, and Cancel keeps the tab, its change and the engine", async () => {
      const { api } = await openDirty();
      click(q(".tab-close"));
      await flush();
      expect(notice()?.textContent).toContain('Save changes to "book.pdf"?');
      click(button("Cancel"));
      await flush();
      expect(tabTitles()).toEqual(["book.pdf"]);
      expect(dot()).not.toBeNull();
      expect(api.close).not.toHaveBeenCalled();
      expect(api.save).not.toHaveBeenCalled();
    });

    it("Escape is Cancel", async () => {
      await openDirty();
      key("w", { ctrlKey: true });
      await flush();
      expect(notice()).not.toBeNull();
      document
        .getElementById("notice-layer")
        ?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await flush();
      expect(tabTitles()).toEqual(["book.pdf"]);
    });

    it("Don't save closes it without writing anything", async () => {
      const { api } = await openDirty();
      click(q(".tab-close"));
      await flush();
      click(button("Don't save"));
      await flush();
      expect(tabTitles()).toEqual([]);
      expect(api.save).not.toHaveBeenCalled();
      expect(api.close).toHaveBeenCalledWith(1);
    });

    it("Save writes the file first and closes the tab when it worked", async () => {
      const { api } = await openDirty();
      click(q(".tab-close"));
      await flush();
      click(button("Save"));
      await flush();
      expect(api.save).toHaveBeenCalledWith(1, false, false);
      expect(api.close).toHaveBeenCalledWith(1);
      expect(tabTitles()).toEqual([]);
    });

    it("keeps the tab open, and says why, when the save does not work", async () => {
      const { api } = await openDirty();
      api.save.mockRejectedValueOnce({ code: "save_disk_full", message: "x" });
      key("w", { ctrlKey: true });
      await flush();
      click(button("Save"));
      await flush();
      expect(notice()?.textContent).toContain("The disk is full");
      click(button("OK"));
      await flush();
      expect(tabTitles()).toEqual(["book.pdf"]);
      expect(dot()).not.toBeNull();
      expect(api.close).not.toHaveBeenCalled();
    });

    it("a tab without changes closes at once, with no question", async () => {
      const m = make();
      await m.app.openPath("/docs/a.pdf");
      await flush();
      click(q(".tab-close"));
      expect(notice()).toBeNull();
      expect(tabTitles()).toEqual([]);
    });
  });

  describe("saving", () => {
    it("Ctrl+S saves the document in place, and it is clean afterwards", async () => {
      const { api, deps, app } = await openDirty();
      key("s", { ctrlKey: true });
      await flush();
      expect(api.save).toHaveBeenCalledWith(1, false, false);
      expect(dot()).toBeNull();
      expect(lastTitle(deps)).toBe("book.pdf - Patient Partridge");
      expect(app.unsavedCount).toBe(0);
      expect(api.setDirty).toHaveBeenLastCalledWith(1, false);
      // Nothing to save any more: the key does nothing.
      key("s", { ctrlKey: true });
      await flush();
      expect(api.save).toHaveBeenCalledTimes(1);
    });

    it("undoing past the saved state makes the document dirty again", async () => {
      const { app, api } = await openDirty();
      key("s", { ctrlKey: true });
      await flush();
      key("z", { ctrlKey: true });
      await flush();
      expect(app.unsavedCount).toBe(1);
      expect(api.setDirty).toHaveBeenLastCalledWith(1, true);
    });

    it("a change made while the file is being written still counts as unsaved", async () => {
      const { app, api } = await openDirty();
      let finish!: (file: SavedFile) => void;
      api.save.mockImplementationOnce(() => new Promise<SavedFile>((resolve) => (finish = resolve)));
      key("s", { ctrlKey: true });
      await flush();
      app.recordEdit({ label: "Later", undo: () => undefined, redo: () => undefined });
      finish({ path: "/docs/book.pdf", name: "book.pdf" });
      await flush();
      expect(app.unsavedCount).toBe(1);
      expect(dot()).not.toBeNull();
    });

    it("does not start a second save while one is running", async () => {
      const { api } = await openDirty();
      let finish!: (file: SavedFile) => void;
      api.save.mockImplementationOnce(() => new Promise<SavedFile>((resolve) => (finish = resolve)));
      key("s", { ctrlKey: true });
      key("s", { ctrlKey: true });
      await flush();
      expect(api.save).toHaveBeenCalledTimes(1);
      finish({ path: "/docs/book.pdf", name: "book.pdf" });
      await flush();
    });

    it("asks before it overwrites a file that something else changed, and Cancel saves nothing", async () => {
      const { api, app } = await openDirty();
      api.save.mockRejectedValueOnce({ code: "save_changed", message: "x" });
      key("s", { ctrlKey: true });
      await flush();
      expect(notice()?.textContent).toContain("The file was changed by something else");
      click(button("Cancel"));
      await flush();
      expect(api.save).toHaveBeenCalledTimes(1);
      expect(api.saveAs).not.toHaveBeenCalled();
      expect(app.unsavedCount).toBe(1);
    });

    it("Replace the file saves again with the answer", async () => {
      const { api, app } = await openDirty();
      api.save.mockRejectedValueOnce({ code: "save_changed", message: "x" });
      key("s", { ctrlKey: true });
      await flush();
      click(button("Replace the file"));
      await flush();
      expect(api.save).toHaveBeenNthCalledWith(1, 1, false, false);
      expect(api.save).toHaveBeenNthCalledWith(2, 1, true, false);
      expect(app.unsavedCount).toBe(0);
    });

    it("Save a copy… from that question saves under another name instead", async () => {
      const { api, app } = await openDirty();
      api.save.mockRejectedValueOnce({ code: "save_changed", message: "x" });
      key("s", { ctrlKey: true });
      await flush();
      click(button("Save a copy…"));
      await flush();
      expect(api.save).toHaveBeenCalledTimes(1);
      expect(api.saveAs).toHaveBeenCalledWith(1, false);
      expect(app.unsavedCount).toBe(0);
      expect(tabTitles()).toEqual(["copy.pdf"]);
    });

    it("warns that a save breaks a signature, and offers a copy", async () => {
      const { api, app } = await openDirty();
      api.save.mockRejectedValueOnce({ code: "save_signed", message: "x" });
      key("s", { ctrlKey: true });
      await flush();
      expect(notice()?.textContent).toContain("This document is signed");
      expect(notice()?.textContent).toContain("breaks the signature");
      click(button("Save a copy…"));
      await flush();
      // The copy was the user's choice, so the second question does not come.
      expect(api.saveAs).toHaveBeenCalledWith(1, true);
      expect(app.unsavedCount).toBe(0);
    });

    it("Save anyway saves in place with the answer; Cancel saves nothing", async () => {
      const m = await openDirty();
      m.api.save.mockRejectedValueOnce({ code: "save_signed", message: "x" });
      key("s", { ctrlKey: true });
      await flush();
      click(button("Cancel"));
      await flush();
      expect(m.api.save).toHaveBeenCalledTimes(1);
      expect(m.app.unsavedCount).toBe(1);

      m.api.save.mockRejectedValueOnce({ code: "save_signed", message: "x" });
      key("s", { ctrlKey: true });
      await flush();
      click(button("Save anyway"));
      await flush();
      expect(m.api.save).toHaveBeenLastCalledWith(1, false, true);
      expect(m.app.unsavedCount).toBe(0);
    });

    it("tells the user what went wrong and keeps the change, for every other failure", async () => {
      const { api, app } = await openDirty();
      for (const [code, text] of [
        ["save_verify_failed", "did not pass the check"],
        ["save_protection_lost", "password protection would be lost"],
        ["save_write_failed", "could not be saved"],
      ]) {
        api.save.mockRejectedValueOnce({ code, message: "detail" });
        key("s", { ctrlKey: true });
        await flush();
        expect(notice()?.textContent).toContain(text);
        click(button("OK"));
        await flush();
        expect(app.unsavedCount).toBe(1);
      }
    });

    it("Save As (Ctrl+Shift+S) saves a copy and the tab follows the copy", async () => {
      const { api, app, deps } = await openDirty();
      key("S", { ctrlKey: true, shiftKey: true });
      await flush();
      expect(api.saveAs).toHaveBeenCalledWith(1, false);
      expect(tabTitles()).toEqual(["copy.pdf"]);
      expect(lastTitle(deps)).toBe("copy.pdf - Patient Partridge");
      expect(tab("copy.pdf").title).toBe("/docs/copy.pdf");
      expect(app.unsavedCount).toBe(0);
      // The list of tabs for the next start names the copy.
      expect(api.saveSession.mock.calls.at(-1)?.[0]).toEqual(["/docs/copy.pdf"]);
    });

    it("a cancelled Save As changes nothing", async () => {
      const { api, app } = await openDirty();
      api.saveAs.mockResolvedValueOnce(null);
      key("S", { ctrlKey: true, shiftKey: true });
      await flush();
      expect(tabTitles()).toEqual(["book.pdf"]);
      expect(app.unsavedCount).toBe(1);
    });
  });

  describe("quitting", () => {
    it("quits at once when nothing is unsaved", async () => {
      const m = make();
      await m.app.openPath("/docs/a.pdf");
      await flush();
      await m.app.requestQuit();
      expect(m.api.quitApp).toHaveBeenCalledTimes(1);
      expect(notice()).toBeNull();
    });

    it("asks first when a document has changes: Cancel stays, and tells the app process", async () => {
      const { app, api } = await openDirty();
      const asking = app.requestQuit();
      await flush();
      expect(notice()?.textContent).toContain('Save changes to "book.pdf"?');
      click(button("Cancel"));
      await asking;
      expect(api.quitApp).not.toHaveBeenCalled();
      expect(api.stayOpen).toHaveBeenCalledTimes(1);
      expect(app.unsavedCount).toBe(1);
    });

    it("Quit without saving quits and writes nothing", async () => {
      const { app, api } = await openDirty();
      const asking = app.requestQuit();
      await flush();
      click(button("Quit without saving"));
      await asking;
      expect(api.quitApp).toHaveBeenCalledTimes(1);
      expect(api.save).not.toHaveBeenCalled();
    });

    it("Save and quit saves every document with changes, then quits", async () => {
      const { app, api } = await openDirty();
      await app.openPath("/docs/other.pdf");
      await flush();
      app.recordEdit({ label: "Other", undo: () => undefined, redo: () => undefined });
      await app.openPath("/docs/clean.pdf");
      await flush();
      const asking = app.requestQuit();
      await flush();
      expect(notice()?.textContent).toContain("Save changes to 2 documents?");
      expect(notice()?.textContent).toContain('"book.pdf"');
      expect(notice()?.textContent).toContain('"other.pdf"');
      expect(notice()?.textContent).not.toContain('"clean.pdf"');
      click(button("Save and quit"));
      await asking;
      expect(api.save.mock.calls.map(([doc]) => doc)).toEqual([1, 2]);
      expect(api.quitApp).toHaveBeenCalledTimes(1);
    });

    it("does not quit when a save fails or is cancelled: the changes stay", async () => {
      const { app, api } = await openDirty();
      api.save.mockRejectedValueOnce({ code: "save_disk_full", message: "x" });
      const asking = app.requestQuit();
      await flush();
      click(button("Save and quit"));
      await flush();
      // The error of the save comes first, then the app stays open.
      click(button("OK"));
      await asking;
      expect(api.quitApp).not.toHaveBeenCalled();
      expect(api.stayOpen).toHaveBeenCalledTimes(1);
      expect(app.unsavedCount).toBe(1);
    });

    it("asks only once when the app asks twice", async () => {
      const { app } = await openDirty();
      const first = app.requestQuit();
      void app.requestQuit();
      await flush();
      expect(document.querySelectorAll(".notice")).toHaveLength(1);
      click(button("Cancel"));
      await first;
    });
  });

  it("tells the user that changes are lost when the engine of a changed document stops", async () => {
    const { app } = await openDirty();
    // The view reports the lost engine the way a failed render does.
    const internals = app as unknown as { tabs: unknown[]; documentLost(tab: unknown, error: unknown): Promise<void> };
    void internals.documentLost(internals.tabs[0], { code: "engine_stopped", message: "it exited" });
    await flush();
    expect(notice()?.textContent).toContain("The changes that were not saved are lost.");
    expect(tabTitles()).toEqual([]);
  });
});

describe("App: a save that an edit overtakes", () => {
  it("is reported as not saved, so closing the tab does not drop the edit", async () => {
    const m = make();
    await m.app.openPath("/docs/book.pdf");
    await flush();
    const edit = { label: "Change", undo: () => undefined, redo: () => undefined };
    m.app.recordEdit(edit);
    let finish!: (file: SavedFile) => void;
    m.api.save.mockImplementationOnce(() => new Promise<SavedFile>((resolve) => (finish = resolve)));
    click(q(".tab-close"));
    await flush();
    click(button("Save"));
    await flush();
    // An edit is made while the file is written.
    m.app.recordEdit({ ...edit, label: "Later" });
    finish({ path: "/docs/book.pdf", name: "book.pdf" });
    await flush();
    expect(tabTitles()).toEqual(["book.pdf"]);
    expect(m.api.close).not.toHaveBeenCalled();
    expect(m.app.unsavedCount).toBe(1);
    // The app process was told the document is dirty and never that it is clean.
    expect(m.api.setDirty).toHaveBeenLastCalledWith(1, true);
    expect(m.api.setDirty).not.toHaveBeenCalledWith(1, false);
  });

  it("Save and quit does not quit over an edit made during the save", async () => {
    const m = make();
    await m.app.openPath("/docs/book.pdf");
    await flush();
    m.app.recordEdit({ label: "Change", undo: () => undefined, redo: () => undefined });
    let finish!: (file: SavedFile) => void;
    m.api.save.mockImplementationOnce(() => new Promise<SavedFile>((resolve) => (finish = resolve)));
    const asking = m.app.requestQuit();
    await flush();
    click(button("Save and quit"));
    await flush();
    m.app.recordEdit({ label: "Later", undo: () => undefined, redo: () => undefined });
    finish({ path: "/docs/book.pdf", name: "book.pdf" });
    await asking;
    expect(m.api.quitApp).not.toHaveBeenCalled();
    expect(m.api.stayOpen).toHaveBeenCalledTimes(1);
  });

  it("waits for a save that is running instead of giving up with no message", async () => {
    const m = make();
    await m.app.openPath("/docs/book.pdf");
    await flush();
    m.app.recordEdit({ label: "Change", undo: () => undefined, redo: () => undefined });
    let finish!: (file: SavedFile) => void;
    m.api.save.mockImplementationOnce(() => new Promise<SavedFile>((resolve) => (finish = resolve)));
    key("s", { ctrlKey: true });
    await flush();
    const asking = m.app.requestQuit();
    await flush();
    click(button("Save and quit"));
    await flush();
    finish({ path: "/docs/book.pdf", name: "book.pdf" });
    await asking;
    expect(m.api.save).toHaveBeenCalledTimes(1);
    expect(m.api.quitApp).toHaveBeenCalledTimes(1);
  });
});

describe("App: a save that does not end", () => {
  /** A document with a change, and a save that stays stuck until the test lets it go. */
  async function stuck() {
    const m = make();
    await m.app.openPath("/docs/book.pdf");
    await flush();
    m.app.recordEdit({ label: "Change", undo: () => undefined, redo: () => undefined });
    let finish!: (file: SavedFile) => void;
    m.api.save.mockImplementationOnce(() => new Promise<SavedFile>((resolve) => (finish = resolve)));
    return { ...m, finish: (file: SavedFile) => finish(file) };
  }
  const slow = () => new Promise((r) => setTimeout(r, 600));
  const sheet = () => document.querySelector<HTMLElement>(".notice .sheet-busy");

  it("shows that the save goes on, with a way out, when Save and quit takes long", async () => {
    const m = await stuck();
    const asking = m.app.requestQuit();
    await flush();
    click(button("Save and quit"));
    await flush();
    // A quick save shows nothing.
    expect(notice()).toBeNull();
    await slow();
    expect(notice()?.textContent).toContain('Saving "book.pdf"');
    expect(sheet()).not.toBeNull();
    expect(button("Cancel")).not.toBeNull();
    click(button("Quit without saving"));
    await asking;
    expect(m.api.quitApp).toHaveBeenCalledTimes(1);
    expect(notice()).toBeNull();
  });

  it("shows no box about the save beside a question that came up during it", async () => {
    const m = make();
    await m.app.openPath("/docs/book.pdf");
    await flush();
    m.app.recordEdit({ label: "Change", undo: () => undefined, redo: () => undefined });
    let fail!: (error: unknown) => void;
    m.api.save.mockImplementationOnce(() => new Promise<SavedFile>((_, reject) => (fail = reject)));
    const asking = m.app.requestQuit();
    await flush();
    click(button("Save and quit"));
    await flush();
    // The save ends with a question before the box would come up.
    fail({ code: "save_locked", message: "x" });
    await flush();
    expect(button("Save a copy…")).not.toBeNull();
    await slow();
    // The question is still open: it is the only box, and it has the focus.
    expect(document.querySelectorAll(".notice")).toHaveLength(1);
    expect(sheet()).toBeNull();
    expect(document.activeElement?.textContent).toBe("Cancel");
    click(button("Cancel"));
    await flush();
    await asking;
    expect(m.api.quitApp).not.toHaveBeenCalled();
  });

  it("answers a second request to close or quit at once, while the first still waits", async () => {
    const m = await stuck();
    const asking = m.app.requestQuit();
    await flush();
    click(button("Save and quit"));
    await flush();
    expect(notice()).toBeNull();
    // The close button or Cmd+Q again, long before the box would come up by itself.
    void m.app.requestQuit();
    await flush();
    expect(notice()?.textContent).toContain('Saving "book.pdf"');
    expect(document.querySelectorAll(".notice")).toHaveLength(1);
    expect(document.activeElement?.textContent).toBe("Cancel");
    click(button("Quit without saving"));
    await asking;
    expect(m.api.quitApp).toHaveBeenCalledTimes(1);
  });

  it("Cancel keeps the program open, and the save that was left running does not quit it", async () => {
    const m = await stuck();
    const asking = m.app.requestQuit();
    await flush();
    click(button("Save and quit"));
    await flush();
    void m.app.requestQuit();
    await flush();
    click(button("Cancel"));
    await asking;
    expect(m.api.quitApp).not.toHaveBeenCalled();
    expect(m.api.stayOpen).toHaveBeenCalledTimes(1);
    // The save ends later: nothing quits.
    m.finish({ path: "/docs/book.pdf", name: "book.pdf" });
    await flush();
    expect(m.api.quitApp).not.toHaveBeenCalled();
    expect(m.app.unsavedCount).toBe(0);
    // And a new request works as a new request.
    const again = m.app.requestQuit();
    await flush();
    await again;
    expect(m.api.quitApp).toHaveBeenCalledTimes(1);
  });

  it("takes the box away by itself when the save ends", async () => {
    const m = await stuck();
    const asking = m.app.requestQuit();
    await flush();
    click(button("Save and quit"));
    await flush();
    void m.app.requestQuit();
    await flush();
    expect(sheet()).not.toBeNull();
    m.finish({ path: "/docs/book.pdf", name: "book.pdf" });
    await asking;
    expect(notice()).toBeNull();
    expect(m.api.quitApp).toHaveBeenCalledTimes(1);
  });

  it("does the same for Save when a tab is closed: the tab can be closed without waiting", async () => {
    const m = await stuck();
    click(q(".tab-close"));
    await flush();
    click(button("Save"));
    await slow();
    expect(notice()?.textContent).toContain('Saving "book.pdf"');
    click(button("Don't save"));
    await flush();
    expect(tabTitles()).toEqual([]);
    expect(m.api.close).toHaveBeenCalledTimes(1);
  });
});

describe("App: a save that cannot replace the file", () => {
  async function openDirty() {
    const m = make();
    await m.app.openPath("/docs/book.pdf");
    await flush();
    m.app.recordEdit({ label: "Change", undo: () => undefined, redo: () => undefined });
    return m;
  }

  it("offers a copy when the file is held open, shared under several names, or someone else's", async () => {
    for (const code of ["save_locked", "save_hard_link", "save_owner_changes", "save_access_kept", "save_read_only"]) {
      document.body.replaceChildren();
      installPage();
      const m = await openDirty();
      m.api.save.mockRejectedValueOnce({ code, message: "x" });
      key("s", { ctrlKey: true });
      await flush();
      expect(button("Save a copy…"), code).not.toBeNull();
      click(button("Save a copy…"));
      await flush();
      expect(m.api.saveAs, code).toHaveBeenCalledWith(1, false);
      expect(m.app.unsavedCount, code).toBe(0);
    }
  });

  it("does not say the file is read-only when it is not", async () => {
    const m = await openDirty();
    m.api.save.mockRejectedValueOnce({ code: "save_locked", message: "x" });
    key("s", { ctrlKey: true });
    await flush();
    expect(notice()?.textContent).not.toContain("read-only");
    expect(notice()?.textContent).toContain("USB stick or a network drive");
  });

  it("offers a copy when the file was touched, and the copy says so in one line", async () => {
    const m = await openDirty();
    m.api.save.mockRejectedValueOnce({ code: "save_source_touched", message: "x" });
    m.api.saveAs.mockResolvedValueOnce({ path: "/docs/copy.pdf", name: "copy.pdf", touched: true });
    key("s", { ctrlKey: true });
    await flush();
    expect(button("Save a copy…")).not.toBeNull();
    click(button("Save a copy…"));
    await flush();
    expect(m.api.saveAs).toHaveBeenCalledWith(1, false);
    expect(m.app.unsavedCount).toBe(0);
    expect(notice()?.textContent).toContain("touched");
    expect(notice()?.textContent).toContain("copy.pdf");
    click(button("OK"));
    await flush();
    expect(notice()).toBeNull();
  });

  it("tells the user to open the file again when it changed in place, and offers no copy", async () => {
    const m = await openDirty();
    m.api.save.mockRejectedValueOnce({ code: "save_source_changed", message: "x" });
    key("s", { ctrlKey: true });
    await flush();
    expect(notice()?.textContent).toContain("Close the tab and open the file again");
    expect(button("Save a copy…")).toBeNull();
    click(button("OK"));
    await flush();
    expect(m.api.saveAs).not.toHaveBeenCalled();
    expect(m.app.unsavedCount).toBe(1);
  });
});

describe("App: page tools", () => {
  const changed = (pages: number, snapshot = 0, count = 1): PagesChanged => ({
    pages: Array.from({ length: pages }, () => letter),
    snapshot,
    count,
    formFields: 0,
  });

  function fakePages() {
    return {
      pagesDelete: vi.fn((_d: number, pages: number[]) => Promise.resolve(changed(3 - pages.length, 7, pages.length))),
      pagesReorder: vi.fn(() => Promise.resolve(changed(3, 8))),
      pagesRotate: vi.fn((_d: number, pages: number[], _t: number) => Promise.resolve(changed(3, 0, pages.length))),
      pagesCrop: vi.fn(() => Promise.resolve({ pages: [letter, letter, letter], before: [], after: [] })),
      pagesSetBoxes: vi.fn(() => Promise.resolve(changed(3))),
      pagesInsertBlank: vi.fn(() => Promise.resolve(changed(4, 9))),
      pagesRestore: vi.fn((_d: number, snapshot: number) => Promise.resolve(changed(3, snapshot + 100, 0))),
      pagesInsertPdf: vi.fn(() => Promise.resolve({ changed: changed(5, 11, 2), inserted: 2 })),
      pagesExtract: vi.fn(() => Promise.resolve(null)),
      pagesCancel: vi.fn(() => Promise.resolve("idle" as const)),
    };
  }

  const opsOf = (app: App) => (app as unknown as { pageOps: PageOps }).pageOps;

  /** A document that has been turned once, so that it has unsaved changes. */
  async function changedDocument() {
    const pages = fakePages();
    const made = make({ pages: pages as unknown as PageToolsApi });
    await made.app.openPath("/a.pdf");
    await flush();
    await opsOf(made.app).rotate([0], 1);
    return { ...made, pages, ops: opsOf(made.app) };
  }

  describe("a tool that is working on a document that looks clean", () => {
    const firstTab = (app: App) => (app as unknown as { tabs: { id: number }[] }).tabs[0].id;

    /** A document with nothing unsaved and a turn that has not answered yet. */
    async function turningCleanDocument() {
      const pages = fakePages();
      const made = make({ pages: pages as unknown as PageToolsApi });
      await made.app.openPath("/a.pdf");
      await flush();
      let finish!: (c: PagesChanged) => void;
      pages.pagesRotate.mockImplementationOnce(() => new Promise((r) => (finish = r)));
      const ops = opsOf(made.app);
      const turning = ops.rotate([0], 1);
      await flush();
      return { ...made, pages, ops, turning, finish: (c: PagesChanged = changed(3, 0, 1)) => finish(c) };
    }

    it("closing its tab waits for it, and then asks about the change it made", async () => {
      const { app, turning, finish } = await turningCleanDocument();
      const closing = app.requestClose(firstTab(app));
      await flush();
      expect(tabTitles()).toEqual(["a.pdf"]);
      expect(notice()).toBeNull();
      finish();
      await turning;
      await flush();
      // The tab is dirty now: the question is asked, and the tab is still there.
      expect(notice()?.textContent).toContain("Save changes");
      expect(tabTitles()).toEqual(["a.pdf"]);
      click(button("Cancel"));
      expect(await closing).toBe(false);
    });

    it("quitting waits for it, and asks about the change it made", async () => {
      const { app, api, turning, finish } = await turningCleanDocument();
      const quitting = app.requestQuit();
      await flush();
      expect(api.quitApp).not.toHaveBeenCalled();
      expect(notice()).toBeNull();
      finish();
      await turning;
      await flush();
      expect(notice()?.textContent).toContain("Save changes");
      expect(api.quitApp).not.toHaveBeenCalled();
      click(button("Cancel"));
      await quitting;
    });

    it("printing waits for it, so that the pages printed are those it leaves", async () => {
      const { app, turning, finish } = await turningCleanDocument();
      const printing = app.printActive();
      await flush();
      expect(document.querySelector(".print-dialog")).toBeNull();
      finish();
      await turning;
      await flush();
      expect(document.querySelector(".print-dialog")).not.toBeNull();
      document.querySelector<HTMLElement>(".print-dialog .btn-secondary")?.click();
      await printing;
    });
  });


  describe("a change that takes long, on one of two documents", () => {
    const tabsOf = (app: App) => (app as unknown as { tabs: { id: number; doc: number }[] }).tabs;
    const lost = { code: "engine_stopped", message: "the engine stopped" };

    /** Two documents. A turn of the first has not answered yet. The grace before a question is short. */
    async function turningFirst(graceMs = 20, firstDirty = false) {
      const pages = fakePages();
      const made = make({ pages: pages as unknown as PageToolsApi, changeGraceMs: graceMs });
      await made.app.openPath("/a.pdf");
      await flush();
      await made.app.openPath("/b.pdf");
      await flush();
      click(tab("a.pdf"));
      await flush();
      if (firstDirty) made.app.recordEdit(edit);
      let finish!: (c: PagesChanged) => void;
      let fail!: (e: unknown) => void;
      pages.pagesRotate.mockImplementationOnce(
        () =>
          new Promise<PagesChanged>((resolve, reject) => {
            finish = resolve;
            fail = reject;
          }),
      );
      const ops = opsOf(made.app);
      const turning = ops.rotate([0], 1);
      await flush();
      const [a, b] = tabsOf(made.app);
      return {
        ...made,
        pages,
        ops,
        turning,
        a,
        b,
        finish: (c: PagesChanged = changed(3, 0, 1)) => finish(c),
        fail: (e: unknown = lost) => fail(e),
      };
    }

    const later = (ms: number) => new Promise((r) => setTimeout(r, ms));
    const edit = { label: "edit", undo: () => Promise.resolve(), redo: () => Promise.resolve() };

    it("a save of the other document does not wait for it", async () => {
      const { app, api, finish, turning, b } = await turningFirst(60_000);
      click(tab("b.pdf"));
      await flush();
      app.recordEdit(edit);
      const saved = app.saveActive();
      await flush();
      expect(api.save).toHaveBeenCalledOnce();
      expect(api.save.mock.calls[0][0]).toBe(b.doc);
      expect(await saved).toBe(true);
      finish();
      await turning;
    });

    it("closing the other document does not wait for it, and asks nothing", async () => {
      const { app, finish, turning, b } = await turningFirst(60_000);
      expect(await app.requestClose(b.id)).toBe(true);
      expect(tabTitles()).toEqual(["a.pdf"]);
      expect(notice()).toBeNull();
      finish();
      await turning;
    });

    it("printing the other document does not wait for it", async () => {
      const { app, finish, turning } = await turningFirst(60_000);
      click(tab("b.pdf"));
      await flush();
      const printing = app.printActive();
      await flush();
      expect(document.querySelector(".print-dialog")).not.toBeNull();
      document.querySelector<HTMLElement>(".print-dialog .btn-secondary")?.click();
      await printing;
      finish();
      await turning;
    });

    it("a save of the document that is being changed waits a moment, and then says why it did not save", async () => {
      const pages = fakePages();
      const { app, api } = make({ pages: pages as unknown as PageToolsApi, changeGraceMs: 20 });
      await app.openPath("/a.pdf");
      await flush();
      const ops = opsOf(app);
      await ops.rotate([0], 1);
      let finish!: (c: PagesChanged) => void;
      pages.pagesRotate.mockImplementationOnce(() => new Promise<PagesChanged>((r) => (finish = r)));
      const second = ops.rotate([1], 1);
      await flush();
      const saved = app.saveActive();
      await later(80);
      await flush();
      expect(notice()?.textContent).toContain("is being changed");
      expect(api.save).not.toHaveBeenCalled();
      click(button("OK"));
      expect(await saved).toBe(false);
      finish(changed(3, 0, 1));
      await second;
    });

    it("closing it asks, after a moment, whether to wait or to close now and lose the change", async () => {
      const { app, a, finish, turning } = await turningFirst();
      const closing = app.requestClose(a.id);
      await later(80);
      await flush();
      expect(notice()?.textContent).toContain('"a.pdf" is still being changed');
      expect(notice()?.textContent).toContain("that change and the changes to this document that you have not saved are lost");
      expect(tabTitles()).toEqual(["a.pdf", "b.pdf"]);
      // Wait: nothing is closed, nothing is lost.
      click(button("Wait"));
      expect(await closing).toBe(false);
      expect(tabTitles()).toEqual(["a.pdf", "b.pdf"]);
      finish();
      await turning;
    });

    it("closing it now ends the document at once, and the change that was under way says nothing", async () => {
      const { app, api, a, fail, turning } = await turningFirst();
      const closing = app.requestClose(a.id);
      await later(80);
      await flush();
      click(button("Close now"));
      expect(await closing).toBe(true);
      expect(tabTitles()).toEqual(["b.pdf"]);
      expect(api.close).toHaveBeenCalledWith(a.doc);
      // The engine of the document is ended with it: the turn fails, and that is not said again.
      fail();
      await turning;
      await flush();
      expect(notice()).toBeNull();
    });

    it("closing it again while the first request waits brings the question up at once", async () => {
      const { app, a, finish, turning } = await turningFirst(60_000);
      const first = app.requestClose(a.id);
      await flush();
      expect(notice()).toBeNull();
      expect(await app.requestClose(a.id)).toBe(false);
      await flush();
      expect(notice()?.textContent).toContain("is still being changed");
      click(button("Wait"));
      expect(await first).toBe(false);
      finish();
      await turning;
    });

    it("a change that ends while the question is open leaves the ordinary questions to follow", async () => {
      const { app, a, finish, turning } = await turningFirst();
      const closing = app.requestClose(a.id);
      await later(80);
      await flush();
      finish();
      await turning;
      await flush();
      click(button("Close now"));
      await flush();
      // The change was made: the document has changes that are not saved, and is not closed unasked.
      expect(notice()?.textContent).toContain("Save changes");
      click(button("Cancel"));
      expect(await closing).toBe(false);
      expect(tabTitles()).toEqual(["a.pdf", "b.pdf"]);
    });

    it("quitting asks, after a moment, whether to wait or to quit now", async () => {
      const { app, api, finish, turning } = await turningFirst();
      const quitting = app.requestQuit();
      await later(80);
      await flush();
      expect(notice()?.textContent).toContain('"a.pdf" is still being changed');
      expect(notice()?.textContent).toContain("If you quit now");
      click(button("Wait"));
      await quitting;
      expect(api.quitApp).not.toHaveBeenCalled();
      expect(api.stayOpen).toHaveBeenCalledOnce();
      finish();
      await turning;
    });

    it("quitting now quits at once when nothing else is unsaved", async () => {
      const { app, api, finish, turning } = await turningFirst();
      const quitting = app.requestQuit();
      await later(80);
      await flush();
      click(button("Quit now"));
      await quitting;
      expect(api.quitApp).toHaveBeenCalledOnce();
      expect(notice()).toBeNull();
      finish();
      await turning;
    });

    it("quitting now does not ask about the document whose change was given up, though it has other changes unsaved", async () => {
      const { app, api, finish, turning } = await turningFirst(20, true);
      const quitting = app.requestQuit();
      await later(80);
      await flush();
      click(button("Quit now"));
      await quitting;
      expect(notice()).toBeNull();
      expect(api.save).not.toHaveBeenCalled();
      expect(api.quitApp).toHaveBeenCalledOnce();
      finish();
      await turning;
    });

    it("quitting now still asks about the other documents that have changes that are not saved", async () => {
      const { app, api, finish, turning } = await turningFirst();
      click(tab("b.pdf"));
      await flush();
      app.recordEdit(edit);
      const quitting = app.requestQuit();
      await later(80);
      await flush();
      click(button("Quit now"));
      await flush();
      // Only b.pdf is asked about: the change to a.pdf was given up with the choice.
      expect(notice()?.textContent).toContain('Save changes to "b.pdf"?');
      click(button("Save and quit"));
      await quitting;
      expect(api.save).toHaveBeenCalledOnce();
      expect(api.save.mock.calls[0][0]).toBe(tabsOf(app)[1].doc);
      expect(api.quitApp).toHaveBeenCalledOnce();
      finish();
      await turning;
    });

    it("a second request to quit brings the question up at once", async () => {
      const { app, api, finish, turning } = await turningFirst(60_000);
      const quitting = app.requestQuit();
      await flush();
      expect(notice()).toBeNull();
      await app.requestQuit();
      await flush();
      expect(notice()?.textContent).toContain("is still being changed");
      click(button("Wait"));
      await quitting;
      expect(api.quitApp).not.toHaveBeenCalled();
      finish();
      await turning;
    });

    it("printing it waits a moment, and then says that the print would show the old pages", async () => {
      const { app, finish, turning } = await turningFirst();
      const printing = app.printActive();
      await later(80);
      await flush();
      expect(notice()?.textContent).toContain("is being changed");
      expect(document.querySelector(".print-dialog")).toBeNull();
      click(button("OK"));
      await printing;
      expect(document.querySelector(".print-dialog")).toBeNull();
      finish();
      await turning;
    });
  });
  describe("on a signed document", () => {
    const item: SignatureItem = {
      index: 0,
      state: "unknown_signer",
      reason: "no_trust_list",
      certificateTime: "valid",
      subFilter: "adbe.pkcs7.detached",
      byteRange: [0, 100, 300, 50],
      fileLen: 350,
      docMdp: 0,
      certificates: 1,
      signer: "Test Signer",
      organisation: null,
      issuer: null,
      serial: null,
      fingerprint: null,
      validFrom: null,
      validTo: null,
      key: null,
      digest: null,
      claimedTime: null,
      statedReason: null,
      statedLocation: null,
      trustAnchor: null,
    };
    const signed: SignaturesInfo = { total: 1, unlisted: 0, trustAnchors: 0, items: [item] };
    const badge = () => q<HTMLButtonElement>("#signature-badge");
    const INTACT = "Signature intact; signer not verified";
    const STALE = "Not checked for this version";
    const tools: [string, (ops: PageOps) => Promise<unknown>][] = [
      ["a turn", (ops) => ops.rotate([0], 1)],
      ["a crop", (ops) => ops.crop([0], [10, 10, 10, 10])],
      ["a delete", (ops) => ops.remove([2])],
      ["a move", (ops) => ops.reorder([1, 0, 2], [0])],
      ["a blank page", (ops) => ops.insertBlank(1, 612, 792)],
      ["pages added from a file", (ops) => ops.insertFiles(1, ["/b.pdf"])],
    ];

    it.each(tools)("%s drops the verdict at once and it does not come back", async (_name, run) => {
      const pages = fakePages();
      const m = make({ pages: pages as unknown as PageToolsApi, signatures: signed });
      await m.app.openPath("/docs/signed.pdf");
      await flush();
      expect(badge().textContent).toBe(INTACT);
      await run(opsOf(m.app));
      await flush();
      expect(badge().textContent).toBe(STALE);
      // Taking the change back leaves the document unchanged in memory, but the verdict stays gone.
      key("z", { ctrlKey: true });
      await flush();
      expect(badge().textContent).toBe(STALE);
      expect(badge().textContent).not.toContain("intact");
    });

    it("a change that fails leaves the verdict as it was", async () => {
      const pages = fakePages();
      pages.pagesRotate.mockRejectedValueOnce({ code: "page_change_failed", message: "refused" });
      const m = make({ pages: pages as unknown as PageToolsApi, signatures: signed });
      await m.app.openPath("/docs/signed.pdf");
      await flush();
      const turning = opsOf(m.app).rotate([0], 1);
      await flush();
      click(button("OK"));
      await turning;
      await flush();
      expect(badge().textContent).toBe(INTACT);
    });
  });

  it("a save waits for a tool that is working, so that it writes the pages as they are then", async () => {
    const { app, api, pages, ops } = await changedDocument();
    let finish!: (c: PagesChanged) => void;
    pages.pagesRotate.mockImplementationOnce(() => new Promise((r) => (finish = r)));
    const second = ops.rotate([1], 1);
    await flush();
    const saved = app.saveActive();
    await flush();
    expect(api.save).not.toHaveBeenCalled();
    finish(changed(3, 0, 1));
    await second;
    await saved;
    expect(api.save).toHaveBeenCalledOnce();
  });

  it("taking a change back does not start while a tool works, and says nothing about it", async () => {
    const { pages, ops } = await changedDocument();
    let finish!: (c: PagesChanged) => void;
    pages.pagesRotate.mockImplementationOnce(() => new Promise((r) => (finish = r)));
    const second = ops.rotate([1], 1);
    await flush();
    const calls = pages.pagesRotate.mock.calls.length;
    key("z", { ctrlKey: true });
    await flush();
    // Nothing was asked of the engine, and no box says that something failed.
    expect(pages.pagesRotate.mock.calls.length).toBe(calls);
    expect(notice()).toBeNull();
    finish(changed(3, 0, 1));
    await second;
  });

  it("a second tool that is asked for while one works is not started and shows nothing", async () => {
    const { pages, ops } = await changedDocument();
    let finish!: (c: PagesChanged) => void;
    pages.pagesRotate.mockImplementationOnce(() => new Promise((r) => (finish = r)));
    const first = ops.rotate([1], 1);
    await flush();
    // Deleting every page would be refused with a box, if it were looked at at all.
    expect(await ops.remove([0, 1, 2])).toBeNull();
    expect(notice()).toBeNull();
    expect(pages.pagesDelete).not.toHaveBeenCalled();
    finish(changed(3, 0, 1));
    await first;
  });

  it("the result of a search is dropped when the pages change, because page numbers mean other pages", async () => {
    const { app, ops } = await changedDocument();
    key("f", { ctrlKey: true });
    const input = q<HTMLInputElement>("#find-input");
    input.value = "hello";
    input.dispatchEvent(new Event("input"));
    await new Promise((r) => setTimeout(r, 120));
    await flush();
    expect(q("#find-status").textContent).toBe("1 of 1");
    await ops.rotate([2], 1);
    await flush();
    expect(q("#find-status").textContent).not.toBe("1 of 1");
    expect(document.querySelectorAll(".hl-find")).toHaveLength(0);
    expect(app.currentView).not.toBeNull();
  });

  it("a search that was kept for a document behind another is dropped when its pages change", async () => {
    const pages = fakePages();
    const { app } = make({ pages: pages as unknown as PageToolsApi });
    await app.openPath("/a.pdf");
    await flush();
    key("f", { ctrlKey: true });
    const input = q<HTMLInputElement>("#find-input");
    input.value = "hello";
    input.dispatchEvent(new Event("input"));
    await new Promise((r) => setTimeout(r, 120));
    await flush();
    expect(q("#find-status").textContent).toBe("1 of 1");
    // Another document comes to the front; the pages of the first change while it is behind.
    await app.openPath("/b.pdf");
    await flush();
    (app as unknown as { applyPages(doc: number, pages: unknown[]): void }).applyPages(1, [letter, letter]);
    click(tab("a.pdf"));
    await flush();
    expect(q("#find-status").textContent).not.toBe("1 of 1");
  });

  it("the contents of the side panel are read again after the pages change", async () => {
    const { api, ops } = await changedDocument();
    click(q("#sidebar-toggle"));
    click(q("#side-tab-contents"));
    await flush();
    const before = api.getOutline.mock.calls.length;
    expect(before).toBeGreaterThan(0);
    await ops.rotate([1], 1);
    await flush();
    // The bookmarks point at page numbers, which now mean other pages: they are asked for again.
    expect(api.getOutline.mock.calls.length).toBeGreaterThan(before);
  });

  it("an undo that fails because the engine let go of the copy says so, and does not claim that nothing was lost", async () => {
    const { pages, ops } = await changedDocument();
    await ops.remove([1]);
    // The first delete of a document says what a delete leaves; that is answered first.
    click(button("OK"));
    await flush();
    pages.pagesRestore.mockRejectedValueOnce({ code: "snapshot_gone", message: "" });
    key("z", { ctrlKey: true });
    await flush();
    expect(notice()?.textContent).toMatch(/cannot be taken back any more/);
    expect(notice()?.textContent).not.toMatch(/Nothing was lost/);
  });

  it("an undo whose engine died says no second thing: the tab is closed with its own message", async () => {
    const { pages, ops } = await changedDocument();
    await ops.remove([1]);
    click(button("OK"));
    await flush();
    pages.pagesRestore.mockRejectedValueOnce({ code: "engine_stopped", message: "gone" });
    key("z", { ctrlKey: true });
    await flush();
    const text = [...document.querySelectorAll(".notice")].map((n) => n.textContent).join(" | ");
    expect(text).not.toMatch(/Nothing was lost/);
    expect(text).not.toMatch(/could not be taken back/);
  });
});

describe("App: the verdict on a signature belongs to the bytes it was made from", () => {
  const intact: SignatureItem = {
    index: 0,
    state: "unknown_signer",
    reason: "no_trust_list",
    certificateTime: "valid",
    subFilter: "adbe.pkcs7.detached",
    byteRange: [0, 100, 300, 50],
    fileLen: 350,
    docMdp: 0,
    certificates: 1,
    signer: "Test Signer",
    organisation: null,
    issuer: null,
    serial: null,
    fingerprint: null,
    validFrom: null,
    validTo: null,
    key: null,
    digest: null,
    claimedTime: null,
    statedReason: null,
    statedLocation: null,
    trustAnchor: null,
  };
  const signed: SignaturesInfo = { total: 1, unlisted: 0, trustAnchors: 0, items: [intact] };
  const badge = () => q<HTMLButtonElement>("#signature-badge");
  const INTACT = "Signature intact; signer not verified";
  const STALE = "Not checked for this version";

  async function openSigned(path = "/docs/signed.pdf") {
    const m = make({ signatures: signed });
    await m.app.openPath(path);
    await flush();
    expect(badge().textContent).toBe(INTACT);
    return m;
  }

  it("drops the verdict when a copy is saved, and does not ask again", async () => {
    const { api } = await openSigned();
    key("S", { ctrlKey: true, shiftKey: true });
    await flush();
    // The tab now names the copy, and the copy is a file nobody has checked.
    expect(tabTitles()).toEqual(["copy.pdf"]);
    expect(badge().hidden).toBe(false);
    expect(badge().textContent).toBe(STALE);
    expect(badge().textContent).not.toContain("intact");
    expect(badge().classList.contains("sig-stale")).toBe(true);
    badge().click();
    expect(q("#signature-panel").textContent).toContain("has not been checked");
    expect(q("#signature-panel").textContent).toContain("close this tab and open the file again");
    expect(q("#signature-panel").textContent).not.toContain("Signature intact");
    // Nothing offers to check again (the engine would answer for the file it opened), and
    // coming back to the tab asks nothing.
    expect(q("#signature-panel").querySelector("button.btn:not(.btn-icon)")).toBeNull();
    await new Promise((r) => setTimeout(r, 20));
    expect((api as unknown as { getSignatures: ReturnType<typeof vi.fn> }).getSignatures).toHaveBeenCalledTimes(1);
  });

  it("drops the verdict when the document is saved in place", async () => {
    const m = await openSigned();
    m.app.recordEdit({ label: "Change", undo: () => undefined, redo: () => undefined });
    // The change is in memory only; the verdict was about the file as it was opened.
    expect(badge().textContent).toBe(STALE);
    key("s", { ctrlKey: true });
    await flush();
    expect(m.api.save).toHaveBeenCalledTimes(1);
    expect(badge().textContent).toBe(STALE);
  });

  it("drops the verdict as soon as the document changes in memory, before any save", async () => {
    const m = await openSigned();
    m.app.recordEdit({ label: "Change", undo: () => undefined, redo: () => undefined });
    expect(badge().textContent).toBe(STALE);
    // Undoing the change does not bring the verdict back: it was dropped for this tab.
    key("z", { ctrlKey: true });
    await flush();
    expect(badge().textContent).toBe(STALE);
  });

  it("does not make a badge for a document that has no signatures", async () => {
    const m = make({ signatures: { total: 0, unlisted: 0, trustAnchors: 0, items: [] } });
    await m.app.openPath("/docs/plain.pdf");
    await flush();
    expect(badge().hidden).toBe(true);
    key("S", { ctrlKey: true, shiftKey: true });
    await flush();
    expect(badge().hidden).toBe(true);
  });

  it("drops the verdict at once when the engine refuses to draw from a file that changed", async () => {
    const m = make({ signatures: signed });
    // The first page is drawn from other bytes than the ones that were checked: the engine says
    // so, whatever the check had found before.
    m.api.render.mockRejectedValue({ code: "source_changed", message: "the file has changed" });
    await m.app.openPath("/docs/signed.pdf");
    await flush();
    expect(badge().hidden).toBe(false);
    expect(badge().textContent).toBe(STALE);
    expect(badge().textContent).not.toContain("intact");
    badge().click();
    const text = q("#signature-panel").textContent ?? "";
    expect(text).toContain("The file on the disk has been changed");
    expect(text).not.toContain("Signature intact");
    expect(q("#signature-panel").querySelector("button.btn:not(.btn-icon)")).toBeNull();
    // It stays dropped, and nothing asked the engine for a second verdict.
    await new Promise((r) => setTimeout(r, 20));
    expect((m.api as unknown as { getSignatures: ReturnType<typeof vi.fn> }).getSignatures).toHaveBeenCalledTimes(1);
  });

  it("drops the verdict when a read of the file failed after the check, and says the file may be unchanged", async () => {
    const m = make({ signatures: signed });
    // A page lost a part of what was signed to a read that failed: the file was not changed, and
    // the page would have been shown under the badge of an intact signature.
    m.api.render.mockRejectedValue({ code: "source_unreadable", message: "a part could not be read" });
    await m.app.openPath("/docs/signed.pdf");
    await flush();
    expect(badge().textContent).toBe(STALE);
    expect(badge().textContent).not.toContain("intact");
    badge().click();
    const text = q("#signature-panel").textContent ?? "";
    expect(text).toContain("could not be read");
    expect(text).toContain("missing content");
    expect(text).not.toContain("has been changed");
    expect(text).not.toContain("Signature intact");
    expect(q("#signature-panel").querySelector("button.btn:not(.btn-icon)")).toBeNull();
  });

  it("drops the verdict of a tab whose file was replaced by a save in another tab", async () => {
    const m = await openSigned("/docs/one.pdf");
    await m.app.openPath("/docs/two.pdf");
    await flush();
    expect(badge().textContent).toBe(INTACT);
    // The second tab saves a copy over the file the first one is open on (the app process says
    // which documents were open on the file it replaced).
    m.api.saveAs.mockResolvedValueOnce({ path: "/docs/target.pdf", name: "target.pdf", replaced: [1] });
    key("S", { ctrlKey: true, shiftKey: true });
    await flush();
    tab("one.pdf").click();
    await flush();
    expect(badge().textContent).toBe(STALE);
    badge().click();
    expect(q("#signature-panel").textContent).toContain("The file on the disk has been changed");
    expect(q("#signature-panel").textContent).not.toContain("Signature intact");
  });

  it("keeps the verdict of another tab", async () => {
    const m = await openSigned("/docs/one.pdf");
    await m.app.openPath("/docs/two.pdf");
    await flush();
    expect(badge().textContent).toBe(INTACT);
    // Save a copy of the second tab only.
    key("S", { ctrlKey: true, shiftKey: true });
    await flush();
    expect(badge().textContent).toBe(STALE);
    tab("one.pdf").click();
    await flush();
    expect(badge().textContent).toBe(INTACT);
  });
});

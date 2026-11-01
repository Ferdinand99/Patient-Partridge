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

beforeEach(() => {
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
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

const flush = async () => {
  for (let i = 0; i < 8; i++) await new Promise((r) => setTimeout(r, 0));
};

const letter = { width: 612, height: 792 };
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

function make(options: { measureOn?: boolean; storage?: MemoryStorage } = {}) {
  let nextDoc = 1;
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
    getOutline: vi.fn((_doc: number) =>
      Promise.resolve([
        { depth: 0, page: 0, title: "Start" },
        { depth: 0, page: 2, title: "End" },
      ]),
    ),
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

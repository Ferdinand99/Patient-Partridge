// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedDocument } from "../lib/api";
import { App } from "./app";

const putImageData = vi.fn();

function installPage(): void {
  const html = readFileSync("index.html", "utf8");
  const body = /<body>([\s\S]*)<\/body>/.exec(html)?.[1] ?? "";
  document.body.innerHTML = body.replace(/<script[\s\S]*?<\/script>/g, "");
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
const opened = (doc: number, pages: number, name = "book.pdf"): OpenedDocument => ({
  doc,
  name,
  pages: Array.from({ length: pages }, () => letter),
});

function make(measureOn = false) {
  const api = {
    open: vi.fn((_path: string, _password?: string) => Promise.resolve(opened(1, 3))),
    render: vi.fn((_d: number, _p: number, w: number, h: number) =>
      Promise.resolve(new ArrayBuffer(w * h * 4)),
    ),
    close: vi.fn((_doc: number) => Promise.resolve()),
    measureMode: vi.fn(() => Promise.resolve(measureOn)),
    measureReport: vi.fn((_a: number, _b: number, _c: number) => Promise.resolve()),
  };
  const deps = {
    api,
    pickFile: vi.fn(() => Promise.resolve<string | null>(null)),
    setWindowTitle: vi.fn(),
    now: vi.fn(() => 0),
  };
  const app = new App(document, deps);
  app.init();
  return { app, api, deps };
}

const q = <T extends HTMLElement>(selector: string) => document.querySelector(selector) as T;
const notice = () => document.querySelector<HTMLElement>(".notice");
const click = (el: Element | null) => (el as HTMLElement).click();
const button = (label: string) =>
  [...document.querySelectorAll<HTMLButtonElement>(".notice button")].find((b) => b.textContent === label) ?? null;

describe("App: opening a file", () => {
  it("shows the document, the page count and the title", async () => {
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
  });

  it("opens from the file dialog, and does nothing when it is cancelled", async () => {
    const { app, deps, api } = make();
    await app.openDialog();
    expect(api.open).not.toHaveBeenCalled();
    deps.pickFile.mockResolvedValueOnce("/x/y.pdf");
    await app.openDialog();
    expect(api.open).toHaveBeenCalledWith("/x/y.pdf", undefined);
  });

  it("replaces the open document and closes the old one in the engine", async () => {
    const { app, api } = make();
    api.open.mockResolvedValueOnce(opened(1, 3, "a.pdf")).mockResolvedValueOnce(opened(2, 5, "b.pdf"));
    await app.openPath("/a.pdf");
    await app.openPath("/b.pdf");
    await flush();
    expect(api.close).toHaveBeenCalledWith(1);
    expect(q("#page-total").textContent).toBe("/ 5");
    expect(document.querySelectorAll(".viewer").length).toBe(1);
  });

  it("closes a file that finished opening after a newer one was asked for", async () => {
    let finishFirst: (d: OpenedDocument) => void = () => undefined;
    const { app, api } = make();
    api.open
      .mockImplementationOnce(() => new Promise<OpenedDocument>((r) => (finishFirst = r)))
      .mockResolvedValueOnce(opened(2, 2, "new.pdf"));
    const first = app.openPath("/slow.pdf");
    await app.openPath("/new.pdf");
    finishFirst(opened(1, 9, "slow.pdf"));
    await first;
    await flush();
    expect(api.close).toHaveBeenCalledWith(1);
    expect(q("#page-total").textContent).toBe("/ 2");
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
    const input = q<HTMLInputElement>(".notice-input");
    expect(input.type).toBe("password");

    input.value = "wrong";
    click(button("Open"));
    await flush();
    expect(notice()?.textContent).toContain("not correct");

    q<HTMLInputElement>(".notice-input").value = "right";
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
  const key = (k: string, init: KeyboardEventInit = {}) =>
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));

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
    expect(deps.pickFile).toHaveBeenCalledTimes(1);
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

describe("App: measurement mode", () => {
  it("reports the open and first paint times once", async () => {
    let t = 0;
    const { app, api, deps } = make(true);
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

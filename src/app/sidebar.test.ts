// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { SIDEBAR_KEY, Sidebar, parseSidebarState, type SidebarDocument } from "./sidebar";

const letter = { width: 612, height: 792 };
const putImageData = vi.fn();

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
  putImageData.mockReset();
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockImplementation(
    () => ({ putImageData }) as unknown as CanvasRenderingContext2D,
  );
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.id === "thumbs" || this.id === "outline" ? 600 : 0;
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

class MemoryStorage {
  readonly data = new Map<string, string>();
  getItem(key: string): string | null {
    return this.data.get(key) ?? null;
  }
  setItem(key: string, value: string): void {
    this.data.set(key, value);
  }
}

function make(stored?: string) {
  const storage = new MemoryStorage();
  if (stored) storage.setItem(SIDEBAR_KEY, stored);
  const api = {
    render: vi.fn((_d: number, _p: number, w: number, h: number) => Promise.resolve(new ArrayBuffer(w * h * 4))),
    getOutline: vi.fn((_doc: number) =>
      Promise.resolve([
        { depth: 0, page: 0, title: "Start" },
        { depth: 0, page: 3, title: "Middle" },
      ]),
    ),
  };
  const sidebar = new Sidebar(document, { api, storage, thumbs: { settleMs: 5 } });
  return { sidebar, api, storage };
}

function doc(id: number, pages = 30, painted = true): SidebarDocument & { goToPage: Mock<(index: number) => void> } {
  return {
    doc: id,
    pages: Array.from({ length: pages }, () => letter),
    goToPage: vi.fn(),
    hasPainted: () => painted,
    currentPage: () => 0,
  };
}

const q = <T extends HTMLElement>(selector: string) => document.querySelector(selector) as T;

describe("parseSidebarState", () => {
  it("reads a stored state and falls back to closed", () => {
    expect(parseSidebarState('{"open":true,"panel":"contents"}')).toEqual({ open: true, panel: "contents" });
    expect(parseSidebarState('{"open":true}')).toEqual({ open: true, panel: "pages" });
    for (const bad of [null, undefined, "", "nonsense", '{"open":"yes","panel":3}', "[]"]) {
      expect(parseSidebarState(bad)).toEqual({ open: false, panel: "pages" });
    }
  });
});

describe("Sidebar", () => {
  it("starts closed and does nothing while it is closed", async () => {
    const { sidebar, api } = make();
    sidebar.setDocument(doc(1));
    sidebar.notifyPainted();
    await wait(30);
    expect(q("#sidebar").hidden).toBe(true);
    expect(q("#sidebar-toggle").getAttribute("aria-pressed")).toBe("false");
    expect(api.render).not.toHaveBeenCalled();
    expect(api.getOutline).not.toHaveBeenCalled();
    expect(document.querySelectorAll(".thumb").length).toBe(0);
  });

  it("opens with the button, remembers it, and draws thumbnails of the document", async () => {
    const { sidebar, api, storage } = make();
    sidebar.setDocument(doc(1));
    q("#sidebar-toggle").click();
    expect(q("#sidebar").hidden).toBe(false);
    expect(q("#sidebar-toggle").getAttribute("aria-pressed")).toBe("true");
    expect(JSON.parse(storage.data.get(SIDEBAR_KEY) as string)).toEqual({ open: true, panel: "pages" });
    await wait(30);
    await flush();
    expect(api.render).toHaveBeenCalled();
    expect(api.render.mock.calls.every(([d]) => d === 1)).toBe(true);
    expect(document.querySelectorAll(".thumb").length).toBeGreaterThan(0);
  });

  it("opens at start when it was left open", () => {
    const { sidebar } = make('{"open":true,"panel":"contents"}');
    expect(sidebar.isOpen).toBe(true);
    expect(q("#sidebar").hidden).toBe(false);
    expect(q("#outline").hidden).toBe(false);
    expect(q("#thumbs").hidden).toBe(true);
    expect(q("#side-tab-contents").getAttribute("aria-selected")).toBe("true");
  });

  it("waits for the first page of the main view before it draws thumbnails", async () => {
    const { sidebar, api } = make('{"open":true,"panel":"pages"}');
    let painted = false;
    const d = { ...doc(1), hasPainted: () => painted };
    sidebar.setDocument(d);
    await wait(30);
    expect(api.render).not.toHaveBeenCalled();
    painted = true;
    sidebar.notifyPainted();
    await wait(30);
    await flush();
    expect(api.render).toHaveBeenCalled();
  });

  it("goes to the page of a thumbnail that is clicked", () => {
    const { sidebar } = make('{"open":true,"panel":"pages"}');
    const d = doc(1);
    sidebar.setDocument(d);
    q('[data-page="2"]').click();
    expect(d.goToPage).toHaveBeenCalledWith(2);
  });

  it("asks for the table of contents once, the first time it is shown", async () => {
    const { sidebar, api } = make('{"open":true,"panel":"pages"}');
    sidebar.setDocument(doc(1));
    expect(api.getOutline).not.toHaveBeenCalled();
    q("#side-tab-contents").click();
    expect(q("#outline").textContent).toContain("Reading");
    await flush();
    expect(api.getOutline).toHaveBeenCalledTimes(1);
    expect(q("#outline").textContent).toContain("Middle");
    // Back and forth: no second request, and the thumbnails come back.
    q("#side-tab-pages").click();
    q("#side-tab-contents").click();
    await flush();
    expect(api.getOutline).toHaveBeenCalledTimes(1);
    expect(q("#outline").textContent).toContain("Middle");
  });

  it("goes to the page of an entry of the contents", async () => {
    const { sidebar } = make('{"open":true,"panel":"contents"}');
    const d = doc(1);
    sidebar.setDocument(d);
    await flush();
    [...document.querySelectorAll<HTMLElement>(".outline-link")].find((l) => l.textContent === "Middle")?.click();
    expect(d.goToPage).toHaveBeenCalledWith(3);
  });

  it("shows the contents of the document in front when the document changes", async () => {
    const { sidebar, api } = make('{"open":true,"panel":"contents"}');
    api.getOutline.mockImplementation((id: number) =>
      Promise.resolve([{ depth: 0, page: 0, title: id === 1 ? "First book" : "Second book" }]),
    );
    sidebar.setDocument(doc(1));
    await flush();
    expect(q("#outline").textContent).toContain("First book");
    sidebar.setDocument(doc(2));
    await flush();
    expect(q("#outline").textContent).toContain("Second book");
    sidebar.setDocument(null);
    sidebar.setDocument(doc(1));
    await flush();
    expect(q("#outline").textContent).toContain("First book");
    // Document 1 was asked once; coming back used what it had.
    expect(api.getOutline.mock.calls.filter(([id]) => id === 1)).toHaveLength(1);
  });

  it("does not show the contents of a document that is no longer in front", async () => {
    const { sidebar, api } = make('{"open":true,"panel":"contents"}');
    let finish: (v: { depth: number; page: number; title: string }[]) => void = () => undefined;
    api.getOutline.mockImplementationOnce(() => new Promise((r) => (finish = r)));
    sidebar.setDocument(doc(1));
    sidebar.setDocument(doc(2));
    await flush();
    finish([{ depth: 0, page: 0, title: "Late answer" }]);
    await flush();
    expect(q("#outline").textContent).not.toContain("Late answer");
  });

  it("says so when the contents cannot be read, and tries again next time", async () => {
    const { sidebar, api } = make('{"open":true,"panel":"contents"}');
    api.getOutline.mockRejectedValueOnce({ code: "unknown_document", message: "gone" });
    sidebar.setDocument(doc(1));
    await flush();
    expect(q("#outline").textContent).toContain("could not be read");
    q("#side-tab-pages").click();
    q("#side-tab-contents").click();
    await flush();
    expect(api.getOutline).toHaveBeenCalledTimes(2);
    expect(q("#outline").textContent).toContain("Middle");
  });

  it("stops drawing and drops its elements when it is closed", async () => {
    const { sidebar } = make('{"open":true,"panel":"pages"}');
    sidebar.setDocument(doc(1));
    await wait(30);
    expect(document.querySelectorAll(".thumb").length).toBeGreaterThan(0);
    q("#sidebar-toggle").click();
    expect(q("#sidebar").hidden).toBe(true);
    expect(document.querySelectorAll(".thumb").length).toBe(0);
  });

  it("closes with Escape from inside and gives the focus back to the button", () => {
    const { sidebar } = make('{"open":true,"panel":"pages"}');
    sidebar.setDocument(doc(1));
    q("#side-tab-pages").focus();
    q("#side-tab-pages").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(sidebar.isOpen).toBe(false);
    expect(document.activeElement).toBe(q("#sidebar-toggle"));
  });

  it("keeps the scroll place of a document's thumbnails when another tab is shown", async () => {
    const { sidebar } = make('{"open":true,"panel":"pages"}');
    const a = doc(1, 400);
    sidebar.setDocument(a);
    q("#thumbs").scrollTop = 20_000;
    sidebar.setDocument(doc(2, 5));
    sidebar.setDocument(a);
    await wait(10);
    expect(q("#thumbs").scrollTop).toBe(20_000);
  });

  it("marks the current page in both panels", async () => {
    const { sidebar } = make('{"open":true,"panel":"pages"}');
    sidebar.setDocument(doc(1));
    sidebar.setCurrentPage(4);
    expect(q('.thumb[aria-current="page"]').dataset.page).toBe("4");
    q("#side-tab-contents").click();
    await flush();
    sidebar.setCurrentPage(3);
    expect(q('.outline-link[aria-current="location"]').textContent).toBe("Middle");
  });
});

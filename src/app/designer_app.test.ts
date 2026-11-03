// @vitest-environment jsdom
// The form designer through the whole window: the button and the bar, the exclusion with the
// annotation bar, a change as one call and one step of the history, the signature verdict dropped
// by a change, the changes sent before a save, and the layer that waits while a page tool works.
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedDocument, SavedFile } from "../lib/api";
import type { DesignOp, DesignWidgetInfo } from "../lib/formdesign";
import type { PagesChanged, PageToolsApi } from "../lib/pagesapi";
import type { SignaturesInfo } from "../lib/signatures";
import { App } from "./app";
import { BATCH_REST_MS } from "./formdesigner";

const addedListeners: [EventTarget, string, EventListenerOrEventListenerObject, unknown][] = [];
const letter = { width: 612, height: 792 };

beforeEach(() => {
  for (const target of [document, window] as EventTarget[]) {
    const original = target.addEventListener.bind(target);
    vi.spyOn(target, "addEventListener").mockImplementation((type, listener, options) => {
      if (listener) addedListeners.push([target, type, listener, options]);
      original(type, listener, options);
    });
  }
  const html = readFileSync("index.html", "utf8");
  const body = /<body>([\s\S]*)<\/body>/.exec(html)?.[1] ?? "";
  document.body.innerHTML = body.replace(/<script[\s\S]*?<\/script>/g, "");
  document.documentElement.removeAttribute("data-theme");
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
      return this.classList.contains("viewer") ? 800 : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList.contains("viewer") ? 900 : 0;
    },
  });
  // A page is 612 by 792 pixels at the corner of the window, so a pixel is a point.
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const page = this.classList.contains("viewer-page");
    return { left: 0, top: 0, width: page ? 612 : 0, height: page ? 792 : 0, right: 0, bottom: 0, x: 0, y: 0, toJSON: () => ({}) };
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

const changed = (snapshot = 0, count = 1): PagesChanged => ({ pages: [letter, letter], snapshot, count, formFields: 0 });

function pointer(type: string, x: number, y: number): Event {
  const init = { clientX: x, clientY: y, button: 0, pointerId: 1, bubbles: true, cancelable: true };
  return typeof PointerEvent === "function"
    ? new PointerEvent(type, init)
    : (Object.assign(new MouseEvent(type, init), { pointerId: 1 }) as unknown as Event);
}

function signed(): SignaturesInfo {
  return {
    total: 1,
    unlisted: 0,
    trustAnchors: 0,
    items: [
      {
        index: 0,
        state: "unknown_signer",
        reasons: [],
        name: "Test Signer",
        signer: "Test Signer",
        reason: "",
        location: "",
        contact: "",
        signedAt: null,
        signedAtText: "",
        certificates: [],
        coversWholeFile: true,
        revision: 1,
        revisions: 1,
        field: "Signature1",
      },
    ],
  } as unknown as SignaturesInfo;
}

function make(options: { withDesign?: boolean; signatures?: SignaturesInfo; listed?: DesignWidgetInfo[]; distinctDocs?: boolean } = {}) {
  const state = { listed: options.listed ?? [] };
  let nextDoc = 0;
  const calls: string[] = [];
  const pages = {
    pagesDelete: vi.fn(() => Promise.resolve(changed())),
    pagesReorder: vi.fn(() => Promise.resolve(changed())),
    pagesRotate: vi.fn(() => Promise.resolve(changed())),
    pagesCrop: vi.fn(() => Promise.resolve({ pages: [letter, letter], before: [], after: [] })),
    pagesSetBoxes: vi.fn(() => Promise.resolve(changed())),
    pagesInsertBlank: vi.fn(() => Promise.resolve(changed(9))),
    pagesRestore: vi.fn((_d: number, snapshot: number) => Promise.resolve(changed(snapshot + 100, 0))),
    pagesInsertPdf: vi.fn(() => Promise.resolve({ changed: changed(), inserted: 1 })),
    pagesExtract: vi.fn(() => Promise.resolve(null)),
    pagesCancel: vi.fn(() => Promise.resolve("idle" as const)),
    ...(options.withDesign === false
      ? {}
      : {
          designCheck: vi.fn(() => Promise.resolve([] as string[])),
          designList: vi.fn((_d: number, page: number) => Promise.resolve(page === 0 ? structuredClone(state.listed) : [])),
          designApply: vi.fn((_d: number, ops: DesignOp[]) => {
            calls.push(`apply:${ops.length}`);
            return Promise.resolve(changed(21, ops.length));
          }),
        }),
  };
  const api = {
    open: vi.fn((path: string) =>
      Promise.resolve<OpenedDocument>({ doc: options.distinctDocs ? ++nextDoc : 1, name: path.split("/").pop() ?? "", pages: [letter, letter], position: null }),
    ),
    render: vi.fn((_d: number, _p: number, w: number, h: number) => Promise.resolve(new ArrayBuffer(w * h * 4))),
    close: vi.fn(() => Promise.resolve()),
    setPosition: vi.fn(() => Promise.resolve()),
    clearPositions: vi.fn(() => Promise.resolve()),
    getReopenTabs: vi.fn(() => Promise.resolve(true)),
    setReopenTabs: vi.fn(() => Promise.resolve()),
    saveSession: vi.fn(() => Promise.resolve()),
    getOutline: vi.fn(() => Promise.resolve([])),
    getPageText: vi.fn(() => Promise.resolve({ text: "", rects: [] })),
    getPageLinks: vi.fn(() => Promise.resolve([])),
    searchPage: vi.fn(() => Promise.resolve([] as [number, number][])),
    openLink: vi.fn(() => Promise.resolve()),
    measureMode: vi.fn(() => Promise.resolve(false)),
    measureReport: vi.fn(() => Promise.resolve()),
    setDirty: vi.fn(() => Promise.resolve()),
    save: vi.fn(() => {
      calls.push("save");
      return Promise.resolve<SavedFile>({ path: "/docs/form.pdf", name: "form.pdf" });
    }),
    saveAs: vi.fn(() => Promise.resolve<SavedFile | null>(null)),
    quitApp: vi.fn(() => Promise.resolve()),
    stayOpen: vi.fn(() => Promise.resolve()),
    listAnnotations: vi.fn(() => Promise.resolve([])),
    addMarkup: vi.fn(),
    addNote: vi.fn(),
    addInk: vi.fn(),
    addStamp: vi.fn(),
    updateAnnotation: vi.fn(),
    removeAnnotation: vi.fn(),
    restoreAnnotation: vi.fn(),
    ...(options.signatures ? { getSignatures: vi.fn(() => Promise.resolve(options.signatures as SignaturesInfo)) } : {}),
  };
  const deps = {
    api,
    pickFiles: vi.fn(() => Promise.resolve<string[]>([])),
    setWindowTitle: vi.fn(),
    now: vi.fn(() => 0),
    storage: null,
    positionDelayMs: 5,
    thumbSettleMs: 5,
    findDebounceMs: 5,
    linkAnswerDelayMs: 0,
    signaturesDelayMs: 0,
    pages: pages as unknown as PageToolsApi,
    pageBusyDelayMs: 1000,
  };
  const app = new App(document, deps);
  app.init();
  return { app, api, pages, calls, state };
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const designButton = () => $<HTMLButtonElement>("design-button");
const annotateButton = () => $<HTMLButtonElement>("annotate-button");
const place = (x: number, y: number) => {
  const capture = document.querySelector(".design-capture") as HTMLElement;
  capture.dispatchEvent(pointer("pointerdown", x, y));
  capture.dispatchEvent(pointer("pointerup", x, y));
};
const boxes = () => document.querySelectorAll(".design-box");
const keyOn = (el: HTMLElement, key: string, init: KeyboardEventInit = {}) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));

async function openDesigner(m: ReturnType<typeof make>) {
  await m.app.openPath("/docs/form.pdf");
  await flush();
  designButton().click();
  await flush();
}

describe("the button and the bar", () => {
  it("shows the button only when the app process has the form designer", async () => {
    const without = make({ withDesign: false });
    await without.app.openPath("/docs/form.pdf");
    await flush();
    expect(designButton().hidden).toBe(true);
    expect($("designbar").hidden).toBe(true);
  });

  it("opens the bar and the panel on a document, and checks with the engine first", async () => {
    const m = make();
    await m.app.openPath("/docs/form.pdf");
    await flush();
    expect(designButton().hidden).toBe(false);
    expect(designButton().disabled).toBe(false);
    designButton().click();
    await flush();
    expect(m.pages.designCheck?.mock.calls[0]).toEqual([1]);
    expect($("designbar").hidden).toBe(false);
    expect($("design-panel").hidden).toBe(false);
    expect(designButton().getAttribute("aria-pressed")).toBe("true");
    designButton().click();
    await flush();
    expect($("designbar").hidden).toBe(true);
    expect($("design-panel").hidden).toBe(true);
  });

  it("closes the annotation bar when it opens, and the other way round", async () => {
    const m = make({});
    await openDesigner(m);
    annotateButton().click();
    await flush();
    expect($("annotbar").hidden).toBe(false);
    expect($("designbar").hidden).toBe(true);
    designButton().click();
    await flush();
    expect($("designbar").hidden).toBe(false);
    expect($("annotbar").hidden).toBe(true);
  });

  it("stays closed, and says why, when the document cannot take fields", async () => {
    const m = make();
    m.pages.designCheck?.mockRejectedValueOnce({ code: "author_unsupported", message: "this document is encrypted, so fields cannot be added to it" });
    await m.app.openPath("/docs/form.pdf");
    await flush();
    designButton().click();
    await flush();
    expect($("designbar").hidden).toBe(true);
    expect(document.querySelector(".notice")?.textContent).toMatch(/encrypted/);
  });
});

describe("a change", () => {
  it("is one call and one step of the history, and Undo takes it back through the copy", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"], shouldAdvanceTime: true });
    try {
      const m = make();
      await openDesigner(m);
      $("dtool-text").click();
      place(300, 400);
      place(300, 500);
      expect(m.pages.designApply?.mock.calls).toHaveLength(0);
      expect(document.querySelectorAll(".design-box")).toHaveLength(1);
      vi.advanceTimersByTime(BATCH_REST_MS + 10);
      await flush();
      expect(m.pages.designApply?.mock.calls).toHaveLength(1);
      const ops = m.pages.designApply?.mock.calls[0][1] ?? [];
      expect(ops).toHaveLength(1);
      expect(ops[0].op).toBe("add");
      const undo = $<HTMLButtonElement>("design-undo");
      expect(undo.disabled).toBe(false);
      expect(undo.title).toMatch(/Undo add a text field/i);
      undo.click();
      await flush();
      expect(m.pages.pagesRestore).toHaveBeenCalledWith(1, 21);
    } finally {
      vi.useRealTimers();
    }
  });

  it("drops the verdict on a signature, like any edit", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"], shouldAdvanceTime: true });
    try {
      const m = make({ signatures: signed() });
      await m.app.openPath("/docs/form.pdf");
      await flush();
      const badge = () => $("signature-badge");
      expect(badge().classList.contains("sig-unknown_signer")).toBe(true);
      designButton().click();
      await flush();
      $("dtool-text").click();
      place(300, 400);
      vi.advanceTimersByTime(BATCH_REST_MS + 10);
      await flush();
      expect(m.pages.designApply?.mock.calls).toHaveLength(1);
      expect(badge().classList.contains("sig-stale")).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("is sent before the file is saved, and the document counts as changed from the start", async () => {
    const m = make();
    await openDesigner(m);
    $("dtool-checkbox").click();
    place(300, 400);
    expect(m.api.setDirty).toHaveBeenCalledWith(1, true);
    keyOn(document.body, "s", { ctrlKey: true });
    await flush();
    expect(m.calls).toEqual(["apply:1", "save"]);
  });

  it("is sent before a page tool begins, so the pages change under the form as it is", async () => {
    const m = make();
    await openDesigner(m);
    $("dtool-text").click();
    place(300, 400);
    const ops = (m.app as unknown as { pageOps: { insertBlank(at: number, w: number, h: number): Promise<unknown> } }).pageOps;
    await ops.insertBlank(1, 612, 792);
    await flush();
    expect(m.pages.designApply?.mock.calls).toHaveLength(1);
    expect(m.pages.pagesInsertBlank).toHaveBeenCalled();
    expect(m.pages.designApply?.mock.invocationCallOrder[0]).toBeLessThan(m.pages.pagesInsertBlank.mock.invocationCallOrder[0]);
  });

  it("is made with Delete on the picked box and shows at once", async () => {
    const m = make({
      listed: [{ name: "a", kind: "text", widget: 0, widgets: 1, rect: [100, 100, 260, 122], export: "", editable: true, props: { tooltip: "", required: false, readOnly: false, multiline: false, password: false, comb: false, maxLen: 0, editable: false, multiselect: false, fontSize: 0, defaultValue: "", choices: [], exportValue: "" } }],
    });
    await openDesigner(m);
    expect(boxes()).toHaveLength(1);
    keyOn(document.querySelector(".design-box") as HTMLElement, "Delete");
    expect(boxes()).toHaveLength(0);
    await (m.app as unknown as { flushForms(): Promise<void> }).flushForms();
    expect(m.pages.designApply?.mock.calls).toEqual([[1, [{ op: "remove", name: "a" }]]]);
  });
});

describe("while a page tool works", () => {
  it("the boxes wait, and the form is not filled in while the designer is on", async () => {
    const m = make();
    let finish!: () => void;
    m.pages.pagesInsertBlank.mockImplementationOnce(() => new Promise((resolve) => (finish = () => resolve(changed(9)))));
    await openDesigner(m);
    const layer = () => document.querySelector(".design-layer") as HTMLElement;
    expect(layer().inert).toBe(false);
    expect(document.querySelector(".viewer")!.classList.contains("viewer-designing")).toBe(true);
    const ops = (m.app as unknown as { pageOps: { insertBlank(at: number, w: number, h: number): Promise<unknown> } }).pageOps;
    const running = ops.insertBlank(1, 612, 792);
    await flush();
    expect(layer().inert).toBe(true);
    finish();
    await running;
    await flush();
    expect(document.querySelector(".design-layer")).not.toBeNull();
    expect((document.querySelector(".design-layer") as HTMLElement).inert).toBe(false);
  });
});

describe("the changes of a tab that was left", () => {
  const tabs = (app: App) => (app as unknown as { tabs: { id: number; doc: number; history: { undoDepth: number } }[] }).tabs;

  it("go to that document and to no other when the reader goes to another tab before the rest is over", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"], shouldAdvanceTime: true });
    try {
      const m = make({ distinctDocs: true });
      await m.app.openPath("/docs/first.pdf");
      await flush();
      designButton().click();
      await flush();
      $("dtool-text").click();
      place(300, 400);
      expect(boxes()).toHaveLength(1);
      expect(m.pages.designApply?.mock.calls).toHaveLength(0);
      // The second document is opened (it comes to the front) within the rest.
      await m.app.openPath("/docs/second.pdf");
      await flush();
      vi.advanceTimersByTime(BATCH_REST_MS + 10);
      await flush();
      // One call, for the first document. The second document got nothing.
      expect(m.pages.designApply?.mock.calls.map((c) => c[0])).toEqual([1]);
      expect(m.pages.designApply?.mock.calls[0][1].map((o: DesignOp) => o.op)).toEqual(["add"]);
      const [first, second] = tabs(m.app);
      expect(first.history.undoDepth).toBe(1);
      expect(second.history.undoDepth).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it("go to that document when the rest is over while the other tab is in front", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"], shouldAdvanceTime: true });
    try {
      const m = make({ distinctDocs: true });
      await m.app.openPath("/docs/first.pdf");
      await flush();
      designButton().click();
      await flush();
      $("dtool-text").click();
      place(300, 400);
      // The tab is left through the page tools of the other, which hold the timer of the first back.
      const first = tabs(m.app)[0];
      m.pages.designApply?.mockImplementationOnce((d: number, ops: DesignOp[]) => {
        m.calls.push(`apply:${d}:${ops.length}`);
        return Promise.resolve(changed(21, ops.length));
      });
      await m.app.openPath("/docs/second.pdf");
      await flush();
      vi.advanceTimersByTime(BATCH_REST_MS + 10);
      await flush();
      expect(m.calls).toEqual(["apply:1:1"]);
      expect(first.history.undoDepth).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("a field made with the keyboard", () => {
  it("is placed when a tool button is pressed with Enter, and the reader is told where it is", async () => {
    const m = make();
    await openDesigner(m);
    const button = $<HTMLButtonElement>("dtool-text");
    button.focus();
    button.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    button.click();
    expect(boxes()).toHaveLength(1);
    expect(document.getElementById("designbar")!.hidden).toBe(false);
    expect($("dtool-select").getAttribute("aria-pressed")).toBe("true");
    await (m.app as unknown as { flushForms(): Promise<void> }).flushForms();
    const ops = m.pages.designApply?.mock.calls[0][1] ?? [];
    expect(ops[0].op).toBe("add");
    expect(ops[0].op === "add" && ops[0].field.widgets[0].rect).toEqual([226, 385, 386, 407]);
  });

  it("takes the tool, and waits for a click on the page, when the button is pressed with the pointer", async () => {
    const m = make();
    await openDesigner(m);
    $("dtool-text").click();
    expect(boxes()).toHaveLength(0);
    expect($("dtool-text").getAttribute("aria-pressed")).toBe("true");
  });
});

describe("while a file is written", () => {
  it("nothing is sent to the engine until it is done, and the document stays changed", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"], shouldAdvanceTime: true });
    try {
      const m = make();
      let finish!: () => void;
      m.api.save.mockImplementationOnce(() => {
        m.calls.push("save");
        return new Promise<SavedFile>((resolve) => {
          finish = () => resolve({ path: "/docs/form.pdf", name: "form.pdf" });
        });
      });
      await openDesigner(m);
      $("dtool-text").click();
      place(300, 400);
      // The first field goes before the file is written, as it always does.
      keyOn(document.body, "s", { ctrlKey: true });
      await flush();
      expect(m.calls).toEqual(["apply:1", "save"]);
      // Another field is placed while the file is written.
      $("dtool-text").click();
      place(300, 500);
      vi.advanceTimersByTime(BATCH_REST_MS + 10);
      await flush();
      expect(m.pages.designApply?.mock.calls).toHaveLength(1);
      finish();
      await flush();
      // Now it goes, after the save.
      vi.advanceTimersByTime(BATCH_REST_MS + 10);
      await flush();
      expect(m.calls).toEqual(["apply:1", "save", "apply:1"]);
    } finally {
      vi.useRealTimers();
    }
  });
});

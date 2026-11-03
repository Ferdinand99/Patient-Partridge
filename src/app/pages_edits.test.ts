// @vitest-environment jsdom
// Page tools, annotations and the fields of a form on one document, through the whole window: the
// annotation tools, the note box and the fields wait while the pages change, the layers are made
// again for the new pages, and Undo and Redo take all three kinds of change in order.
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedDocument, SavedFile } from "../lib/api";
import type { Annotation } from "../lib/annotations";
import type { FormFieldInfo, FormInfo, FormValueIn } from "../viewer/forms";
import { App, undoFailure } from "./app";
import type { PageOps } from "./pageops";
import type { PageToolsApi, PagesChanged } from "../lib/pagesapi";
import type { SignaturesInfo } from "../lib/signatures";

const addedListeners: [EventTarget, string, EventListenerOrEventListenerObject, unknown][] = [];

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

function textField(index: number, name: string, value = ""): FormFieldInfo {
  return {
    index,
    kind: "text",
    readOnly: false,
    required: false,
    multiline: false,
    password: false,
    editable: false,
    multiselect: false,
    checked: false,
    valueCut: false,
    noToggleOff: false,
    comb: false,
    action: false,
    actionBlocked: false,
    formBlocked: false,
    turns: 0,
    rect: [100, 100, 300, 124],
    fontSize: 12,
    name,
    tooltip: "",
    value,
    maxLen: 0,
    options: [],
  };
}

function make(
  options: {
    fields?: FormFieldInfo[];
    info?: FormInfo;
    withForms?: boolean;
    withAnnotations?: boolean;
    pages?: PageToolsApi;
    signatures?: SignaturesInfo;
  } = {},
) {
  const state = structuredClone(options.fields ?? [textField(0, "name"), { ...textField(1, "agree"), kind: "checkbox" as const }]);
  const api = {
    open: vi.fn((path: string, _password?: string) =>
      Promise.resolve<OpenedDocument>({ doc: 1, name: path.split("/").pop() ?? "", pages: [letter, letter, letter], position: null }),
    ),
    render: vi.fn((_d: number, _p: number, w: number, h: number) => Promise.resolve(new ArrayBuffer(w * h * 4))),
    close: vi.fn((_doc: number) => Promise.resolve()),
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
    setDirty: vi.fn((_doc: number, _dirty: boolean) => Promise.resolve()),
    save: vi.fn(() => Promise.resolve<SavedFile>({ path: "/docs/form.pdf", name: "form.pdf" })),
    saveAs: vi.fn(() => Promise.resolve<SavedFile | null>(null)),
    quitApp: vi.fn(() => Promise.resolve()),
    stayOpen: vi.fn(() => Promise.resolve()),
    ...(options.signatures
      ? { getSignatures: vi.fn((_doc: number) => Promise.resolve(options.signatures as SignaturesInfo)) }
      : {}),
    formInfo: vi.fn((_doc: number) => Promise.resolve<FormInfo>(options.info ?? { kind: "acro", actions: false })),
    getFormFields: vi.fn((_doc: number, page: number) => Promise.resolve(page === 0 ? structuredClone(state) : [])),
    setFormField: vi.fn(
      (_doc: number, _page: number, index: number, name: string, value: FormValueIn): Promise<FormFieldInfo[]> => {
        const target = state.find((f) => f.index === index && f.name === name);
        if (!target) return Promise.reject({ code: "form_field_refused", message: "gone" });
        if (value.kind === "text") target.value = value.text;
        if (value.kind === "checked") target.checked = value.on;
        if (value.kind === "choice") target.options.forEach((o, i) => (o.selected = value.picked.includes(i)));
        return Promise.resolve(structuredClone(state));
      },
    ),
  };
  let nextAnnotation = 50;
  const annotation = (id: number): Annotation => ({
    id,
    kind: "ink",
    can: { editText: false, recolour: false, move: false, remove: true, restore: true, locked: false },
    rect: [10, 10, 40, 40],
    color: [0, 0, 255, 255],
    text: "",
    author: "",
  });
  const annotationCalls = {
    listAnnotations: vi.fn((_doc: number, _page: number) => Promise.resolve<Annotation[]>([])),
    addMarkup: vi.fn(() => Promise.resolve(annotation(nextAnnotation++))),
    addNote: vi.fn(() => Promise.resolve(annotation(nextAnnotation++))),
    addInk: vi.fn(() => Promise.resolve(annotation(nextAnnotation++))),
    addStamp: vi.fn(() => Promise.resolve(annotation(nextAnnotation++))),
    updateAnnotation: vi.fn((_d: number, _p: number, id: number) => Promise.resolve(annotation(id))),
    removeAnnotation: vi.fn(() => Promise.resolve()),
    restoreAnnotation: vi.fn((_d: number, _p: number, id: number) => Promise.resolve(annotation(id))),
  };
  const deps = {
    api: {
      ...(options.withForms === false ? { ...api, formInfo: undefined, getFormFields: undefined, setFormField: undefined } : api),
      ...(options.withAnnotations ? annotationCalls : {}),
    },
    pickFiles: vi.fn(() => Promise.resolve<string[]>([])),
    setWindowTitle: vi.fn(),
    now: vi.fn(() => 0),
    storage: null,
    positionDelayMs: 5,
    thumbSettleMs: 5,
    findDebounceMs: 5,
    linkAnswerDelayMs: 0,
    signaturesDelayMs: 0,
    pages: options.pages,
    pageBusyDelayMs: 1000,
  };
  const app = new App(document, deps);
  app.init();
  return { app, api, deps, state, annotationCalls };
}


// ---------------------------------------------------------------------------------------------

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

interface AnnotatorLike {
  isHeld: boolean;
  tool: string;
  selected: unknown;
  setTool(tool: string): void;
  select(page: number, id: number | null): void;
  removeSelected(): Promise<void>;
  onStroke(page: number, points: [number, number][]): void;
  onPlace(page: number, x: number, y: number, client: { x: number; y: number }): void;
  moveBy(page: number, id: number, dx: number, dy: number): Promise<void>;
  idle(): Promise<void>;
  blockedReason: string | null;
}

const opsOf = (app: App) => (app as unknown as { pageOps: PageOps }).pageOps;
const activeOf = (app: App) =>
  (
    app as unknown as {
      active: { annotator: AnnotatorLike; view: { pageCount: number; invalidatePage(page: number): void }; history: { canUndo: boolean; canRedo: boolean } };
    }
  ).active;
const notices = () => document.querySelectorAll(".notice");
const dismiss = async () => {
  for (let i = 0; i < 5 && notices().length > 0; i++) {
    (document.querySelector(".notice .btn-primary, .notice .btn") as HTMLElement | null)?.click();
    await flush();
  }
};
const status = () => document.getElementById("annot-status")?.textContent ?? "";
const field = (index: number) => document.querySelector<HTMLElement>(`.form-field[data-form-index="${index}"]`) as HTMLElement;
const change = (el: HTMLElement) => el.dispatchEvent(new Event("change", { bubbles: true }));
const keyOn = (el: HTMLElement, key: string, init: KeyboardEventInit = {}) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));

/** A delete that has not answered yet: `finish` lets it end. */
function slowDelete(pages: ReturnType<typeof fakePages>) {
  let finish!: (c: PagesChanged) => void;
  pages.pagesDelete.mockImplementationOnce(() => new Promise<PagesChanged>((r) => (finish = r)));
  return { finish: (c: PagesChanged = changed(2, 7)) => finish(c) };
}

describe("after an annotation of the file is deleted for good", () => {
  const forever = {
    id: 3,
    kind: "other" as const,
    can: { editText: false, recolour: false, move: false, remove: true, restore: false, locked: false },
    rect: [10, 10, 40, 40] as [number, number, number, number],
    color: [0, 0, 0, 255] as [number, number, number, number],
    text: "",
    author: "",
  };

  async function open() {
    const pages = fakePages();
    const m = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    m.annotationCalls.listAnnotations.mockResolvedValue([forever]);
    await m.app.openPath("/docs/form.pdf");
    await flush();
    (m.app as unknown as { setAnnotating(open: boolean): void }).setAnnotating(true);
    activeOf(m.app).view.invalidatePage(0);
    await flush();
    return { ...m, pages };
  }

  async function deleteForGood(m: Awaited<ReturnType<typeof open>>) {
    activeOf(m.app).annotator.select(0, 3);
    const removing = activeOf(m.app).annotator.removeSelected();
    await flush();
    document.querySelector<HTMLElement>(".notice .btn-primary")?.click();
    await removing;
    await flush();
  }

  it("cannot be brought back by Redo of a page change that was taken back before", async () => {
    const m = await open();
    await opsOf(m.app).remove([2]);
    await flush();
    await dismiss();
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    expect(m.pages.pagesRestore).toHaveBeenCalledTimes(1);
    expect(activeOf(m.app).history.canRedo).toBe(true);

    await deleteForGood(m);
    expect(m.annotationCalls.removeAnnotation).toHaveBeenCalledTimes(1);
    expect(activeOf(m.app).history.canRedo).toBe(false);
    keyOn(document.body, "y", { ctrlKey: true });
    await flush();
    // Nothing was made again: no copy was loaded a second time, and nothing else was changed.
    expect(m.pages.pagesRestore).toHaveBeenCalledTimes(1);
    expect(m.pages.pagesDelete).toHaveBeenCalledTimes(1);
    expect(m.annotationCalls.restoreAnnotation).not.toHaveBeenCalled();
  });

  const steps: [string, (m: Awaited<ReturnType<typeof open>>) => Promise<unknown>][] = [
    ["a delete", (m) => opsOf(m.app).remove([2])],
    ["a move", (m) => opsOf(m.app).reorder([1, 0, 2], [1])],
    ["added pages", (m) => opsOf(m.app).insertBlank(1, 612, 792)],
  ];
  for (const [name, change] of steps) {
    it(`still has Undo of ${name} made before it load the document as it was then`, async () => {
      const m = await open();
      await change(m);
      await flush();
      await dismiss();
      await deleteForGood(m);
      expect(m.pages.pagesRestore).not.toHaveBeenCalled();
      keyOn(document.body, "z", { ctrlKey: true });
      await flush();
      expect(m.pages.pagesRestore).toHaveBeenCalledTimes(1);
      // The annotation is not brought back by a call of its own: it is in the copy that was loaded.
      expect(m.annotationCalls.restoreAnnotation).not.toHaveBeenCalled();
      // And what was taken back can be made again, as it was after the delete of the annotation.
      expect(activeOf(m.app).history.canRedo).toBe(true);
    });
  }

  it("does not load a copy when a turn or a crop made before it is taken back", async () => {
    const m = await open();
    await opsOf(m.app).rotate([0], 1);
    await flush();
    await dismiss();
    await opsOf(m.app).crop([1], [10, 10, 10, 10]);
    await flush();
    await dismiss();
    await deleteForGood(m);
    for (let i = 0; i < 2; i++) {
      keyOn(document.body, "z", { ctrlKey: true });
      await flush();
    }
    expect(m.pages.pagesRestore).not.toHaveBeenCalled();
    expect(m.pages.pagesRotate).toHaveBeenCalledTimes(2);
    expect(m.pages.pagesSetBoxes).toHaveBeenCalledTimes(1);
    expect(m.annotationCalls.restoreAnnotation).not.toHaveBeenCalled();
    // Taking them back left the annotation deleted: the only call about it is the delete.
    expect(m.annotationCalls.removeAnnotation).toHaveBeenCalledTimes(1);
  });
});

describe("while a page tool works on the document", () => {
  it("holds the annotation tools and the fields, and says why", async () => {
    const pages = fakePages();
    const slow = slowDelete(pages);
    const { app, api, annotationCalls } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    const annotator = activeOf(app).annotator;
    expect(annotator.isHeld).toBe(false);

    const deleting = opsOf(app).remove([1]);
    await flush();
    expect(opsOf(app).busyOn(1)).toBe(true);
    expect(annotator.isHeld).toBe(true);
    // The bar is told, and it says why a tool does nothing.
    expect(annotator.blockedReason).toContain("pages of this document are being changed");

    // A tool is not picked, and a drawing, a move or a note is not begun.
    annotator.setTool("pen");
    expect(annotator.tool).toBe("select");
    expect(status()).toContain("pages of this document are being changed");
    annotator.onStroke(0, [
      [1, 1],
      [30, 30],
    ]);
    await annotator.moveBy(0, 5, 3, 3);
    await annotator.idle();
    expect(annotationCalls.addInk).not.toHaveBeenCalled();
    expect(annotationCalls.updateAnnotation).not.toHaveBeenCalled();

    // The fields take nothing, and the bar over the pages says why. A box typed in anyway sends
    // nothing and shows what the document has.
    expect(document.querySelector(".form-layer")).not.toBeNull();
    for (const layer of document.querySelectorAll<HTMLElement>(".form-layer")) expect(layer.inert).toBe(true);
    expect(document.querySelector(".form-bar-text")?.textContent).toContain("pages of this document are being changed");
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    expect(api.setFormField).not.toHaveBeenCalled();
    expect(input.value).toBe("");

    slow.finish();
    await deleting;
    await flush();

    // Done: the tools and the fields work again, on the two pages that are left.
    expect(annotator.isHeld).toBe(false);
    expect(annotator.blockedReason).toBeNull();
    annotator.setTool("pen");
    expect(annotator.tool).toBe("pen");
    for (const layer of document.querySelectorAll<HTMLElement>(".form-layer")) expect(layer.inert).toBe(false);
    expect(document.querySelector<HTMLElement>(".form-bar")?.hidden).toBe(true);
    const again = field(0) as HTMLInputElement;
    again.value = "Ada";
    change(again);
    await flush();
    expect(api.setFormField).toHaveBeenCalledTimes(1);
  });

  it("holds them also while a step back of a page change works", async () => {
    const pages = fakePages();
    const { app, api } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    await opsOf(app).remove([1]);
    await flush();
    await dismiss();
    let finish!: (c: PagesChanged) => void;
    pages.pagesRestore.mockImplementationOnce(() => new Promise<PagesChanged>((r) => (finish = r)));
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    const annotator = activeOf(app).annotator;
    expect(opsOf(app).busyOn(1)).toBe(true);
    expect(annotator.isHeld).toBe(true);
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    expect(api.setFormField).not.toHaveBeenCalled();
    finish(changed(3, 107, 0));
    await flush();
    expect(annotator.isHeld).toBe(false);
  });
});

describe("before a page tool begins", () => {
  it("sends the text typed in a field that was not left, so that it is part of the document first", async () => {
    const pages = fakePages();
    const { app, api } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await opsOf(app).remove([1]);
    await flush();
    expect(api.setFormField).toHaveBeenCalledWith(1, 0, 0, "name", { kind: "text", text: "Ada" });
    expect(api.setFormField.mock.invocationCallOrder[0]).toBeLessThan(pages.pagesDelete.mock.invocationCallOrder[0]);
  });

  it("saves the open box of a note, and waits for the changes under way", async () => {
    const pages = fakePages();
    const { app, annotationCalls } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    const annotator = activeOf(app).annotator;
    annotator.setTool("note");
    annotator.onPlace(0, 100, 100, { x: 10, y: 10 });
    const box = document.querySelector<HTMLTextAreaElement>(".annot-editor-text");
    expect(box).not.toBeNull();
    (box as HTMLTextAreaElement).value = "A note";
    (box as HTMLTextAreaElement).dispatchEvent(new Event("input", { bubbles: true }));
    await opsOf(app).remove([1]);
    await flush();
    expect(annotationCalls.addNote).toHaveBeenCalledTimes(1);
    expect(annotationCalls.addNote.mock.invocationCallOrder[0]).toBeLessThan(pages.pagesDelete.mock.invocationCallOrder[0]);
    expect(document.querySelector(".annot-editor")).toBeNull();
  });

  it("does not begin while a field holds text the document refused, and says so", async () => {
    const pages = fakePages();
    const { app, api } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    api.setFormField.mockRejectedValueOnce({ code: "invalid_request", message: "too long" });
    const input = field(0) as HTMLInputElement;
    input.value = "x".repeat(40);
    change(input);
    await flush();
    const trying = opsOf(app).remove([1]);
    await flush();
    // The question is on the screen while nothing has begun.
    expect(document.querySelector(".notice")?.textContent).toContain("The pages of");
    expect(document.querySelector(".notice")?.textContent).toContain("were not changed");
    expect(pages.pagesDelete).not.toHaveBeenCalled();
    await dismiss();
    expect(await trying).toBeNull();
    expect(pages.pagesDelete).not.toHaveBeenCalled();
    // Nothing stays held.
    expect(activeOf(app).annotator.isHeld).toBe(false);
    expect(opsOf(app).busy).toBe(false);
  });
});

describe("after the pages changed", () => {
  it("makes the layers of the fields and of the annotations again for the new list of pages", async () => {
    const pages = fakePages();
    const { app, api, annotationCalls } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    const oldLayer = document.querySelector(".form-layer");
    expect(oldLayer).not.toBeNull();
    await opsOf(app).remove([2]);
    await flush();
    // New layers, fed from the engine again; the old ones are gone.
    expect(document.contains(oldLayer)).toBe(false);
    expect(document.querySelectorAll(".form-layer").length).toBeGreaterThan(0);
    expect(activeOf(app).view.pageCount).toBe(2);
    expect(api.formInfo).toHaveBeenCalledTimes(2);
    // No page that is gone is asked for after the change.
    const lastLists = annotationCalls.listAnnotations.mock.calls.slice(-3);
    for (const call of lastLists) expect(call[1]).toBeLessThan(2);
    for (const call of api.getFormFields.mock.calls.slice(-3)) expect(call[1]).toBeLessThan(2);
    expect(field(0)).not.toBeNull();
  });

  it("forgets the annotation that was picked: its page and id mean something else now", async () => {
    const pages = fakePages();
    const { app } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    const annotator = activeOf(app).annotator;
    annotator.selected = { page: 1, id: 9 };
    await opsOf(app).remove([0]);
    await flush();
    expect(annotator.selected).toBeNull();
  });
});

describe("Undo and Redo over page changes, annotations and form fields", () => {
  it("take them back newest first, and make them again oldest first, whichever kind they are", async () => {
    const pages = fakePages();
    const { app, api, annotationCalls } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    // A field, then a drawing, then the deletion of a page, then a check box.
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    const annotator = activeOf(app).annotator;
    annotator.onStroke(0, [
      [1, 1],
      [30, 30],
    ]);
    await annotator.idle();
    await opsOf(app).remove([2]);
    await flush();
    await dismiss();
    const box = field(1);
    box.focus();
    box.click();
    await flush();
    const order: string[] = [];
    const answer = api.setFormField.getMockImplementation() as (...args: Parameters<typeof api.setFormField>) => Promise<FormFieldInfo[]>;
    api.setFormField.mockImplementation((...args) => {
      order.push(`field ${args[3]} ${JSON.stringify(args[4])}`);
      return answer(...args);
    });
    annotationCalls.removeAnnotation.mockImplementation(() => {
      order.push("remove annotation");
      return Promise.resolve();
    });
    annotationCalls.restoreAnnotation.mockImplementation((_d: number, _p: number, id: number) => {
      order.push("restore annotation");
      return Promise.resolve({
        id,
        kind: "ink" as const,
        can: { editText: false, recolour: false, move: false, remove: true, restore: true, locked: false },
        rect: [10, 10, 40, 40] as [number, number, number, number],
        color: [0, 0, 255, 255] as [number, number, number, number],
        text: "",
        author: "",
      });
    });
    pages.pagesRestore.mockImplementation((_d: number, snapshot: number) => {
      order.push(`pages ${snapshot}`);
      return Promise.resolve(changed(snapshot === 7 ? 3 : 2, snapshot + 100, 0));
    });

    for (let i = 0; i < 4; i++) {
      keyOn(document.body, "z", { ctrlKey: true });
      await flush();
    }
    expect(order).toEqual([
      'field agree {"kind":"checked","on":false}',
      "pages 7",
      "remove annotation",
      'field name {"kind":"text","text":""}',
    ]);
    order.length = 0;
    for (let i = 0; i < 4; i++) {
      keyOn(document.body, "y", { ctrlKey: true });
      await flush();
    }
    expect(order).toEqual([
      'field name {"kind":"text","text":"Ada"}',
      "restore annotation",
      "pages 107",
      'field agree {"kind":"checked","on":true}',
    ]);
  });

  it("shows the result of taking back a field on the boxes that are there after the pages changed twice", async () => {
    const pages = fakePages();
    const { app } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    await opsOf(app).remove([2]);
    await flush();
    await dismiss();
    // The boxes of the first layer are gone; the layer that took over shows the field.
    expect((field(0) as HTMLInputElement).value).toBe("Ada");
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    expect((field(0) as HTMLInputElement).value).toBe("");
    keyOn(document.body, "y", { ctrlKey: true });
    await flush();
    keyOn(document.body, "y", { ctrlKey: true });
    await flush();
    expect((field(0) as HTMLInputElement).value).toBe("Ada");
  });

  it("refuses with a plain message, and goes no further, when the copy that takes a page change back is gone", async () => {
    const pages = fakePages();
    const { app, api } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    await opsOf(app).remove([2]);
    await flush();
    await dismiss();
    pages.pagesRestore.mockRejectedValueOnce({ code: "snapshot_gone", message: "no copy" });
    api.setFormField.mockClear();
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    const text = [...notices()].map((n) => n.textContent).join(" ");
    expect(text).toContain("cannot be taken back any more");
    expect(text).toContain("The change before it in the history");
    expect(text).toContain("cannot be taken back");
    // The field before it was not touched, and the delete is still the next thing to take back.
    expect(api.setFormField).not.toHaveBeenCalled();
    expect(activeOf(app).history.canUndo).toBe(true);
  });

  it("does not take a page change back while a field holds text the document refused, and says so", async () => {
    const pages = fakePages();
    const { app, api } = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi });
    await app.openPath("/docs/form.pdf");
    await flush();
    await opsOf(app).remove([1]);
    await flush();
    await dismiss();
    api.setFormField.mockRejectedValueOnce({ code: "invalid_request", message: "too long" });
    const input = field(0) as HTMLInputElement;
    input.value = "x".repeat(40);
    change(input);
    await flush();
    await dismiss();
    const refused = () => (activeOf(app) as unknown as { view: { hasRefusedFormInput: boolean } }).view.hasRefusedFormInput;
    expect(refused()).toBe(true);

    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    await flush();
    // The same words as for a tool that does not begin; the document and the box are as they were.
    const text = [...notices()].map((n) => n.textContent).join(" ");
    expect(text).toContain("A field has text that could not be added");
    expect(text).toContain("were not changed");
    await dismiss();
    expect(pages.pagesRestore).not.toHaveBeenCalled();
    expect(refused()).toBe(true);
    expect((field(0) as HTMLInputElement).value).toBe("x".repeat(40));
    expect(activeOf(app).history.canUndo).toBe(true);
    expect(opsOf(app).busy).toBe(false);
    expect(activeOf(app).annotator.isHeld).toBe(false);

    // Once the text is put back (Escape in the box), the step is taken.
    keyOn(field(0), "Escape");
    await flush();
    expect(refused()).toBe(false);
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    await flush();
    expect(pages.pagesRestore).toHaveBeenCalledTimes(1);
  });

  it("says which way the history cannot go, in the words of the failure", () => {
    const gone = { code: "snapshot_gone", message: "" };
    expect(undoFailure(gone, "undo", "a.pdf", 0).detail).not.toContain("before it in the history");
    expect(undoFailure(gone, "undo", "a.pdf", 3).detail).toContain("3 changes before it in the history");
    expect(undoFailure(gone, "undo", "a.pdf", 3).detail).toContain("cannot be taken back");
    expect(undoFailure(gone, "redo", "a.pdf", 1).detail).toContain("The change before it in the history");
    expect(undoFailure(gone, "redo", "a.pdf", 1).detail).toContain("cannot be made again");
    expect(undoFailure({ code: "form_field_refused", message: "" }, "undo", "a.pdf", 2).detail).toContain("Nothing was lost");
  });
});

describe("the verdict on a signature, and edits that are not page changes", () => {
  const intact = {
    index: 0,
    state: "unknown_signer",
    reason: "no_trust_list",
    certificateTime: "valid",
    subFilter: "adbe.pkcs7.detached",
    byteRange: [0, 10, 20, 30],
    fileLen: 50,
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
  const signed = { total: 1, unlisted: 0, trustAnchors: 0, items: [intact] } as unknown as SignaturesInfo;
  const badge = () => document.querySelector<HTMLButtonElement>("#signature-badge") as HTMLButtonElement;
  const INTACT = "Signature intact; signer not verified";
  const STALE = "Not checked for this version";

  async function openSigned() {
    const pages = fakePages();
    const made = make({ withAnnotations: true, pages: pages as unknown as PageToolsApi, signatures: signed });
    await made.app.openPath("/docs/signed.pdf");
    await flush();
    expect(badge().textContent).toBe(INTACT);
    return { ...made, pages };
  }

  it("is dropped by an annotation, as it is by a page change", async () => {
    const m = await openSigned();
    const annotator = activeOf(m.app).annotator;
    annotator.onStroke(0, [
      [1, 1],
      [30, 30],
    ]);
    await annotator.idle();
    expect(m.annotationCalls.addInk).toHaveBeenCalledTimes(1);
    expect(badge().textContent).toBe(STALE);
  });

  it("is dropped by a change of a field", async () => {
    const m = await openSigned();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    expect(m.api.setFormField).toHaveBeenCalledTimes(1);
    expect(badge().textContent).toBe(STALE);
  });

  it("is dropped by a page change", async () => {
    const m = await openSigned();
    await opsOf(m.app).remove([1]);
    await flush();
    expect(badge().textContent).toBe(STALE);
  });

  it("is dropped by taking an annotation off for good, which is not an entry of the history", async () => {
    const m = await openSigned();
    m.annotationCalls.listAnnotations.mockResolvedValue([
      {
        id: 3,
        kind: "other",
        can: { editText: false, recolour: false, move: false, remove: true, restore: false, locked: false },
        rect: [10, 10, 40, 40],
        color: [0, 0, 0, 255],
        text: "",
        author: "",
      },
    ]);
    const view = activeOf(m.app).view;
    const annotator = activeOf(m.app).annotator;
    (m.app as unknown as { setAnnotating(open: boolean): void }).setAnnotating(true);
    view.invalidatePage(0);
    await flush();
    annotator.select(0, 3);
    const removing = annotator.removeSelected();
    await flush();
    document.querySelector<HTMLElement>(".notice .btn-primary")?.click();
    await removing;
    await flush();
    expect(m.annotationCalls.removeAnnotation).toHaveBeenCalledTimes(1);
    expect(badge().textContent).toBe(STALE);
  });

  it("is dropped by a save of a copy", async () => {
    const m = await openSigned();
    m.api.saveAs.mockResolvedValueOnce({ path: "/docs/copy.pdf", name: "copy.pdf" });
    keyOn(document.body, "S", { ctrlKey: true, shiftKey: true });
    await flush();
    expect(badge().textContent).toBe(STALE);
  });
});

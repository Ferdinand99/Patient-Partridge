// @vitest-environment jsdom
// Filling in a form through the whole window: the controls over the page, the mark that the
// document has changes that are not saved, undo and redo from the keyboard, and the notices.
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedDocument, SavedFile } from "../lib/api";
import type { Annotation } from "../lib/annotations";
import type { FormFieldInfo, FormInfo, FormValueIn } from "../viewer/forms";
import { XFA_NOTICE } from "../viewer/forms";
import { App } from "./app";

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

function make(options: { fields?: FormFieldInfo[]; info?: FormInfo; withForms?: boolean; withAnnotations?: boolean } = {}) {
  const state = structuredClone(options.fields ?? [textField(0, "name"), { ...textField(1, "agree"), kind: "checkbox" as const }]);
  const api = {
    open: vi.fn((path: string, _password?: string) =>
      Promise.resolve<OpenedDocument>({ doc: 1, name: path.split("/").pop() ?? "", pages: [letter, letter], position: null }),
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
  };
  const app = new App(document, deps);
  app.init();
  return { app, api, deps, state, annotationCalls };
}

const field = (index: number) => document.querySelector<HTMLElement>(`.form-field[data-form-index="${index}"]`) as HTMLElement;
const change = (el: HTMLElement) => el.dispatchEvent(new Event("change", { bubbles: true }));
const keyOn = (el: HTMLElement, key: string, init: KeyboardEventInit = {}) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }));

describe("filling in a form in the window", () => {
  it("shows the fields over the page, and a change makes the document dirty", async () => {
    const { app, api, deps } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    expect(document.querySelectorAll(".form-field")).toHaveLength(2);
    expect(api.setDirty).not.toHaveBeenCalled();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    expect(api.setFormField).toHaveBeenCalledWith(1, 0, 0, "name", { kind: "text", text: "Ada" });
    // The tab, the title and the app process all know that there is something to save.
    expect(api.setDirty).toHaveBeenLastCalledWith(1, true);
    expect(deps.setWindowTitle).toHaveBeenLastCalledWith("• form.pdf - Patient Partridge");
    expect(app.unsavedCount).toBe(1);
    // The page is drawn again so that the new value shows.
    const drawn = api.render.mock.calls.filter((c) => c[1] === 0).length;
    expect(drawn).toBeGreaterThanOrEqual(2);
  });

  it("asks the side panel to draw the thumbnail of the page again after a change, and after Undo", async () => {
    const { app } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    const sidebar = (app as unknown as { sidebar: { pageChanged(doc: number, page: number): void } }).sidebar;
    const changed = vi.spyOn(sidebar, "pageChanged");
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    expect(changed).toHaveBeenCalledWith(1, 0);
    changed.mockClear();
    keyOn(field(1), "z", { ctrlKey: true });
    await flush();
    expect(changed).toHaveBeenCalledWith(1, 0);
  });

  it("takes a change back with Ctrl+Z, makes it again with Ctrl+Y, also with a check box in focus", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    const box = field(1);
    box.focus();
    box.click();
    await flush();
    expect(field(1).getAttribute("aria-checked")).toBe("true");
    expect(api.setDirty).toHaveBeenLastCalledWith(1, true);
    keyOn(field(1), "z", { ctrlKey: true });
    await flush();
    expect(api.setFormField).toHaveBeenLastCalledWith(1, 0, 1, "agree", { kind: "checked", on: false });
    expect(field(1).getAttribute("aria-checked")).toBe("false");
    // Back where the file is: nothing to save.
    expect(api.setDirty).toHaveBeenLastCalledWith(1, false);
    keyOn(field(1), "y", { ctrlKey: true });
    await flush();
    expect(field(1).getAttribute("aria-checked")).toBe("true");
    expect(api.setDirty).toHaveBeenLastCalledWith(1, true);
  });

  it("leaves Ctrl+Z to the box while the reader is typing in a text field", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    api.setFormField.mockClear();
    input.focus();
    keyOn(input, "z", { ctrlKey: true });
    await flush();
    expect(api.setFormField).not.toHaveBeenCalled();
  });

  it("counts a saved document as clean, and the next change as a new one", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "one";
    change(input);
    await flush();
    expect(await app.saveActive()).toBe(true);
    expect(api.save).toHaveBeenCalled();
    expect(app.unsavedCount).toBe(0);
    input.value = "two";
    change(input);
    await flush();
    expect(app.unsavedCount).toBe(1);
  });

  it("does not mark the document when a change is refused", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    api.setFormField.mockRejectedValueOnce({ code: "form_field_refused", message: "no" });
    const input = field(0) as HTMLInputElement;
    input.value = "x";
    change(input);
    await flush();
    expect(app.unsavedCount).toBe(0);
    // Typing marked the tab as changed at once; the refusal took the mark away again.
    expect(api.setDirty.mock.calls.map((c) => c[1])).toEqual([true, false]);
  });

  it("tells the reader that an XFA form is not filled in, and marks nothing", async () => {
    const { app, api } = make({ info: { kind: "xfa", actions: false } });
    await app.openPath("/docs/form.pdf");
    await flush();
    expect(document.querySelector(".form-bar-text")?.textContent).toBe(XFA_NOTICE);
    expect(document.querySelectorAll(".form-field")).toHaveLength(0);
    expect(api.getFormFields).not.toHaveBeenCalled();
    expect(app.unsavedCount).toBe(0);
  });

  it("shows no fields when the app has no way to ask for them", async () => {
    const { app } = make({ withForms: false });
    await app.openPath("/docs/form.pdf");
    await flush();
    expect(document.querySelectorAll(".form-field, .form-bar")).toHaveLength(0);
  });
});

describe("text that is typed and not left", () => {
  /** Types in the first field without leaving it: the text is in the box, not in the document. */
  async function typing(options: Parameters<typeof make>[0] = {}) {
    const m = make(options);
    await m.app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.focus();
    input.value = "typed";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    return { ...m, input };
  }

  it("counts the tab as changed from the first key, in the tab, the title and the app process", async () => {
    const { app, api, deps } = await typing();
    await flush();
    expect(api.setFormField).not.toHaveBeenCalled();
    expect(app.unsavedCount).toBe(1);
    expect(api.setDirty).toHaveBeenLastCalledWith(1, true);
    expect(deps.setWindowTitle).toHaveBeenLastCalledWith("• form.pdf - Patient Partridge");
    expect(document.querySelectorAll(".tab-dirty")).toHaveLength(1);
  });

  it("is sent before a save, so the save is not skipped and the value is in the document", async () => {
    const { app, api, state } = await typing();
    expect(await app.saveActive()).toBe(true);
    expect(api.setFormField).toHaveBeenCalledWith(1, 0, 0, "name", { kind: "text", text: "typed" });
    expect(state[0].value).toBe("typed");
    expect(api.save).toHaveBeenCalledTimes(1);
    // The order: the value first, then the save.
    expect(api.setFormField.mock.invocationCallOrder[0]).toBeLessThan(api.save.mock.invocationCallOrder[0]);
    expect(app.unsavedCount).toBe(0);
  });

  it("is sent before Save as", async () => {
    const { app, api } = await typing();
    await app.saveActiveAs();
    expect(api.setFormField.mock.invocationCallOrder[0]).toBeLessThan(api.saveAs.mock.invocationCallOrder[0]);
  });

  it("is sent before a tab closes, and the tab then asks whether to save", async () => {
    const { app, api } = await typing();
    const closing = app.requestClose(1 as number);
    await flush();
    expect(api.setFormField).toHaveBeenCalledWith(1, 0, 0, "name", { kind: "text", text: "typed" });
    // The question is on the screen: the tab did not close.
    expect(document.querySelector(".notice")?.textContent).toContain("Save changes to");
    expect(app.tabCount).toBe(1);
    (document.querySelector(".notice button[type=button], .notice button") as HTMLElement | null)?.click();
    await closing.catch(() => undefined);
  });

  it("is sent before the app quits, and quitting then asks", async () => {
    const { app, api } = await typing();
    void app.requestQuit();
    await flush();
    expect(api.setFormField).toHaveBeenCalled();
    expect(api.quitApp).not.toHaveBeenCalled();
    expect(document.querySelector(".notice")?.textContent).toContain("Save changes to");
  });

  it("counts for an update: the app says a document has changes that are not saved", async () => {
    const { app } = await typing();
    expect(app.unsavedCount).toBe(1);
    await app.saveAllChanges();
    expect(app.unsavedCount).toBe(0);
  });

  it("is not lost when the page is scrolled out of the mounted range: it is sent first", async () => {
    const { app, api, input } = await typing();
    // The tab goes behind another one: its pages are given up.
    await app.openPath("/docs/second.pdf");
    await flush();
    expect(api.setFormField).toHaveBeenCalledWith(1, 0, 0, "name", { kind: "text", text: "typed" });
    expect(input.isConnected).toBe(false);
  });

  it("is sent when the box loses the focus and no change event comes (WebKit, when the window loses the focus)", async () => {
    const { app, api, input } = await typing();
    input.dispatchEvent(new Event("blur"));
    await flush();
    expect(api.setFormField).toHaveBeenCalledTimes(1);
    expect(api.setFormField).toHaveBeenCalledWith(1, 0, 0, "name", { kind: "text", text: "typed" });
    // The text is still in the box, and the tab is changed as before.
    expect(input.value).toBe("typed");
    expect(app.unsavedCount).toBe(1);
    // The change event that WebKit sends when the focus comes back does not send it again.
    input.dispatchEvent(new Event("change", { bubbles: true }));
    await flush();
    expect(api.setFormField).toHaveBeenCalledTimes(1);
  });

  it("no longer counts once the reader puts the old text back", async () => {
    const { app, input } = await typing();
    input.value = "";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(app.unsavedCount).toBe(0);
  });
});

describe("undo from a drop-down list", () => {
  it("takes the choice back with Ctrl+Z while the list still has the focus", async () => {
    const combo: FormFieldInfo = {
      ...textField(0, "country"),
      kind: "combo",
      options: [
        { selected: true, label: "Norway" },
        { selected: false, label: "Sweden" },
      ],
    };
    const { app, api } = make({ fields: [combo] });
    await app.openPath("/docs/form.pdf");
    await flush();
    const select = field(0) as HTMLSelectElement;
    select.focus();
    select.value = "1";
    change(select);
    await flush();
    expect(api.setFormField).toHaveBeenLastCalledWith(1, 0, 0, "country", { kind: "choice", picked: [1] });
    keyOn(select, "z", { ctrlKey: true });
    await flush();
    expect(api.setFormField).toHaveBeenLastCalledWith(1, 0, 0, "country", { kind: "choice", picked: [0] });
  });
});

describe("a form that is filled in and annotated", () => {
  it("is saved with both kinds of change by one save, and Undo and Redo take them in the order they were made", async () => {
    const { app, api, annotationCalls } = make({ withAnnotations: true });
    await app.openPath("/docs/form.pdf");
    await flush();
    // A field first, then a drawing, then a check box.
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    const annotator = (app as unknown as { active: { annotator: { onStroke(page: number, points: [number, number][]): void; idle(): Promise<void> } } }).active
      .annotator;
    annotator.onStroke(0, [
      [1, 1],
      [30, 30],
    ]);
    await annotator.idle();
    expect(annotationCalls.addInk).toHaveBeenCalledTimes(1);
    const box = field(1);
    box.focus();
    box.click();
    await flush();
    expect(app.unsavedCount).toBe(1);

    // One save writes the document with all three, and leaves nothing to save.
    expect(await app.saveActive()).toBe(true);
    expect(api.save).toHaveBeenCalledTimes(1);
    expect(app.unsavedCount).toBe(0);
    expect(api.setDirty).toHaveBeenLastCalledWith(1, false);

    // Undo goes back over them, newest first, also after the save: the check box, the drawing, the
    // field. Each makes the document differ from the saved file.
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    expect(api.setFormField).toHaveBeenLastCalledWith(1, 0, 1, "agree", { kind: "checked", on: false });
    expect(app.unsavedCount).toBe(1);
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    expect(annotationCalls.removeAnnotation).toHaveBeenCalledTimes(1);
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    expect(api.setFormField).toHaveBeenLastCalledWith(1, 0, 0, "name", { kind: "text", text: "" });
    // Redo puts them back in the order they were made.
    keyOn(document.body, "y", { ctrlKey: true });
    await flush();
    expect(api.setFormField).toHaveBeenLastCalledWith(1, 0, 0, "name", { kind: "text", text: "Ada" });
    keyOn(document.body, "y", { ctrlKey: true });
    await flush();
    expect(annotationCalls.restoreAnnotation).toHaveBeenCalledTimes(1);
    keyOn(document.body, "y", { ctrlKey: true });
    await flush();
    expect(api.setFormField).toHaveBeenLastCalledWith(1, 0, 1, "agree", { kind: "checked", on: true });
    // Back at the state of the save.
    expect(app.unsavedCount).toBe(0);
    expect(api.setDirty).toHaveBeenLastCalledWith(1, false);
    // The form and the annotation boxes are both still on the page.
    expect(document.querySelectorAll(".form-field")).toHaveLength(2);
  });
});

describe("Undo while a form change is on its way", () => {
  it("waits for that change and then takes it back, not the one before it", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    // The engine is slow to answer the change.
    const answer = api.setFormField.getMockImplementation() as (...args: Parameters<typeof api.setFormField>) => Promise<FormFieldInfo[]>;
    let release = () => {};
    const held = new Promise<void>((resolve) => (release = resolve));
    api.setFormField.mockImplementationOnce(async (...args) => {
      await held;
      return answer(...args);
    });
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    expect(api.setFormField).toHaveBeenCalledTimes(1);

    // Undo is pressed before the answer: it does not run ahead of the change.
    keyOn(document.body, "z", { ctrlKey: true });
    await flush();
    expect(api.setFormField).toHaveBeenCalledTimes(1);

    release();
    await flush();
    // The change was made and then taken back: the field is as it was, and nothing is left to save.
    expect(api.setFormField).toHaveBeenCalledTimes(2);
    expect(api.setFormField).toHaveBeenLastCalledWith(1, 0, 0, "name", { kind: "text", text: "" });
    expect(app.unsavedCount).toBe(0);
  });
});

describe("Undo of a form change that cannot be taken back", () => {
  const radio = (index: number): FormFieldInfo => ({ ...textField(index, "pick"), kind: "radio", rect: [100, 100 + 30 * index, 120, 120 + 30 * index] });

  for (const withAnnotations of [false, true]) {
    it(`says the document is as it was, and leaves it saved (${withAnnotations ? "with" : "without"} the annotator)`, async () => {
      const { app, api } = make({ fields: [radio(0), radio(1)], withAnnotations });
      await app.openPath("/docs/form.pdf");
      await flush();
      // The first choice of a group: nothing was chosen before, and a radio button cannot be turned
      // off, so the change cannot be taken back.
      const first = field(0);
      first.focus();
      first.click();
      await flush();
      expect(app.unsavedCount).toBe(1);
      expect(await app.saveActive()).toBe(true);
      expect(app.unsavedCount).toBe(0);
      api.setFormField.mockClear();

      keyOn(document.body, "z", { ctrlKey: true });
      await flush();
      // The engine was not asked, and nothing about the document changed.
      expect(api.setFormField).not.toHaveBeenCalled();
      expect(app.unsavedCount).toBe(0);
      expect(api.setDirty).toHaveBeenLastCalledWith(1, false);
      const text = document.body.textContent ?? "";
      expect(text).toContain("The change could not be taken back");
      expect(text).toContain("The document is as it was before. Nothing was lost.");
      expect(text).not.toContain("Part of the change may have been made");
    });
  }
});

const noticeText = () => document.querySelector(".notice")?.textContent ?? "";
const pressButton = (label: string) => {
  const found = [...document.querySelectorAll<HTMLElement>(".notice button")].find((b) => b.textContent === label);
  if (!found) throw new Error(`no button "${label}"`);
  found.click();
};

describe("text that the engine refuses because it is too long", () => {
  const LONG = "x".repeat(9000);

  /** The reader pastes more than the document takes into the first field, as the client refuses it. */
  async function pasted() {
    const m = make();
    const real = m.api.setFormField.getMockImplementation() as (...args: unknown[]) => Promise<FormFieldInfo[]>;
    m.api.setFormField.mockImplementation((...args: Parameters<typeof m.api.setFormField>) => {
      const value = args[4];
      if (value.kind === "text" && new TextEncoder().encode(value.text).length > 8192) {
        return Promise.reject({ code: "invalid_request", message: "the name or the value is too long" });
      }
      return real(...args);
    });
    await m.app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.focus();
    input.value = LONG;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    return { ...m, input };
  }

  it("keeps the text in the box and the tab marked as changed when the box is left", async () => {
    const { app, api, input } = await pasted();
    input.dispatchEvent(new Event("blur"));
    await flush();
    expect(input.value).toBe(LONG);
    expect(app.unsavedCount).toBe(1);
    expect(api.setDirty).toHaveBeenLastCalledWith(1, true);
    expect(document.querySelector(".form-bar-text")?.textContent).toContain("too long");
    expect(document.querySelectorAll(".tab-dirty")).toHaveLength(1);
  });

  it("asks at quit instead of quitting", async () => {
    const { app, api, input } = await pasted();
    void app.requestQuit();
    await flush();
    expect(api.quitApp).not.toHaveBeenCalled();
    expect(noticeText()).toContain("Save changes to");
    expect(noticeText()).toContain("did not accept");
    expect(input.value).toBe(LONG);
    expect(api.save).not.toHaveBeenCalled();
  });

  it("quits without saving only when the reader says so", async () => {
    const { app, api } = await pasted();
    void app.requestQuit();
    await flush();
    pressButton("Quit without saving");
    await flush();
    expect(api.quitApp).toHaveBeenCalledTimes(1);
    expect(api.save).not.toHaveBeenCalled();
  });

  it("does not quit after Save and quit: the save says why it did not happen, and the program stays open", async () => {
    const { app, api, input } = await pasted();
    void app.requestQuit();
    await flush();
    pressButton("Save and quit");
    await flush();
    expect(noticeText()).toContain("was not saved");
    pressButton("OK");
    await flush();
    expect(api.save).not.toHaveBeenCalled();
    expect(api.quitApp).not.toHaveBeenCalled();
    expect(api.stayOpen).toHaveBeenCalled();
    expect(app.unsavedCount).toBe(1);
    expect(input.value).toBe(LONG);
  });

  it("asks at Ctrl+W instead of closing the tab", async () => {
    const { app, api, input } = await pasted();
    keyOn(input, "w", { ctrlKey: true });
    await flush();
    expect(app.tabCount).toBe(1);
    expect(noticeText()).toContain("Save changes to");
    expect(noticeText()).toContain("did not accept");
    expect(input.value).toBe(LONG);
    expect(api.save).not.toHaveBeenCalled();
    pressButton("Cancel");
    await flush();
    expect(app.tabCount).toBe(1);
    expect(app.unsavedCount).toBe(1);
  });

  it("does not close the tab after Save in that question", async () => {
    const { app, api } = await pasted();
    keyOn(field(0), "w", { ctrlKey: true });
    await flush();
    pressButton("Save");
    await flush();
    expect(noticeText()).toContain("was not saved");
    pressButton("OK");
    await flush();
    expect(app.tabCount).toBe(1);
    expect(api.save).not.toHaveBeenCalled();
  });

  it("stops Ctrl+S: nothing is written, the dot stays, and the box keeps the text", async () => {
    const { app, api, input } = await pasted();
    // Another change is in the document too, so a save would have something to write.
    field(1).click();
    await flush();
    keyOn(input, "s", { ctrlKey: true });
    await flush();
    expect(api.save).not.toHaveBeenCalled();
    expect(noticeText()).toContain("was not saved");
    expect(noticeText()).toContain("A field has text that could not be added");
    expect(app.unsavedCount).toBe(1);
    expect(input.value).toBe(LONG);
    expect(document.querySelectorAll(".tab-dirty")).toHaveLength(1);
  });

  it("stops Save a copy as well", async () => {
    const { app, api } = await pasted();
    void app.saveActiveAs();
    await flush();
    expect(api.saveAs).not.toHaveBeenCalled();
    expect(noticeText()).toContain("was not saved");
  });

  it("lets the reader save once the text is shortened", async () => {
    const { app, api, input } = await pasted();
    keyOn(input, "s", { ctrlKey: true });
    await flush();
    pressButton("OK");
    input.value = "short";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(app.unsavedCount).toBe(1);
    expect(await app.saveActive()).toBe(true);
    expect(api.save).toHaveBeenCalledTimes(1);
    expect(app.unsavedCount).toBe(0);
  });

  it("is clean again when the reader clears the field", async () => {
    const { app, api, input } = await pasted();
    input.dispatchEvent(new Event("blur"));
    await flush();
    expect(app.unsavedCount).toBe(1);
    input.value = "";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(app.unsavedCount).toBe(0);
    expect(api.setDirty).toHaveBeenLastCalledWith(1, false);
  });

  it("is clean again after Escape, which puts the old text back", async () => {
    const { app, input } = await pasted();
    input.dispatchEvent(new Event("blur"));
    await flush();
    keyOn(input, "Escape");
    expect(input.value).toBe("");
    expect(app.unsavedCount).toBe(0);
  });

  it("is still counted after the page scrolls out of the mounted range", async () => {
    const { app, input } = await pasted();
    input.dispatchEvent(new Event("blur"));
    await flush();
    await app.openPath("/docs/second.pdf");
    await flush();
    expect(input.isConnected).toBe(false);
    expect(app.unsavedCount).toBe(1);
  });
});

describe("text that the engine refuses: finding the field again", () => {
  const LONG = "x".repeat(9000);

  async function refusedBox() {
    const m = make({ fields: [textField(0, "comments"), textField(1, "other")] });
    const real = m.api.setFormField.getMockImplementation() as (...args: unknown[]) => Promise<FormFieldInfo[]>;
    m.api.setFormField.mockImplementation((...args: Parameters<typeof m.api.setFormField>) => {
      const value = args[4];
      if (value.kind === "text" && new TextEncoder().encode(value.text).length > 8192) {
        return Promise.reject({ code: "invalid_request", message: "the name or the value is too long" });
      }
      return real(...args);
    });
    await m.app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.focus();
    input.value = LONG;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.blur();
    await flush();
    return { ...m, input };
  }

  beforeEach(() => {
    HTMLElement.prototype.scrollIntoView = vi.fn();
  });
  afterEach(() => {
    delete (HTMLElement.prototype as { scrollIntoView?: unknown }).scrollIntoView;
  });

  it("marks the box: its text shows, and it is flagged as invalid with a reason", async () => {
    const { input } = await refusedBox();
    expect(input.classList.contains("form-refused")).toBe(true);
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(input.title).toContain("too long");
    expect(field(1).classList.contains("form-refused")).toBe(false);
    expect(field(1).hasAttribute("aria-invalid")).toBe(false);
  });

  it("takes the mark away when the reader types again, and when Escape puts the old text back", async () => {
    const { input } = await refusedBox();
    input.focus();
    input.value = "short";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(input.classList.contains("form-refused")).toBe(false);
    expect(input.hasAttribute("aria-invalid")).toBe(false);
    expect(input.hasAttribute("title")).toBe(false);
    input.value = LONG;
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.blur();
    await flush();
    expect(input.classList.contains("form-refused")).toBe(true);
    keyOn(input, "Escape");
    expect(input.classList.contains("form-refused")).toBe(false);
    expect(input.hasAttribute("aria-invalid")).toBe(false);
  });

  it("keeps the mark and the text when the fields of the page change shape", async () => {
    const { app, api } = await refusedBox();
    api.getFormFields.mockImplementation((_d: number, page: number) =>
      Promise.resolve(page === 0 ? [textField(0, "comments"), textField(1, "other"), textField(2, "extra")] : []),
    );
    field(1).focus();
    (field(1) as HTMLInputElement).value = "b";
    change(field(1));
    await flush();
    expect(app.unsavedCount).toBe(1);
    expect(field(0).classList.contains("form-refused")).toBe(true);
    expect((field(0) as HTMLInputElement).value).toBe(LONG);
  });

  it("names the field and the page when a save is refused, and does not write", async () => {
    const { app, api } = await refusedBox();
    void app.saveActive();
    await flush();
    expect(noticeText()).toContain('The field "comments" on page 1');
    expect(noticeText()).toContain("Escape");
    expect(api.save).not.toHaveBeenCalled();
  });

  it("says how many other fields are in the same state", async () => {
    const { app } = await refusedBox();
    const second = field(1) as HTMLInputElement;
    second.focus();
    second.value = LONG;
    second.dispatchEvent(new Event("input", { bubbles: true }));
    second.blur();
    await flush();
    void app.saveActive();
    await flush();
    expect(noticeText()).toContain("One other field has the same problem.");
  });

  it("takes the reader to the field, and puts the focus in it", async () => {
    const { app, input } = await refusedBox();
    expect(document.activeElement).not.toBe(input);
    void app.saveActive();
    await flush();
    pressButton("Go to the field");
    await flush();
    expect(document.activeElement).toBe(input);
    expect(HTMLElement.prototype.scrollIntoView).toHaveBeenCalled();
    expect(input.value).toBe(LONG);
  });

  it("leaves the focus where it is with OK", async () => {
    const { app, input } = await refusedBox();
    void app.saveActive();
    await flush();
    pressButton("OK");
    await flush();
    expect(document.activeElement).not.toBe(input);
  });

  it("keeps the focus in the field when the toolbar button was pressed", async () => {
    const { input } = await refusedBox();
    (document.getElementById("save-button") as HTMLButtonElement).click();
    await flush();
    pressButton("Go to the field");
    await flush();
    expect(document.activeElement).toBe(input);
  });
});

describe("a second press while a save is being answered", () => {
  const saveButton = () => document.getElementById("save-button") as HTMLButtonElement;
  const copyButton = () => document.getElementById("save-copy-button") as HTMLButtonElement;

  async function dirty() {
    const m = make();
    await m.app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    return m;
  }

  it("opens one dialog for a double click on Save a copy, also after it was cancelled", async () => {
    const { api } = await dirty();
    let answer: (v: SavedFile | null) => void = () => undefined;
    api.saveAs.mockImplementationOnce(() => new Promise((r) => (answer = r)));
    copyButton().click();
    copyButton().click();
    await flush();
    expect(api.saveAs).toHaveBeenCalledTimes(1);
    answer(null);
    await flush();
    expect(api.saveAs).toHaveBeenCalledTimes(1);
    // Once it is over, the next press works.
    copyButton().click();
    await flush();
    expect(api.saveAs).toHaveBeenCalledTimes(2);
  });

  it("asks the error of a double click on Save once", async () => {
    const { api } = await dirty();
    api.save.mockRejectedValue({ code: "save_disk_full", message: "full" });
    saveButton().click();
    saveButton().click();
    await flush();
    expect(api.save).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll(".notice")).toHaveLength(1);
    pressButton("OK");
    await flush();
    expect(api.save).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll(".notice")).toHaveLength(0);
  });

  it("ignores a repeated Ctrl+S while the first one waits for an answer", async () => {
    const { api } = await dirty();
    api.save.mockRejectedValue({ code: "save_signed", message: "signed" });
    const ctrlS = () =>
      document.body.dispatchEvent(
        new KeyboardEvent("keydown", { key: "s", code: "KeyS", ctrlKey: true, bubbles: true, cancelable: true }),
      );
    ctrlS();
    await flush();
    ctrlS();
    await flush();
    expect(api.save).toHaveBeenCalledTimes(1);
    expect(document.querySelectorAll(".notice")).toHaveLength(1);
    pressButton("Cancel");
    await flush();
    expect(document.querySelectorAll(".notice")).toHaveLength(0);
  });

  it("does not take the focus from a notice that is open after Save a copy", async () => {
    const { api } = await dirty();
    api.saveAs.mockResolvedValueOnce({ path: "/docs/copy.pdf", name: "copy.pdf", touched: true });
    copyButton().click();
    await flush();
    expect(document.querySelector(".notice")).not.toBeNull();
    expect(document.activeElement?.classList.contains("viewer")).toBe(false);
  });
});

describe("Ctrl+P with a lost engine", () => {
  it("does not open the print dialog when the flush ends the tab", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    api.setFormField.mockRejectedValue({ code: "engine_stopped", message: "it exited" });
    const input = field(0) as HTMLInputElement;
    input.focus();
    input.value = "Ada";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", { key: "p", code: "KeyP", ctrlKey: true, bubbles: true, cancelable: true }),
    );
    await flush();
    expect(app.tabCount).toBe(0);
    expect(document.querySelector(".print-dialog")).toBeNull();
    expect(document.querySelectorAll(".notice")).toHaveLength(1);
    expect(noticeText()).toContain("The PDF engine stopped");
  });

  it("can print again afterwards in a new tab", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    api.setFormField.mockRejectedValueOnce({ code: "engine_stopped", message: "it exited" });
    const input = field(0) as HTMLInputElement;
    input.focus();
    input.value = "Ada";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    void app.printActive();
    await flush();
    pressButton("Open the file again");
    await flush();
    void app.printActive();
    await flush();
    expect(document.querySelector(".print-dialog")).not.toBeNull();
  });
});

describe("a form change that loses the engine", () => {
  it("ends the tab the way a lost engine does, and says that the changes are lost", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    api.setFormField.mockRejectedValueOnce({ code: "engine_stopped", message: "it exited" });
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    expect(app.tabCount).toBe(0);
    expect(app.currentView).toBeNull();
    expect(noticeText()).toContain("The PDF engine stopped");
    expect(noticeText()).toContain("The changes that were not saved are lost.");
    pressButton("Open the file again");
    await flush();
    expect(api.open).toHaveBeenCalledTimes(2);
    expect(app.tabCount).toBe(1);
  });

  it("also ends the tab when a check box change loses the engine", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    api.setFormField.mockRejectedValueOnce({ code: "unknown_document", message: "gone" });
    field(1).click();
    await flush();
    expect(app.tabCount).toBe(0);
    expect(noticeText()).toContain("The PDF engine was restarted");
  });

  it("tells the reader only once when several changes are queued behind the lost one", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    api.setFormField.mockRejectedValue({ code: "engine_stopped", message: "it exited" });
    field(1).click();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    expect(app.tabCount).toBe(0);
    expect(document.querySelectorAll(".notice")).toHaveLength(1);
  });
});

describe("Save and Save a copy in the toolbar", () => {
  const saveButton = () => document.getElementById("save-button") as HTMLButtonElement;
  const copyButton = () => document.getElementById("save-copy-button") as HTMLButtonElement;

  it("are both off with no document open", () => {
    make();
    expect(saveButton().disabled).toBe(true);
    expect(copyButton().disabled).toBe(true);
  });

  it("offers Save a copy for a document without changes, and Save only once it has some", async () => {
    const { app } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    expect(saveButton().disabled).toBe(true);
    expect(copyButton().disabled).toBe(false);
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    expect(saveButton().disabled).toBe(false);
  });

  it("turns Save on from the first key typed in a field", async () => {
    const { app } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.focus();
    input.value = "A";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(saveButton().disabled).toBe(false);
  });

  it("saves the document and turns Save off again", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    saveButton().click();
    await flush();
    expect(api.save).toHaveBeenCalledTimes(1);
    expect(app.unsavedCount).toBe(0);
    expect(saveButton().disabled).toBe(true);
  });

  it("asks for a new name with Save a copy", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    copyButton().click();
    await flush();
    expect(api.saveAs).toHaveBeenCalledTimes(1);
    expect(app.tabCount).toBe(1);
  });

  it("have a name and a key in their label for a screen reader and a tooltip", () => {
    make();
    expect(saveButton().getAttribute("aria-label")).toBe("Save");
    expect(saveButton().title).toBe("Save (Ctrl+S)");
    expect(copyButton().getAttribute("aria-label")).toBe("Save a copy");
    expect(copyButton().title).toContain("Ctrl+Shift+S");
  });

  it("go off when the tab with changes is closed", async () => {
    const { app } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    const closing = app.requestClose(1);
    await flush();
    pressButton("Don't save");
    await closing;
    expect(saveButton().disabled).toBe(true);
    expect(copyButton().disabled).toBe(true);
  });
});

describe("keyboard shortcuts on a layout without Latin letters", () => {
  const press = (target: HTMLElement, key: string, code: string, init: KeyboardEventInit = {}) =>
    target.dispatchEvent(
      new KeyboardEvent("keydown", { key, code, ctrlKey: true, bubbles: true, cancelable: true, ...init }),
    );

  it("saves with Ctrl+S on a Russian layout (the key prints a Cyrillic letter)", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    press(document.body, "ы", "KeyS");
    await flush();
    expect(api.save).toHaveBeenCalledTimes(1);
  });

  it("saves a copy on a Hebrew layout, undoes on an Arabic one and closes with Ctrl+W on a Greek one", async () => {
    const { app, api } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    press(document.body, "ד", "KeyS", { shiftKey: true });
    await flush();
    expect(api.saveAs).toHaveBeenCalledTimes(1);
    field(1).click();
    await flush();
    expect(field(1).getAttribute("aria-checked")).toBe("true");
    press(field(1), "ئ", "KeyZ");
    await flush();
    expect(field(1).getAttribute("aria-checked")).toBe("false");
    press(document.body, "ς", "KeyW");
    await flush();
    expect(app.tabCount).toBe(0);
  });

  it("opens the file dialog with Ctrl+O on a Russian layout", () => {
    const { deps } = make();
    press(document.body, "щ", "KeyO");
    expect(deps.pickFiles).toHaveBeenCalledTimes(1);
  });

  it("does not close the tab with Ctrl+comma on Dvorak or Ctrl+e-acute on BEPO", async () => {
    const { app } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    press(document.body, ",", "KeyW");
    press(document.body, "<", "KeyW", { shiftKey: true });
    press(document.body, "é", "KeyW");
    await flush();
    expect(app.tabCount).toBe(1);
    // The same place does close the tab on a layout without Latin letters.
    press(document.body, "ц", "KeyW");
    await flush();
    expect(app.tabCount).toBe(0);
  });

  it("keeps the letter that is printed on a Dvorak layout", async () => {
    const { app, api, deps } = make();
    await app.openPath("/docs/form.pdf");
    await flush();
    const input = field(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    // The key in the S place prints O on Dvorak: it opens the file dialog and does not save.
    press(document.body, "o", "KeyS");
    await flush();
    expect(deps.pickFiles).toHaveBeenCalledTimes(1);
    expect(api.save).not.toHaveBeenCalled();
    // The key that prints S sits in the semicolon place.
    press(document.body, "s", "Semicolon");
    await flush();
    expect(api.save).toHaveBeenCalledTimes(1);
  });
});

// @vitest-environment jsdom
// Filling in a form through the whole window: the controls over the page, the mark that the
// document has changes that are not saved, undo and redo from the keyboard, and the notices.
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenedDocument, SavedFile } from "../lib/api";
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

function make(options: { fields?: FormFieldInfo[]; info?: FormInfo; withForms?: boolean } = {}) {
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
  const deps = {
    api: options.withForms === false ? { ...api, formInfo: undefined, getFormFields: undefined, setFormField: undefined } : api,
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
  return { app, api, deps, state };
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

  it("is given up when the box loses the focus without a change being sent", async () => {
    const { app, input } = await typing();
    input.dispatchEvent(new Event("blur"));
    expect(input.value).toBe("");
    expect(app.unsavedCount).toBe(0);
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

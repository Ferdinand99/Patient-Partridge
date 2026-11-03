// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ACTIONS_NOTICE,
  BLOCKED_NOTICE,
  FORM_BLOCKED_NOTICE,
  FormLayer,
  XFA_NOTICE,
  type FormEdit,
  type FormFieldInfo,
  type FormInfo,
  type FormValueIn,
  type FormsApi,
} from "./forms";
import { DocumentView, type ViewerApi } from "./viewer";

const letter = { width: 612, height: 792 };

const flush = async () => {
  for (let i = 0; i < 8; i++) await new Promise((r) => setTimeout(r, 0));
};

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
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  document.body.replaceChildren();
});

/** A field with everything off; a test turns on what it needs. */
function field(over: Partial<FormFieldInfo> & Pick<FormFieldInfo, "index" | "name">): FormFieldInfo {
  return {
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
    rect: [150, 68, 400, 92],
    fontSize: 12,
    tooltip: "",
    value: "",
    maxLen: 0,
    options: [],
    ...over,
  };
}

const opt = (label: string, selected = false) => ({ label, selected });

/**
 * A stand-in for the app process that remembers the fields of each page and changes them the way the
 * engine does, as far as these tests need.
 */
function fake(pages: FormFieldInfo[][], info: FormInfo = { kind: "acro", actions: false }) {
  const state = pages.map((list) => structuredClone(list));
  const api = {
    formInfo: vi.fn((_doc: number): Promise<FormInfo> => Promise.resolve(info)),
    getFormFields: vi.fn((_doc: number, page: number) => Promise.resolve(structuredClone(state[page] ?? []))),
    setFormField: vi.fn(
      (_doc: number, page: number, index: number, name: string, value: FormValueIn): Promise<FormFieldInfo[]> => {
        const list = state[page];
        const target = list?.find((f) => f.index === index);
        if (!target || target.name !== name) {
          return Promise.reject({ code: "form_field_refused", message: "not the one" });
        }
        if (value.kind === "text") target.value = value.text;
        else if (value.kind === "choice") target.options.forEach((o, i) => (o.selected = value.picked.includes(i)));
        else {
          if (target.kind === "radio") {
            if (!value.on) return Promise.reject({ code: "form_field_refused", message: "cannot" });
            for (const f of list) if (f.kind === "radio" && f.name === name) f.checked = false;
          }
          target.checked = value.on;
        }
        // The widgets of the same field on other pages show the same value.
        for (const other of state) {
          if (other === list) continue;
          for (const f of other) {
            if (f.name !== name || f.kind !== target.kind) continue;
            f.value = target.value;
            f.options.forEach((o, i) => (o.selected = target.options[i]?.selected ?? false));
          }
        }
        return Promise.resolve(structuredClone(list));
      },
    ),
  };
  return { api: api as FormsApi & typeof api, state };
}

interface Rig {
  layer: FormLayer;
  host: HTMLElement;
  page: HTMLElement;
  api: ReturnType<typeof fake>["api"];
  state: FormFieldInfo[][];
  edits: FormEdit[];
  redraws: number[];
  controls(): HTMLElement[];
  control(index: number): HTMLElement;
  bar(): string;
}

async function rig(
  pages: FormFieldInfo[][],
  info?: FormInfo,
  mountPages: number[] = pages.map((_, i) => i),
): Promise<Rig> {
  const { api, state } = fake(pages, info);
  const host = document.createElement("div");
  host.className = "viewer";
  document.body.append(host);
  const edits: FormEdit[] = [];
  const redraws: number[] = [];
  const layer = new FormLayer(7, pages.map(() => letter), api, host, {
    onEdit: (e) => edits.push(e),
    onRedraw: (p) => redraws.push(p),
  });
  layer.start();
  await flush();
  const page = document.createElement("div");
  host.append(page);
  for (const p of mountPages) {
    layer.mount(p, p === mountPages[0] ? page : host.appendChild(document.createElement("div")));
    layer.pageDrawn(p);
  }
  await flush();
  return {
    layer,
    host,
    page,
    api,
    state,
    edits,
    redraws,
    controls: () => [...page.querySelectorAll<HTMLElement>(".form-field")],
    control: (index) => page.querySelector<HTMLElement>(`.form-field[data-form-index="${index}"]`) as HTMLElement,
    bar: () => host.querySelector(".form-bar:not([hidden]) .form-bar-text")?.textContent ?? "",
  };
}

/** A percentage as a number: the browser may write 25.000% as 25%. */
const pct = (value: string) => Number.parseFloat(value);

const change = (el: HTMLElement) => el.dispatchEvent(new Event("change", { bubbles: true }));

describe("the controls of a form", () => {
  it("puts a control of the right kind on the page for each field, named for a screen reader", async () => {
    const r = await rig([
      [
        field({ index: 0, name: "fullname", tooltip: "Full name", required: true }),
        field({ index: 1, name: "plain" }),
        field({ index: 2, name: "notes", multiline: true }),
        field({ index: 3, name: "secret", password: true }),
        field({ index: 4, name: "agree", kind: "checkbox", tooltip: "I agree" }),
        field({ index: 5, name: "locked", readOnly: true, value: "Fixed" }),
        field({ index: 6, name: "country", kind: "combo", options: [opt("Norway"), opt("Sweden")] }),
        field({ index: 7, name: "city", kind: "combo", editable: true, options: [opt("Oslo")] }),
        field({ index: 8, name: "colours", kind: "list", options: [opt("Red"), opt("Green")] }),
        field({ index: 9, name: "toppings", kind: "list", multiselect: true, options: [opt("Ham"), opt("Cheese")] }),
      ],
    ]);
    expect(r.controls()).toHaveLength(10);
    const full = r.control(0) as HTMLInputElement;
    expect(full.tagName).toBe("INPUT");
    expect(full.type).toBe("text");
    // The tooltip names the field; without one the name does.
    expect(full.getAttribute("aria-label")).toBe("Full name");
    expect(full.title).toBe("Full name");
    expect(r.control(1).getAttribute("aria-label")).toBe("plain");
    expect(full.getAttribute("aria-required")).toBe("true");
    expect(r.control(1).hasAttribute("aria-required")).toBe(false);
    expect(r.control(2).tagName).toBe("TEXTAREA");
    expect((r.control(3) as HTMLInputElement).type).toBe("password");
    const agree = r.control(4);
    expect(agree.tagName).toBe("BUTTON");
    expect(agree.getAttribute("role")).toBe("checkbox");
    expect(agree.getAttribute("aria-checked")).toBe("false");
    const locked = r.control(5) as HTMLInputElement;
    expect(locked.readOnly).toBe(true);
    expect(locked.getAttribute("aria-readonly")).toBe("true");
    expect(locked.value).toBe("Fixed");
    expect(r.control(6).tagName).toBe("SELECT");
    expect((r.control(7) as HTMLInputElement).getAttribute("list")).toBeTruthy();
    expect((r.control(8) as HTMLSelectElement).multiple).toBe(false);
    expect((r.control(9) as HTMLSelectElement).multiple).toBe(true);
    // The order of the controls in the page is the order of the fields: that is the order of the Tab key.
    expect(r.controls().map((c) => c.dataset.formIndex)).toEqual(["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"]);
  });

  it("shows what the PDF says as text and nothing else", async () => {
    const hostile = '<img src=x onerror="window.pwned=1"><script>window.pwned=2</script>';
    const r = await rig([
      [
        field({ index: 0, name: hostile, tooltip: hostile, value: hostile }),
        field({ index: 1, name: "c", kind: "combo", options: [opt(hostile, true)] }),
        field({ index: 2, name: "r", kind: "radio", tooltip: hostile }),
      ],
    ]);
    expect(r.page.querySelectorAll("img, script, b").length).toBe(0);
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
    const text = r.control(0) as HTMLInputElement;
    expect(text.value).toBe(hostile);
    expect(text.getAttribute("aria-label")).toBe(hostile);
    expect(text.title).toBe(hostile);
    expect((r.control(1) as HTMLSelectElement).selectedOptions[0].textContent).toBe(hostile);
    expect(r.control(2).getAttribute("aria-label")).toContain(hostile);
    // The bar says its own words only.
    expect(r.host.querySelector(".form-bar")).toBeNull();
  });

  it("places a field with percentages of the page and sizes its text from the zoom", async () => {
    const r = await rig([[field({ index: 0, name: "a", rect: [153, 79.2, 459, 118.8], fontSize: 10 })]]);
    const el = r.control(0);
    expect(pct(el.style.left)).toBeCloseTo(25, 2);
    expect(pct(el.style.top)).toBeCloseTo(10, 2);
    expect(pct(el.style.width)).toBeCloseTo(50, 2);
    expect(pct(el.style.height)).toBeCloseTo(5, 2);
    // Ten points, whatever the zoom: the layer says how many pixels a point is.
    expect(el.style.fontSize).toBe("calc(var(--form-k, 1) * 10.00px)");
    // A field without a size of its own gets one from its height, within limits.
    const auto = await rig([[field({ index: 0, name: "a", fontSize: 0, rect: [0, 0, 100, 100] })]]);
    expect(auto.control(0).style.fontSize).toBe("calc(var(--form-k, 1) * 14.00px)");
  });

  it("turns a control with a turned page, about its middle", async () => {
    // A field 100 wide and 20 high on the unturned page, as it is drawn after a quarter turn:
    // 20 wide and 100 high, around the point (306, 396).
    const r = await rig([[field({ index: 0, name: "a", turns: 1, rect: [296, 346, 316, 446] })]]);
    const el = r.control(0);
    expect(el.style.transform).toBe("rotate(90deg)");
    // The control keeps the shape of the unturned field: 100 wide and 20 high.
    expect(pct(el.style.width)).toBeCloseTo((100 / 612) * 100, 2);
    expect(pct(el.style.height)).toBeCloseTo((20 / 792) * 100, 2);
    // Its middle is the middle of the drawn field.
    expect(pct(el.style.left)).toBeCloseTo(((306 - 50) / 612) * 100, 2);
    expect(pct(el.style.top)).toBeCloseTo(((396 - 10) / 792) * 100, 2);
    const half = await rig([[field({ index: 0, name: "a", turns: 2 })]]);
    expect(half.control(0).style.transform).toBe("rotate(180deg)");
    const none = await rig([[field({ index: 0, name: "a" })]]);
    expect(none.control(0).style.transform).toBe("");
  });

  it("marks a required field that is still empty, and a read-only field", async () => {
    const r = await rig([
      [field({ index: 0, name: "a", required: true }), field({ index: 1, name: "b", readOnly: true })],
    ]);
    expect(r.control(0).classList.contains("form-missing")).toBe(true);
    expect(r.control(0).classList.contains("form-required")).toBe(true);
    expect(r.control(1).classList.contains("form-readonly")).toBe(true);
    const input = r.control(0) as HTMLInputElement;
    input.value = "x";
    change(input);
    await flush();
    expect(r.control(0).classList.contains("form-missing")).toBe(false);
    expect(r.control(0).classList.contains("form-required")).toBe(true);
  });
});

describe("filling in", () => {
  it("sends a text when the reader leaves the field and records the change for undo and redo", async () => {
    const r = await rig([[field({ index: 3, name: "fullname", value: "old" })]]);
    const input = r.control(3) as HTMLInputElement;
    input.focus();
    input.value = "Ada";
    change(input);
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledWith(7, 0, 3, "fullname", { kind: "text", text: "Ada" });
    expect(r.redraws).toEqual([0]);
    expect(r.edits).toHaveLength(1);
    expect(input.value).toBe("Ada");
    // The picture below is drawn again before the control lets go of it.
    expect(input.classList.contains("form-pending")).toBe(true);
    r.layer.pageDrawn(0);
    expect(input.classList.contains("form-pending")).toBe(false);

    await r.edits[0].undo();
    expect(r.api.setFormField).toHaveBeenLastCalledWith(7, 0, 3, "fullname", { kind: "text", text: "old" });
    expect((r.control(3) as HTMLInputElement).value).toBe("old");
    await r.edits[0].redo();
    expect(r.api.setFormField).toHaveBeenLastCalledWith(7, 0, 3, "fullname", { kind: "text", text: "Ada" });
    expect((r.control(3) as HTMLInputElement).value).toBe("Ada");
  });

  it("sends nothing when the text did not change, and Enter and leaving send it once", async () => {
    const r = await rig([[field({ index: 0, name: "a", value: "same" })]]);
    const input = r.control(0) as HTMLInputElement;
    change(input);
    await flush();
    expect(r.api.setFormField).not.toHaveBeenCalled();
    input.value = "new";
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    change(input);
    input.dispatchEvent(new Event("blur"));
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledTimes(1);
  });

  it("sends text that was typed when the box loses the focus and no change event comes", async () => {
    // WebKit sends a blur and no change when the window loses the focus, and the change only when
    // the focus comes back, if at all.
    const r = await rig([[field({ index: 0, name: "a", value: "old" })]]);
    const input = r.control(0) as HTMLInputElement;
    input.focus();
    input.value = "typed";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(r.layer.hasPendingInput).toBe(true);
    input.dispatchEvent(new Event("blur"));
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledTimes(1);
    expect(r.api.setFormField).toHaveBeenCalledWith(7, 0, 0, "a", { kind: "text", text: "typed" });
    expect(input.value).toBe("typed");
    expect(r.edits).toHaveLength(1);
    // The change event that comes later finds the text sent.
    change(input);
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledTimes(1);
    expect(r.layer.hasPendingInput).toBe(false);
  });

  it("sends the text of a box with several lines when it loses the focus with no change event", async () => {
    const r = await rig([[field({ index: 0, name: "notes", multiline: true })]]);
    const area = r.control(0) as HTMLTextAreaElement;
    area.focus();
    area.value = "one\ntwo";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    area.dispatchEvent(new Event("blur"));
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledWith(7, 0, 0, "notes", { kind: "text", text: "one\ntwo" });
  });

  it("shows the text of the engine again when a box that was not typed in loses the focus", async () => {
    const r = await rig([[field({ index: 0, name: "a", value: "kept" })]]);
    const input = r.control(0) as HTMLInputElement;
    input.focus();
    // Changed by something other than typing (no input event), so there is nothing to send.
    input.value = "stray";
    input.dispatchEvent(new Event("blur"));
    await flush();
    expect(input.value).toBe("kept");
    expect(r.api.setFormField).not.toHaveBeenCalled();
  });

  it("puts the old text back when the reader presses Escape", async () => {
    const r = await rig([[field({ index: 0, name: "a", value: "kept" })]]);
    const input = r.control(0) as HTMLInputElement;
    input.value = "typed";
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    change(input);
    await flush();
    expect(input.value).toBe("kept");
    expect(r.api.setFormField).not.toHaveBeenCalled();
  });

  it("sends the lines of a text box with \\n and shows what the engine kept", async () => {
    const r = await rig([[field({ index: 0, name: "n", multiline: true, maxLen: 5 })]]);
    const area = r.control(0) as HTMLTextAreaElement;
    expect(area.maxLength).toBe(5);
    area.value = "a\r\nb";
    change(area);
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledWith(7, 0, 0, "n", { kind: "text", text: "a\nb" });
  });

  it("does not change a field that is read only", async () => {
    const r = await rig([
      [
        field({ index: 0, name: "a", readOnly: true, value: "fixed" }),
        field({ index: 1, name: "b", kind: "checkbox", readOnly: true }),
        field({ index: 2, name: "c", kind: "combo", readOnly: true, options: [opt("X", true), opt("Y")] }),
        field({ index: 3, name: "d", valueCut: true, value: "cut" }),
      ],
    ]);
    const input = r.control(0) as HTMLInputElement;
    input.value = "changed";
    change(input);
    r.control(1).click();
    const select = r.control(2) as HTMLSelectElement;
    select.value = "2";
    change(select);
    const cut = r.control(3) as HTMLInputElement;
    expect(cut.readOnly).toBe(true);
    cut.value = "other";
    change(cut);
    await flush();
    expect(r.api.setFormField).not.toHaveBeenCalled();
    expect(input.value).toBe("fixed");
    expect(select.value).toBe("0");
    expect(cut.value).toBe("cut");
    expect(r.edits).toHaveLength(0);
  });

  it("toggles a check box and takes it back", async () => {
    const r = await rig([[field({ index: 0, name: "agree", kind: "checkbox" })]]);
    r.control(0).click();
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledWith(7, 0, 0, "agree", { kind: "checked", on: true });
    expect(r.control(0).getAttribute("aria-checked")).toBe("true");
    r.control(0).click();
    await flush();
    expect(r.api.setFormField).toHaveBeenLastCalledWith(7, 0, 0, "agree", { kind: "checked", on: false });
    expect(r.control(0).getAttribute("aria-checked")).toBe("false");
    expect(r.edits).toHaveLength(2);
    await r.edits[1].undo();
    expect(r.control(0).getAttribute("aria-checked")).toBe("true");
    await r.edits[0].undo();
    expect(r.control(0).getAttribute("aria-checked")).toBe("false");
  });

  it("picks a radio button, moves with the arrow keys, and is one stop for the Tab key", async () => {
    const radios = ["S", "M", "L"].map((_, i) => field({ index: i, name: "size", kind: "radio", tooltip: "Size" }));
    const r = await rig([radios]);
    const group = r.page.querySelector('[role="radiogroup"]');
    expect(group?.getAttribute("aria-label")).toBe("Size");
    expect(r.control(0).getAttribute("aria-label")).toBe("Size, choice 1 of 3");
    // None is on: the first is the one stop.
    expect(r.controls().map((c) => c.tabIndex)).toEqual([0, -1, -1]);
    r.control(1).click();
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledWith(7, 0, 1, "size", { kind: "checked", on: true });
    expect(r.controls().map((c) => c.getAttribute("aria-checked"))).toEqual(["false", "true", "false"]);
    expect(r.controls().map((c) => c.tabIndex)).toEqual([-1, 0, -1]);
    // Pressing the one that is on does nothing.
    r.api.setFormField.mockClear();
    r.control(1).click();
    await flush();
    expect(r.api.setFormField).not.toHaveBeenCalled();
    // The arrow key moves to the next, wraps round, and picks it.
    r.control(1).focus();
    r.control(1).dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    await flush();
    expect(r.controls().map((c) => c.getAttribute("aria-checked"))).toEqual(["false", "false", "true"]);
    r.control(2).dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    await flush();
    expect(r.controls().map((c) => c.getAttribute("aria-checked"))).toEqual(["true", "false", "false"]);
    r.control(0).dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true, cancelable: true }));
    await flush();
    expect(r.controls().map((c) => c.getAttribute("aria-checked"))).toEqual(["false", "false", "true"]);
  });

  it("takes a radio choice back by choosing the one before, and says when that cannot be done", async () => {
    const radios = [0, 1, 2].map((i) => field({ index: i, name: "size", kind: "radio" }));
    const r = await rig([radios]);
    r.control(0).click();
    await flush();
    r.control(2).click();
    await flush();
    expect(r.edits).toHaveLength(2);
    await r.edits[1].undo();
    expect(r.api.setFormField).toHaveBeenLastCalledWith(7, 0, 0, "size", { kind: "checked", on: true });
    expect(r.controls().map((c) => c.getAttribute("aria-checked"))).toEqual(["true", "false", "false"]);
    // The first choice of a group that had none cannot be taken back: PDFium cannot clear a group.
    r.api.setFormField.mockClear();
    await expect(r.edits[0].undo()).rejects.toThrow();
    expect(r.api.setFormField).not.toHaveBeenCalled();
    expect(r.bar()).toContain("cannot be turned off");
    expect(r.controls().map((c) => c.getAttribute("aria-checked"))).toEqual(["true", "false", "false"]);
  });

  it("sends the choices of a drop-down list and a list box", async () => {
    const r = await rig([
      [
        field({ index: 0, name: "country", kind: "combo", options: [opt("Norway"), opt("Sweden")] }),
        field({ index: 1, name: "toppings", kind: "list", multiselect: true, options: [opt("Ham"), opt("Cheese"), opt("Olives")] }),
        field({ index: 2, name: "colours", kind: "list", options: [opt("Red"), opt("Green")] }),
      ],
    ]);
    const country = r.control(0) as HTMLSelectElement;
    // Nothing chosen: an empty line is shown first and cannot be picked.
    expect(country.options).toHaveLength(3);
    expect(country.options[0].disabled).toBe(true);
    expect(country.value).toBe("-1");
    country.value = "1";
    change(country);
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledWith(7, 0, 0, "country", { kind: "choice", picked: [1] });
    const toppings = r.control(1) as HTMLSelectElement;
    toppings.options[0].selected = true;
    toppings.options[2].selected = true;
    change(toppings);
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledWith(7, 0, 1, "toppings", { kind: "choice", picked: [0, 2] });
    expect(Array.from((r.control(1) as HTMLSelectElement).selectedOptions).map((o) => o.textContent)).toEqual(["Ham", "Olives"]);
    // Undo of a choice goes back to the choice before it.
    await r.edits[1].undo();
    expect(Array.from((r.control(1) as HTMLSelectElement).selectedOptions)).toHaveLength(0);
    // A drop-down list that showed nothing cannot go back to showing nothing.
    await expect(r.edits[0].undo()).rejects.toThrow();
    expect(r.bar()).toContain("cannot be cleared");
  });

  it("sends the text of a drop-down list with a box to type in", async () => {
    const r = await rig([[field({ index: 0, name: "city", kind: "combo", editable: true, options: [opt("Oslo")] })]]);
    const input = r.control(0) as HTMLInputElement;
    input.value = "Tromsø";
    change(input);
    await flush();
    expect(r.api.setFormField).toHaveBeenCalledWith(7, 0, 0, "city", { kind: "text", text: "Tromsø" });
    expect(r.page.querySelector("datalist option")?.getAttribute("value")).toBe("Oslo");
  });

  it("shows what the engine has again, and says why, when it refuses a change", async () => {
    const r = await rig([[field({ index: 0, name: "country", kind: "combo", options: [opt("Norway", true), opt("Sweden")] })]]);
    r.api.setFormField.mockRejectedValueOnce({ code: "form_field_refused", message: "no" });
    const select = r.control(0) as HTMLSelectElement;
    select.value = "2";
    change(select);
    await flush();
    expect(r.edits).toHaveLength(0);
    expect(r.redraws).toHaveLength(0);
    expect((r.control(0) as HTMLSelectElement).value).toBe("0");
    expect(r.bar()).toContain("cannot be cleared");
    expect(r.control(0).classList.contains("form-pending")).toBe(false);
    // The cross puts the message away.
    r.host.querySelector<HTMLButtonElement>(".form-bar-close")?.click();
    expect(r.bar()).toBe("");
    expect(r.host.querySelector(".form-bar")?.hasAttribute("hidden")).toBe(true);
  });

  it("sends the changes one after the other, in the order they were made", async () => {
    const r = await rig([[field({ index: 0, name: "a" }), field({ index: 1, name: "b" })]]);
    let release: () => void = () => undefined;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const inner = r.api.setFormField.getMockImplementation() as NonNullable<ReturnType<typeof r.api.setFormField.getMockImplementation>>;
    r.api.setFormField.mockImplementationOnce(async (...args: Parameters<typeof inner>) => {
      await gate;
      return inner(...args);
    });
    const a = r.control(0) as HTMLInputElement;
    const b = r.control(1) as HTMLInputElement;
    a.value = "1";
    change(a);
    b.value = "2";
    change(b);
    await flush();
    // The second is not sent while the first is on its way.
    expect(r.api.setFormField).toHaveBeenCalledTimes(1);
    release();
    await flush();
    expect(r.api.setFormField.mock.calls.map((c) => c[3])).toEqual(["a", "b"]);
    expect(r.edits).toHaveLength(2);
  });
});

describe("a field with widgets on more than one page", () => {
  it("shows a text that is set in the widget of another page too, and has that page drawn again", async () => {
    const r = await rig([[field({ index: 0, name: "applicant" })], [field({ index: 0, name: "applicant" })]]);
    const input = r.control(0) as HTMLInputElement;
    input.value = "Ada";
    change(input);
    await flush();
    const second = r.host.querySelectorAll(".form-layer")[1].querySelector("input") as HTMLInputElement;
    expect(second.value).toBe("Ada");
    expect(r.redraws).toEqual([0, 1]);
  });

  it("does the same for a drop-down list", async () => {
    const list = (selected: boolean) =>
      field({ index: 0, name: "country", kind: "combo", options: [opt("Norway", selected), opt("Sweden")] });
    const r = await rig([[list(true)], [list(true)]]);
    const select = r.control(0) as HTMLSelectElement;
    select.value = "1";
    change(select);
    await flush();
    const second = r.host.querySelectorAll(".form-layer")[1].querySelector("select") as HTMLSelectElement;
    expect(second.value).toBe("1");
    expect(r.redraws).toEqual([0, 1]);
  });
});

describe("what the form bar says", () => {
  it("says an XFA form is not filled in, and asks for no field", async () => {
    const r = await rig([[field({ index: 0, name: "a" })]], { kind: "xfa", actions: true });
    expect(r.bar()).toBe(XFA_NOTICE);
    expect(r.api.getFormFields).not.toHaveBeenCalled();
    expect(r.controls()).toHaveLength(0);
    expect(r.layer.fillable).toBe(false);
  });

  it("says in plain words why a field is left alone, and says it again when the reader tries the field", async () => {
    const r = await rig(
      [[field({ index: 0, name: "a", readOnly: true, action: true, actionBlocked: true, tooltip: "Name" }), field({ index: 1, name: "b" })]],
      { kind: "acro", actions: true },
    );
    // Once, when the page is shown.
    expect(r.bar()).toBe(BLOCKED_NOTICE);
    expect(BLOCKED_NOTICE).toContain("cannot be filled in here");
    expect(BLOCKED_NOTICE).toContain("Some fields");
    expect(BLOCKED_NOTICE).toContain("may have an action that could clear or hide fields");
    expect(BLOCKED_NOTICE).toContain("leaves these fields as they are");
    expect(BLOCKED_NOTICE).not.toContain("does not run");
    expect(BLOCKED_NOTICE).not.toContain("button");
    expect(BLOCKED_NOTICE).toContain("another PDF reader");
    // The field says it too.
    expect(r.control(0).title).toContain("Name - This field cannot be filled in here");
    expect(r.control(0).title).toContain("it may have an action that could clear or hide fields");
    expect(r.control(0).title).toContain("leaves it as it is");
    expect(r.control(0).title).not.toContain("does not run");
    expect(r.control(1).title).toBe("");
    // The reader puts the message away, then tries the field: it comes back, once.
    r.host.querySelector<HTMLButtonElement>(".form-bar-close")?.click();
    expect(r.bar()).toBe(ACTIONS_NOTICE);
    r.host.querySelector<HTMLButtonElement>(".form-bar-close")?.click();
    expect(r.bar()).toBe("");
    const input = r.control(0) as HTMLInputElement;
    input.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    input.focus();
    input.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(r.bar()).toBe(BLOCKED_NOTICE);
    expect(r.host.querySelectorAll(".form-bar-text")).toHaveLength(1);
    // A field that can be filled in does not bring it.
    r.host.querySelector<HTMLButtonElement>(".form-bar-close")?.click();
    expect(r.bar()).toBe("");
    (r.control(1) as HTMLInputElement).focus();
    (r.control(1) as HTMLInputElement).dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(r.bar()).toBe("");
    expect(r.api.setFormField).not.toHaveBeenCalled();
  });

  it("says something true when no field can be filled in: no 'some', and no script on a plain field", async () => {
    const r = await rig(
      [[field({ index: 0, name: "a", readOnly: true, actionBlocked: true, formBlocked: true, tooltip: "Name" }), field({ index: 1, name: "b", readOnly: true, actionBlocked: true, formBlocked: true })]],
      { kind: "acro", actions: false },
    );
    expect(r.bar()).toBe(FORM_BLOCKED_NOTICE);
    expect(FORM_BLOCKED_NOTICE).not.toContain("Some fields");
    expect(FORM_BLOCKED_NOTICE).not.toContain("script");
    expect(FORM_BLOCKED_NOTICE).not.toContain("action");
    expect(FORM_BLOCKED_NOTICE).toContain("cannot be sure");
    expect(FORM_BLOCKED_NOTICE).toContain("another PDF reader");
    // A plain field is not told that it has an action or a script.
    for (const index of [0, 1]) {
      expect(r.control(index).title).not.toMatch(/script|has an action/);
      expect(r.control(index).title).toContain("No field of this form can be filled in here");
    }
    expect(r.control(0).title).toContain("Name - ");
    // Pressing a field says the same words as the bar.
    r.host.querySelector<HTMLButtonElement>(".form-bar-close")?.click();
    expect(r.bar()).toBe("");
    r.control(1).dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
    expect(r.bar()).toBe(FORM_BLOCKED_NOTICE);
    expect(r.api.setFormField).not.toHaveBeenCalled();
  });

  it("says why for a check box and a drop-down list that are left alone, when they are pressed", async () => {
    const r = await rig(
      [
        [
          field({ index: 0, name: "box", kind: "checkbox", readOnly: true, action: true, actionBlocked: true }),
          field({ index: 1, name: "pick", kind: "combo", readOnly: true, action: true, actionBlocked: true, options: [opt("X", true)] }),
        ],
      ],
      { kind: "acro", actions: true },
    );
    for (const index of [0, 1]) {
      r.host.querySelector<HTMLButtonElement>(".form-bar-close")?.click();
      r.host.querySelector<HTMLButtonElement>(".form-bar-close")?.click();
      expect(r.bar()).toBe("");
      r.control(index).dispatchEvent(new MouseEvent("mousedown", { bubbles: true, cancelable: true }));
      expect(r.bar()).toBe(BLOCKED_NOTICE);
    }
    expect(r.api.setFormField).not.toHaveBeenCalled();
  });

  it("says once that scripts and actions do not run", async () => {
    const r = await rig(
      [[field({ index: 0, name: "a", action: true })], [field({ index: 0, name: "b", action: true })]],
      { kind: "acro", actions: true },
    );
    expect(r.bar()).toBe(ACTIONS_NOTICE);
    expect(r.host.querySelectorAll(".form-bar")).toHaveLength(1);
    // Dismissed, it stays away: the fields of the next page do not bring it back.
    r.host.querySelector<HTMLButtonElement>(".form-bar-close")?.click();
    expect(r.bar()).toBe("");
    r.layer.unmount(1);
    r.layer.mount(1, r.host.appendChild(document.createElement("div")));
    r.layer.pageDrawn(1);
    await flush();
    expect(r.bar()).toBe("");
  });

  it("learns about an action from a field when the document does not say", async () => {
    const r = await rig([[field({ index: 0, name: "a", action: true })]], { kind: "acro", actions: false });
    expect(r.bar()).toBe(ACTIONS_NOTICE);
  });

  it("says nothing about a plain form, and a short message goes away by itself", async () => {
    vi.useFakeTimers();
    const { api } = fake([[field({ index: 0, name: "c", kind: "combo", options: [opt("X", true)] })]]);
    const host = document.createElement("div");
    document.body.append(host);
    const layer = new FormLayer(7, [letter], api, host, { onEdit: () => undefined, onRedraw: () => undefined });
    layer.start();
    await vi.advanceTimersByTimeAsync(10);
    const page = host.appendChild(document.createElement("div"));
    layer.mount(0, page);
    layer.pageDrawn(0);
    await vi.advanceTimersByTimeAsync(10);
    expect(host.querySelector(".form-bar")).toBeNull();
    api.setFormField.mockRejectedValueOnce({ code: "form_field_refused", message: "no" });
    const select = page.querySelector<HTMLSelectElement>("select") as HTMLSelectElement;
    select.value = "0";
    select.options[1].selected = false;
    change(select);
    await vi.advanceTimersByTimeAsync(10);
    expect(host.querySelector(".form-bar:not([hidden])")).not.toBeNull();
    await vi.advanceTimersByTimeAsync(7000);
    expect(host.querySelector(".form-bar:not([hidden])")).toBeNull();
  });
});

describe("pages that come and go", () => {
  it("asks for the fields of a page again when it is mounted again, and not before it is drawn", async () => {
    const { api } = fake([[field({ index: 0, name: "a" })]]);
    const host = document.createElement("div");
    document.body.append(host);
    const layer = new FormLayer(7, [letter], api, host, { onEdit: () => undefined, onRedraw: () => undefined });
    layer.start();
    await flush();
    const page = host.appendChild(document.createElement("div"));
    layer.mount(0, page);
    await flush();
    // The picture is not there yet; mounted in a document, the fields are asked for at once.
    expect(page.querySelectorAll(".form-field")).toHaveLength(1);
    layer.unmount(0);
    expect(page.querySelector(".form-layer")).toBeNull();
    const again = host.appendChild(document.createElement("div"));
    layer.mount(0, again);
    await flush();
    expect(again.querySelectorAll(".form-field")).toHaveLength(1);
    expect(api.getFormFields).toHaveBeenCalledTimes(2);
    layer.destroy();
    expect(host.querySelector(".form-layer")).toBeNull();
  });

  it("shows the field after a change on another page of a radio group", async () => {
    const group = (n: number) => field({ index: n, name: "size", kind: "radio" });
    const r = await rig([[group(0)], [group(0)]]);
    r.control(0).click();
    await flush();
    expect(r.redraws).toContain(0);
  });
});

describe("what the notice about scripts and actions says", () => {
  it("says that scripts are not run and that the PDF library can run other actions", () => {
    expect(ACTIONS_NOTICE).toContain("does not run scripts");
    expect(ACTIONS_NOTICE).toContain("The PDF library can run some other actions itself");
    expect(ACTIONS_NOTICE).toContain("clear or hide other fields");
    // Actions are not said to be left unrun as a whole.
    expect(ACTIONS_NOTICE).not.toContain("actions that Patient Partridge does not run");
  });
});

describe("going to a field", () => {
  beforeEach(() => {
    HTMLElement.prototype.scrollIntoView = vi.fn();
  });
  afterEach(() => {
    delete (HTMLElement.prototype as { scrollIntoView?: unknown }).scrollIntoView;
  });

  it("focuses a field that is on screen", async () => {
    const r = await rig([[field({ index: 0, name: "a" }), field({ index: 1, name: "b" })]]);
    r.layer.reveal(0, 1);
    expect(document.activeElement).toBe(r.control(1));
    expect(HTMLElement.prototype.scrollIntoView).toHaveBeenCalledTimes(1);
  });

  it("waits for a page that is not there yet, and focuses the field when its box is built", async () => {
    const r = await rig([[field({ index: 0, name: "a" })], [field({ index: 0, name: "b" })]], undefined, [0]);
    r.layer.reveal(1, 0);
    expect(document.activeElement).not.toBeNull();
    const second = r.host.appendChild(document.createElement("div"));
    r.layer.mount(1, second);
    r.layer.pageDrawn(1);
    await flush();
    expect(document.activeElement).toBe(second.querySelector('.form-field[data-form-index="0"]'));
  });

  it("gives up on a page that never shows the field", async () => {
    const r = await rig([[field({ index: 0, name: "a" })], [field({ index: 0, name: "b" })]], undefined, [0]);
    vi.useFakeTimers();
    try {
      r.layer.reveal(1, 0);
      await vi.advanceTimersByTimeAsync(6000);
      const second = r.host.appendChild(document.createElement("div"));
      r.layer.mount(1, second);
      r.layer.pageDrawn(1);
      await vi.advanceTimersByTimeAsync(50);
      expect(document.activeElement).not.toBe(second.querySelector(".form-field"));
    } finally {
      vi.useRealTimers();
    }
  });

  it("lists the fields with refused text by page, with their labels", async () => {
    const r = await rig([[field({ index: 0, name: "notes", tooltip: "Notes" })], [field({ index: 0, name: "more" })]]);
    r.api.setFormField.mockRejectedValue({ code: "invalid_request", message: "too long" });
    for (const [index, el] of [
      [1, r.host.querySelector<HTMLInputElement>('.form-layer[data-page="1"] .form-field')],
      [0, r.control(0) as HTMLInputElement],
    ] as const) {
      expect(el).not.toBeNull();
      const input = el as HTMLInputElement;
      input.focus();
      input.value = `text ${index}`;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.blur();
      await flush();
    }
    expect(r.layer.refusedFields).toEqual([
      { page: 0, index: 0, label: "Notes" },
      { page: 1, index: 0, label: "more" },
    ]);
  });
});

describe("the view of a document with a form", () => {
  function open(pageCount: number, render: ViewerApi["render"], forms: ReturnType<typeof fake>["api"]) {
    const events = { onPageChange: vi.fn(), onZoomChange: vi.fn(), onFatal: vi.fn(), onFirstPaint: vi.fn() };
    const recorded: FormEdit[] = [];
    const view = new DocumentView(
      { doc: 7, pages: Array.from({ length: pageCount }, () => letter) },
      { render },
      events,
      { forms: { api: forms, record: (e) => recorded.push(e) } },
    );
    document.body.append(view.element);
    view.start();
    return { view, recorded };
  }

  it("draws a page again when a field was changed, also when it is being drawn right now", async () => {
    const { api } = fake([[field({ index: 0, name: "a" })]]);
    let calls = 0;
    const gates: (() => void)[] = [];
    const render: ViewerApi["render"] = (_d, _p, w, h) => {
      calls += 1;
      if (calls === 1) {
        // The first picture is slow: it is on its way when the field is changed.
        return new Promise<ArrayBuffer>((resolve) => gates.push(() => resolve(new ArrayBuffer(w * h * 4))));
      }
      return Promise.resolve(new ArrayBuffer(w * h * 4));
    };
    const { view, recorded } = open(1, render, api);
    await flush();
    expect(calls).toBe(1);
    // The field exists as soon as the page is mounted, before its first picture is there.
    const input = view.element.querySelector<HTMLInputElement>(".form-field");
    expect(input).not.toBeNull();
    (input as HTMLInputElement).value = "typed";
    change(input as HTMLInputElement);
    await flush();
    expect(recorded).toHaveLength(1);
    // The slow picture arrives: it is shown, but it is out of date, so the page is drawn once more.
    gates.forEach((g) => g());
    await flush();
    expect(calls).toBe(2);
    expect(input?.classList.contains("form-pending")).toBe(false);
    // A page called out of date that is not on screen is left alone.
    view.invalidatePage(9);
    await flush();
    expect(calls).toBe(2);
  });

  it("keeps the pages in the document in page order, so the Tab key walks the fields in order", async () => {
    const sliver = { width: 612, height: 400 };
    const { api } = fake(Array.from({ length: 40 }, () => [field({ index: 0, name: "a" })]));
    const events = { onPageChange: vi.fn(), onZoomChange: vi.fn(), onFatal: vi.fn(), onFirstPaint: vi.fn() };
    const view = new DocumentView(
      { doc: 7, pages: Array.from({ length: 40 }, () => sliver) },
      { render: (_d, _p, w, h) => Promise.resolve(new ArrayBuffer(w * h * 4)) },
      events,
      { forms: { api, record: () => undefined } },
    );
    document.body.append(view.element);
    view.start();
    await flush();
    const order = () =>
      [...view.element.querySelectorAll<HTMLElement>(".viewer-page")].map((el) => Number(el.dataset.page));
    expect(order()).toEqual([...order()].sort((a, b) => a - b));
    // Down the document, then back up: the pages that come back are put in front of the later ones.
    view.element.scrollTop = 6000;
    view.element.dispatchEvent(new Event("scroll"));
    await flush();
    view.element.scrollTop = 4800;
    view.element.dispatchEvent(new Event("scroll"));
    await flush();
    expect(order()).toEqual([...order()].sort((a, b) => a - b));
    view.destroy();
  });

  it("has no form layer when the app gives it no way to ask for fields", async () => {
    const events = { onPageChange: vi.fn(), onZoomChange: vi.fn(), onFatal: vi.fn(), onFirstPaint: vi.fn() };
    const view = new DocumentView(
      { doc: 7, pages: [letter] },
      { render: (_d, _p, w, h) => Promise.resolve(new ArrayBuffer(w * h * 4)) },
      events,
    );
    document.body.append(view.element);
    view.start();
    await flush();
    expect(view.element.querySelector(".form-layer")).toBeNull();
  });
});

describe("the view of a document with a form and annotations", () => {
  const note = {
    id: 4,
    kind: "note" as const,
    can: { editText: true, recolour: false, move: true, remove: true, restore: true, locked: false },
    rect: [300, 300, 320, 320] as [number, number, number, number],
    color: [255, 235, 59, 255] as [number, number, number, number],
    text: "a note",
    author: "",
  };

  function open(forms: ReturnType<typeof fake>["api"]) {
    const render = vi.fn((_d: number, _p: number, w: number, h: number) => Promise.resolve(new ArrayBuffer(w * h * 4)));
    const listAnnotations = vi.fn((_doc: number, _page: number) => Promise.resolve([note]));
    const events = { onPageChange: vi.fn(), onZoomChange: vi.fn(), onFatal: vi.fn(), onFirstPaint: vi.fn() };
    const recorded: FormEdit[] = [];
    const view = new DocumentView({ doc: 7, pages: [letter] }, { render, listAnnotations }, events, {
      forms: { api: forms, record: (e) => recorded.push(e) },
    });
    document.body.append(view.element);
    view.start();
    return { view, render, listAnnotations, recorded };
  }

  const layers = (view: DocumentView) =>
    [...(view.element.querySelector(".viewer-page")?.children ?? [])].map((el) => el.className);

  it("puts the fields under the boxes of the annotations and keeps both on the page through a redraw", async () => {
    const { api } = fake([[field({ index: 0, name: "a" })]]);
    const { view, render, listAnnotations } = open(api);
    await flush();
    expect(view.element.querySelectorAll(".form-field")).toHaveLength(1);
    expect(view.element.querySelectorAll(".annot")).toHaveLength(1);
    // The form is mounted first, so the annotation layer lies over it.
    const order = layers(view);
    expect(order.indexOf("form-layer")).toBeGreaterThan(-1);
    expect(order.indexOf("annot-layer")).toBeGreaterThan(order.indexOf("form-layer"));
    expect(render).toHaveBeenCalledTimes(1);
    expect(listAnnotations).toHaveBeenCalledTimes(1);
    const input = view.element.querySelector<HTMLInputElement>(".form-field") as HTMLInputElement;

    // An annotation changed the page: it is drawn again, the annotations are listed again, and
    // the field and the box are both still there afterwards.
    view.invalidatePage(0);
    await flush();
    expect(render).toHaveBeenCalledTimes(2);
    expect(listAnnotations).toHaveBeenCalledTimes(2);
    expect(view.element.querySelectorAll(".form-field")).toHaveLength(1);
    expect(view.element.querySelectorAll(".annot")).toHaveLength(1);
    expect(layers(view)).toEqual(order);

    // A field changed after that: the page is drawn again once more, and the box is still there.
    input.value = "typed";
    change(input);
    await flush();
    expect(render.mock.calls.length).toBeGreaterThanOrEqual(3);
    expect(view.element.querySelectorAll(".annot")).toHaveLength(1);
    expect(view.element.querySelectorAll(".form-field")).toHaveLength(1);
    expect((view.element.querySelector(".form-field") as HTMLInputElement).value).toBe("typed");
    view.destroy();
    expect(document.querySelector(".form-layer")).toBeNull();
    expect(document.querySelector(".annot-layer")).toBeNull();
  });
});

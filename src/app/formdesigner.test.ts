// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyProps, type DesignOp, type DesignWidgetInfo, type Rect } from "../lib/formdesign";
import { DesignLayer } from "../viewer/designlayer";
import { BATCH_REST_MS, FormDesigner, type DesignHost, type DesignTarget } from "./formdesigner";
import type { DesignResult } from "./pageops";

const SIZE = { width: 612, height: 792 };

function item(name: string, rect: Rect, extra: Partial<DesignWidgetInfo> = {}): DesignWidgetInfo {
  return { name, kind: "text", widget: 0, widgets: 1, rect, export: "", editable: true, props: emptyProps(), ...extra };
}

function pointer(type: string, x: number, y: number): Event {
  const init = { clientX: x, clientY: y, button: 0, pointerId: 1, bubbles: true, cancelable: true };
  return typeof PointerEvent === "function"
    ? new PointerEvent(type, init)
    : (Object.assign(new MouseEvent(type, init), { pointerId: 1 }) as unknown as Event);
}

type Applied = { doc: number; ops: DesignOp[]; label: string };

interface FakeView {
  currentPageIndex: number;
  designLayer: DesignLayer | null;
  setDesign: ReturnType<typeof vi.fn>;
  setDesignTool: ReturnType<typeof vi.fn>;
}

interface Setup {
  designer: FormDesigner;
  host: DesignHost;
  page: HTMLElement;
  applied: Applied[];
  apply: ReturnType<typeof vi.fn>;
  list: ReturnType<typeof vi.fn>;
  check: ReturnType<typeof vi.fn>;
  notice: ReturnType<typeof vi.fn>;
  announce: ReturnType<typeof vi.fn>;
  view: FakeView;
  target: DesignTarget;
  /** The page tools: `isSettling` is on while what comes before a tool is going on. */
  pageOps: { canDesign: boolean; applyDesign: ReturnType<typeof vi.fn>; isSettling: boolean };
  listed: DesignWidgetInfo[];
  /** The document in front is another one from now on. */
  front(doc: number): void;
  /** Another document the designer can be opened on, with the same page. */
  second: { target: DesignTarget; view: FakeView; page: HTMLElement };
}

function makeView(page: HTMLElement, pagesOf: () => (typeof SIZE)[]): FakeView {
  const view: FakeView = { currentPageIndex: 0, designLayer: null, setDesign: vi.fn(), setDesignTool: vi.fn() };
  view.setDesignTool.mockImplementation((tool: Parameters<DesignLayer["setTool"]>[0]) => view.designLayer?.setTool(tool));
  view.setDesign.mockImplementation((make: ((pages: (typeof SIZE)[]) => DesignLayer) | null) => {
    view.designLayer?.destroy();
    view.designLayer = make ? make(pagesOf()) : null;
    page.replaceChildren();
    view.designLayer?.mount(0, page);
  });
  return view;
}

function pageElement(id: string): HTMLElement {
  const page = document.getElementById(id) as HTMLElement;
  page.getBoundingClientRect = () => ({ left: 0, top: 0, width: SIZE.width, height: SIZE.height, right: SIZE.width, bottom: SIZE.height, x: 0, y: 0, toJSON: () => ({}) });
  return page;
}

function setup(initial: DesignWidgetInfo[] = []): Setup {
  document.body.innerHTML = `<aside id="design-panel" hidden></aside><div id="page"></div><div id="page2"></div>`;
  const page = pageElement("page");
  const state = { listed: initial };
  const applied: Applied[] = [];
  const view = makeView(page, () => [SIZE, SIZE]);
  const target = { doc: 1, name: "form.pdf", view, history: {}, pages: [SIZE, SIZE] } as unknown as DesignTarget;
  const page2 = pageElement("page2");
  const view2 = makeView(page2, () => [SIZE, SIZE]);
  const target2 = { doc: 2, name: "other.pdf", view: view2, history: {}, pages: [SIZE, SIZE] } as unknown as DesignTarget;
  let frontDoc = 1;
  const check = vi.fn(async () => ["existing"]);
  const list = vi.fn(async (_doc: number, page: number) => (page === 0 ? structuredClone(state.listed) : []));
  const apply = vi.fn(async (doc: number, ops: DesignOp[], label: string): Promise<DesignResult> => {
    applied.push({ doc, ops, label });
    return "done";
  });
  const notice = vi.fn(async () => ({ button: "ok" }));
  const announce = vi.fn();
  const pageOps = { canDesign: true, applyDesign: apply, isSettling: false };
  const host: DesignHost = {
    target: () => (frontDoc === 1 ? target : target2),
    api: { designCheck: check, designList: list } as unknown as DesignHost["api"],
    ops: pageOps as unknown as DesignHost["ops"],
    pageText: async () => ({ text: "", rects: [] }),
    notice,
    announce,
    changed: vi.fn(),
    pendingChanged: vi.fn(),
  };
  const designer = new FormDesigner(document, host);
  return Object.assign(
    { designer, host, page, applied, apply, list, check, notice, announce, view, target, pageOps, front: (d: number) => (frontDoc = d), second: { target: target2, view: view2, page: page2 } },
    state,
  ) as unknown as Setup;
}

const flushPromises = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
};

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const key = (el: Element, name: string, init: KeyboardEventInit = {}) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true, cancelable: true, ...init }));

function placeAt(s: Setup, x: number, y: number): void {
  const capture = s.page.querySelector(".design-capture")!;
  capture.dispatchEvent(pointer("pointerdown", x, y));
  capture.dispatchEvent(pointer("pointerup", x, y));
}

describe("opening", () => {
  it("opens when the engine says the document can take fields, and shows the fields of the page", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    expect(await s.designer.openMode()).toBe(true);
    await flushPromises();
    expect(s.designer.open).toBe(true);
    expect(s.check).toHaveBeenCalledWith(1);
    expect(s.list).toHaveBeenCalledWith(1, 0);
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(1);
    expect(document.getElementById("design-panel")!.hidden).toBe(false);
  });

  it("says why when it cannot, and stays closed", async () => {
    const s = setup();
    s.check.mockRejectedValueOnce({ code: "author_unsupported", message: "this document is encrypted, so fields cannot be added to it" });
    expect(await s.designer.openMode()).toBe(false);
    expect(s.designer.open).toBe(false);
    const call = s.notice.mock.calls[0][0] as { title: string; detail: string };
    expect(call.title).toBe("Fields cannot be added to this document");
    expect(call.detail).toMatch(/encrypted/);
    expect(s.view.setDesign).not.toHaveBeenCalled();
  });

  it("sends what is waiting, and takes the layer off, when it closes", async () => {
    const s = setup();
    await s.designer.openMode();
    s.designer.setTool("text");
    placeAt(s, 250, 420);
    expect(s.designer.hasPending(1)).toBe(true);
    await s.designer.closeMode();
    expect(s.applied).toHaveLength(1);
    expect(s.designer.open).toBe(false);
    expect(s.view.designLayer).toBeNull();
  });
});

describe("placing", () => {
  it("names a new field with a free name, picks it, and sends it after a rest as one batch", async () => {
    const s = setup([item("Text 1", [10, 10, 100, 30])]);
    await s.designer.openMode();
    await flushPromises();
    s.designer.setTool("text");
    placeAt(s, 250, 420);
    // At once the page shows it, and nothing has gone to the engine.
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(2);
    expect(s.apply).not.toHaveBeenCalled();
    expect(s.designer.tool).toBe("select");
    expect(s.designer.isPicked({ name: "Text 2", widget: 0 })).toBe(true);
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.applied).toHaveLength(1);
    expect(s.applied[0].doc).toBe(1);
    expect(s.applied[0].label).toBe("Add a text field");
    expect(s.applied[0].ops).toEqual([
      { op: "add", field: { name: "Text 2", kind: "text", props: {}, widgets: [{ page: 0, rect: [170, 409, 330, 431], export: "" }] } },
    ]);
    expect(s.designer.hasPending(1)).toBe(false);
  });

  it("gives a drop-down some choices, because a list with none is refused", async () => {
    const s = setup();
    await s.designer.openMode();
    s.designer.setTool("combo");
    placeAt(s, 250, 420);
    await s.designer.flush(1);
    const op = s.applied[0].ops[0];
    expect(op.op === "add" && op.field.props.choices).toEqual(["Option 1", "Option 2", "Option 3"]);
  });

  it("adds the clicks of the radio tool to one group until Escape", async () => {
    const s = setup();
    await s.designer.openMode();
    s.designer.setTool("radio");
    for (const x of [100, 200, 300]) placeAt(s, x, 420);
    expect(s.designer.tool).toBe("radio");
    await s.designer.flush(1);
    const ops = s.applied[0].ops;
    expect(ops.map((o) => o.op)).toEqual(["add", "addWidget", "addWidget"]);
    const first = ops[0];
    expect(first.op === "add" && first.field.name).toBe("Group 1");
    expect(first.op === "add" && first.field.widgets[0].export).toBe("Choice1");
    expect(ops[1].op === "addWidget" && ops[1].widget.export).toBe("Choice2");
    expect(ops[2].op === "addWidget" && ops[2].name).toBe("Group 1");
    expect(s.designer.handleEscape()).toBe(true);
    expect(s.designer.tool).toBe("select");
  });

  it("places a field in the middle of the page when the tool button is pressed with the keyboard", async () => {
    const s = setup();
    await s.designer.openMode();
    await flushPromises();
    s.designer.setTool("text", { place: true });
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(1);
    // The default size, around the middle of the page, and the reader is told where it is.
    await s.designer.flush(1);
    const op = s.applied[0].ops[0];
    expect(op.op === "add" && op.field.widgets[0].rect).toEqual([226, 385, 386, 407]);
    expect(s.announce.mock.calls.map((c) => c[0] as string).join(" ")).toMatch(/Added a text field\. Text 1 is on page 1, 226 points from the left and 385 from the top, 160 points wide and 22 tall/);
    // A second one does not land on the first.
    s.designer.setTool("text", { place: true });
    await s.designer.flush(1);
    const second = s.applied[1].ops[0];
    expect(second.op === "add" && second.field.widgets[0].rect).not.toEqual([226, 385, 386, 407]);
    // A pointer asks for the tool and nothing more.
    s.designer.setTool("text");
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(2);
    expect(s.designer.tool).toBe("text");
  });

  it("refuses a field on a page that has all it can have, and says so", async () => {
    const full = Array.from({ length: 1000 }, (_, i) => item(`f${i}`, [(i % 40) * 14, Math.floor(i / 40) * 20, (i % 40) * 14 + 12, Math.floor(i / 40) * 20 + 16]));
    const s = setup(full);
    await s.designer.openMode();
    await flushPromises();
    s.designer.setTool("text");
    placeAt(s, 300, 700);
    await s.designer.flush(1);
    expect(s.apply).not.toHaveBeenCalled();
    expect(s.announce).toHaveBeenCalledWith("A page can have at most 1000 fields.");
  });
});

describe("moving", () => {
  it("makes a run of arrow keys one change, and tells where the box is", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    const box = () => s.page.querySelector<HTMLElement>(".design-box")!;
    for (let i = 0; i < 5; i++) key(box(), "ArrowRight");
    expect(s.apply).not.toHaveBeenCalled();
    await s.designer.flush(1);
    expect(s.applied).toHaveLength(1);
    expect(s.applied[0].ops).toEqual([{ op: "move", name: "a", widget: 0, page: 0, rect: [105, 100, 205, 150] }]);
    expect(s.announce).toHaveBeenLastCalledWith("a is on page 1, 105 points from the left and 100 from the top, 100 points wide and 50 tall.");
  });

  it("deletes the picked field, or one button of a group", async () => {
    const s = setup([item("a", [100, 100, 200, 150]), item("size", [10, 10, 24, 24], { kind: "radio", widget: 0, widgets: 2 }), item("size", [40, 10, 54, 24], { kind: "radio", widget: 1, widgets: 2 })]);
    await s.designer.openMode();
    await flushPromises();
    const boxes = () => [...s.page.querySelectorAll<HTMLElement>(".design-box")];
    const [a, first] = boxes();
    key(a, "Delete");
    key(first, "Delete");
    await s.designer.flush(1);
    expect(s.applied[0].ops).toEqual([
      { op: "remove", name: "a" },
      { op: "removeWidget", name: "size", widget: 0 },
    ]);
    expect(s.applied[0].label).toBe("Edit the form");
  });
});

describe("the panel", () => {
  async function picked(): Promise<Setup> {
    const s = setup([item("a", [100, 100, 200, 150]), item("b", [100, 200, 200, 250])]);
    await s.designer.openMode();
    await flushPromises();
    s.page.querySelector<HTMLElement>(".design-box")!.dispatchEvent(pointer("pointerdown", 150, 120));
    return s;
  }

  const panel = () => document.getElementById("design-panel") as HTMLElement;
  const control = (id: RegExp) => [...panel().querySelectorAll<HTMLInputElement>("input, select, textarea")].find((c) => id.test(c.id))!;
  const change = (id: RegExp, value: string | boolean) => {
    const c = control(id);
    if (typeof value === "boolean") c.checked = value;
    else c.value = value;
    c.dispatchEvent(new Event("change", { bubbles: true }));
  };

  it("shows the picked field", async () => {
    await picked();
    const name = panel().querySelector<HTMLInputElement>('input[id$="-name"]')!;
    expect(name.value).toBe("a");
    expect(panel().querySelector("h2")!.textContent).toBe("Text field");
    expect(panel().querySelectorAll(".design-order-item")).toHaveLength(2);
  });

  it("renames a field to a free name and sends one change", async () => {
    const s = await picked();
    change(/-name$/, "first_name");
    await s.designer.flush(1);
    expect(s.applied[0].ops).toHaveLength(1);
    const op = s.applied[0].ops[0];
    expect(op.op === "update" && op.name).toBe("a");
    expect(op.op === "update" && op.newName).toBe("first_name");
  });

  it("refuses a name that is taken or has a dot, tells why, and sends nothing", async () => {
    const s = await picked();
    change(/-name$/, "b");
    expect(panel().querySelector(".design-error")!.textContent).toMatch(/already a field called/);
    expect((panel().querySelector(".design-error") as HTMLElement).hidden).toBe(false);
    change(/-name$/, "x.y");
    expect(panel().querySelector(".design-error")!.textContent).toMatch(/dot/);
    await s.designer.flush(1);
    expect(s.apply).not.toHaveBeenCalled();
  });

  it("does not keep a refused name in the field once the reader leaves it", async () => {
    const s = await picked();
    const name = control(/-name$/);
    name.focus();
    change(/-name$/, "b");
    expect(name.value).toBe("b");
    name.blur();
    expect(name.value).toBe("a");
    // The words that say why stay.
    expect((panel().querySelector(".design-error") as HTMLElement).hidden).toBe(false);
    await s.designer.flush(1);
    expect(s.apply).not.toHaveBeenCalled();
  });

  it("changes the properties and refuses what the font cannot draw", async () => {
    const s = await picked();
    change(/-tooltip$/, "Your name");
    change(/-required$/, true);
    change(/-maxLen$/, "12");
    await s.designer.flush(1);
    const op = s.applied[0].ops.find((o) => o.op === "update");
    expect(op && op.op === "update" && op.props).toMatchObject({ tooltip: "Your name", required: true, maxLen: 12 });
    change(/-defaultText$/, "price €5");
    expect(panel().querySelector(".design-error")!.textContent).toMatch(/cannot be drawn/);
  });

  it("refuses before it sends what the engine would refuse, so that a move made first is not rolled back with it", async () => {
    const s = await picked();
    key(s.page.querySelector(".design-box")!, "ArrowRight");
    change(/-tooltip$/, "x".repeat(600));
    expect(panel().querySelector(".design-error")!.textContent).toMatch(/tooltip is too long/);
    change(/-tooltip$/, "");
    change(/-maxLen$/, "40000");
    expect(panel().querySelector(".design-error")!.textContent).toMatch(/at most 32767/);
    await s.designer.flush(1);
    // Only the move went, and nothing for the two refused changes.
    expect(s.applied).toHaveLength(1);
    expect(s.applied[0].ops.map((o) => o.op)).toEqual(["move"]);
  });

  it("leaves a size and a limit that another program chose alone, and does not ask about them again", async () => {
    const odd = item("big", [100, 100, 200, 150], { props: { ...emptyProps(), fontSize: 200, maxLen: 40000 } });
    const s = setup([odd]);
    await s.designer.openMode();
    await flushPromises();
    s.page.querySelector<HTMLElement>(".design-box")!.dispatchEvent(pointer("pointerdown", 150, 120));
    // The size is shown as it is, not as "fit the box".
    expect(control(/-fontSize$/).value).toBe("200");
    change(/-tooltip$/, "A tooltip");
    expect((panel().querySelector(".design-error") as HTMLElement).hidden).toBe(true);
    await s.designer.flush(1);
    const op = s.applied[0].ops[0];
    expect(op.op === "update" && op.props).toMatchObject({ tooltip: "A tooltip", fontSize: 200, maxLen: 40000 });
  });

  it("puts the fields of the page in a new Tab order, and keeps the keyboard on the button it was on", async () => {
    const s = await picked();
    const down = panel().querySelectorAll<HTMLButtonElement>(".design-order-down")[0];
    down.focus();
    down.click();
    // The list is made again; the focus is on the same field's button.
    const now = document.activeElement as HTMLElement;
    expect(now.closest(".design-order-item")?.getAttribute("data-name")).toBe("a");
    await s.designer.flush(1);
    expect(s.applied[0].ops).toEqual([
      {
        op: "reorder",
        page: 0,
        order: [
          { name: "b", widget: 0 },
          { name: "a", widget: 0 },
        ],
      },
    ]);
    expect(s.announce).toHaveBeenLastCalledWith("a is number 2 of 2 in the Tab order.");
  });

  it("takes a field it cannot change as it is", async () => {
    const s = setup([item("old", [100, 100, 200, 150], { editable: false })]);
    await s.designer.openMode();
    await flushPromises();
    s.page.querySelector<HTMLElement>(".design-box")!.dispatchEvent(pointer("pointerdown", 150, 120));
    expect(panel().textContent).toMatch(/cannot be changed here/);
    change(/-tooltip$/, "x");
    await s.designer.flush(1);
    expect(s.apply).not.toHaveBeenCalled();
  });

  it("keeps a choice that is picked when the list of choices is edited and the choice is still in it", async () => {
    const list = item("pick", [100, 100, 200, 150], { kind: "list", props: { ...emptyProps(), choices: ["a", "b", "c"], defaultValue: "b" } });
    const s = setup([list]);
    await s.designer.openMode();
    await flushPromises();
    s.page.querySelector<HTMLElement>(".design-box")!.dispatchEvent(pointer("pointerdown", 150, 120));
    expect(control(/-defaultChoice$/).value).toBe("b");
    change(/-choices$/, "a\nb\nd");
    await s.designer.flush(1);
    const op = s.applied[0].ops[0];
    expect(op.op === "update" && op.props.defaultValue).toBe("b");
    expect(op.op === "update" && op.props.choices).toEqual(["a", "b", "d"]);
  });
});

describe("when a change is made while a batch is on its way", () => {
  it("waits behind the batch and is sent after it, and is not lost", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    let finish!: () => void;
    s.apply.mockImplementationOnce(
      (doc: number, ops: DesignOp[], label: string) =>
        new Promise<DesignResult>((resolve) => {
          s.applied.push({ doc, ops, label });
          finish = () => resolve("done");
        }),
    );
    const box = () => s.page.querySelector<HTMLElement>(".design-box")!;
    key(box(), "ArrowRight");
    const sending = s.designer.flush(1);
    await flushPromises();
    expect(s.applied).toHaveLength(1);
    // The reader renames the field while the first batch is on its way.
    box().dispatchEvent(pointer("pointerdown", 150, 120));
    const name = document.querySelector<HTMLInputElement>('#design-panel input[id$="-name"]')!;
    name.value = "renamed";
    name.dispatchEvent(new Event("change", { bubbles: true }));
    expect((document.querySelector("#design-panel .design-error") as HTMLElement).hidden).toBe(true);
    finish();
    await sending;
    expect(s.applied).toHaveLength(2);
    const second = s.applied[1].ops[0];
    expect(second.op === "update" && second.newName).toBe("renamed");
    expect(s.designer.hasPending(1)).toBe(false);
  });

  it("tells the reader, and shows the field as it is, when another tool is working on the pages", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    s.page.querySelector<HTMLElement>(".design-box")!.dispatchEvent(pointer("pointerdown", 150, 120));
    s.designer.setBusy(true);
    const panel = document.getElementById("design-panel") as HTMLElement;
    // The controls do not take a change.
    expect([...panel.querySelectorAll<HTMLInputElement>('input[id$="-name"], input[id$="-tooltip"]')].every((c) => c.disabled)).toBe(true);
    expect(panel.querySelector<HTMLButtonElement>(".design-remove")!.disabled).toBe(true);
    key(s.page.querySelector(".design-box")!, "ArrowRight");
    expect(s.announce).toHaveBeenLastCalledWith("The pages are being changed. Try again in a moment.");
    s.designer.setBusy(false);
    await s.designer.flush(1);
    expect(s.apply).not.toHaveBeenCalled();
  });
});

describe("the batch goes to the document it was made for", () => {
  it("is sent for the first document when the reader goes to another tab within the rest", async () => {
    const s = setup();
    await s.designer.openMode();
    await flushPromises();
    s.designer.setTool("text");
    placeAt(s, 250, 420);
    expect(s.designer.hasPending(1)).toBe(true);
    // The other tab is in front now, and the designer follows it before the rest is over.
    s.front(2);
    void s.designer.followTab();
    await flushPromises();
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.applied.map((a) => a.doc)).toEqual([1]);
    expect(s.applied[0].ops.map((o) => o.op)).toEqual(["add"]);
    expect(s.designer.hasPending(1)).toBe(false);
    expect(s.designer.hasPending(2)).toBe(false);
  });

  it("is sent for the first document when the rest is over while another tab is in front", async () => {
    const s = setup();
    await s.designer.openMode();
    await flushPromises();
    s.designer.setTool("text");
    placeAt(s, 250, 420);
    s.front(2);
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.applied.map((a) => a.doc)).toEqual([1]);
  });

  it("keeps the changes of each document apart", async () => {
    const s = setup();
    await s.designer.openMode();
    await flushPromises();
    s.designer.setTool("text");
    placeAt(s, 250, 420);
    s.front(2);
    await s.designer.followTab();
    await flushPromises();
    s.designer.setTool("checkbox");
    const capture = s.second.page.querySelector(".design-capture")!;
    capture.dispatchEvent(pointer("pointerdown", 100, 100));
    capture.dispatchEvent(pointer("pointerup", 100, 100));
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.applied.map((a) => [a.doc, a.ops[0].op === "add" && a.ops[0].field.kind])).toEqual([
      [1, "text"],
      [2, "checkbox"],
    ]);
  });
});

describe("when the batch does not reach the engine", () => {
  it("keeps it and sends it again when another tool was working", async () => {
    const s = setup();
    await s.designer.openMode();
    await flushPromises();
    s.designer.setTool("text");
    placeAt(s, 250, 420);
    s.apply.mockResolvedValueOnce("busy" as DesignResult);
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.apply).toHaveBeenCalledTimes(1);
    expect(s.designer.hasPending(1)).toBe(true);
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(1);
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.apply).toHaveBeenCalledTimes(2);
    expect(s.applied).toHaveLength(1);
    expect(s.applied[0].ops.map((o) => o.op)).toEqual(["add"]);
    expect(s.designer.hasPending(1)).toBe(false);
  });

  it("keeps it, without sending it again by itself, when what comes before the change was not settled", async () => {
    const s = setup();
    await s.designer.openMode();
    await flushPromises();
    s.designer.setTool("text");
    placeAt(s, 250, 420);
    s.apply.mockResolvedValueOnce("unsettled" as DesignResult);
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.designer.hasPending(1)).toBe(true);
    vi.advanceTimersByTime(10 * BATCH_REST_MS);
    await flushPromises();
    expect(s.apply).toHaveBeenCalledTimes(1);
    // The next time the document is looked at, it goes.
    await expect(s.designer.flush(1)).resolves.toBe(true);
    expect(s.applied).toHaveLength(1);
  });

  it("forgets it when the document is gone", async () => {
    const s = setup();
    await s.designer.openMode();
    await flushPromises();
    s.designer.setTool("text");
    placeAt(s, 250, 420);
    s.apply.mockResolvedValueOnce("gone" as DesignResult);
    await expect(s.designer.flush(1)).resolves.toBe(false);
    expect(s.designer.hasPending(1)).toBe(false);
  });

  it("sends a long batch as batches of at most a hundred changes, in order", async () => {
    const many = Array.from({ length: 130 }, (_, i) => item(`f${i}`, [(i % 20) * 28, Math.floor(i / 20) * 30, (i % 20) * 28 + 24, Math.floor(i / 20) * 30 + 20]));
    const s = setup(many);
    await s.designer.openMode();
    await flushPromises();
    for (const box of [...s.page.querySelectorAll<HTMLElement>(".design-box")]) key(box, "Delete");
    await s.designer.flush(1);
    expect(s.applied.map((a) => a.ops.length)).toEqual([100, 30]);
    expect(s.applied[0].ops[0]).toEqual({ op: "remove", name: "f0" });
    expect(s.applied[1].ops[0]).toEqual({ op: "remove", name: "f100" });
  });
});

describe("while a file is written", () => {
  it("sends nothing until the file is done, and keeps what was changed meanwhile", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    const release = s.designer.hold(1);
    key(s.page.querySelector(".design-box")!, "ArrowRight");
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.apply).not.toHaveBeenCalled();
    expect(s.designer.hasPending(1)).toBe(true);
    release();
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.applied).toHaveLength(1);
    expect(s.designer.hasPending(1)).toBe(false);
  });
});

describe("when the engine refuses", () => {
  it("reads the fields again, and drops what it did not make", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    s.apply.mockResolvedValueOnce("refused" as DesignResult);
    key(s.page.querySelector<HTMLElement>(".design-box")!, "Delete");
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(0);
    const before = s.list.mock.calls.length;
    await s.designer.flush(1);
    await flushPromises();
    expect(s.list.mock.calls.length).toBeGreaterThan(before);
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(1);
    expect(s.designer.hasPending(1)).toBe(false);
  });

  it("forgets what it made on the pages that are not on the screen", async () => {
    const s = setup();
    await s.designer.openMode();
    await flushPromises();
    // A second page is shown, a field is placed on it, and the page leaves the screen.
    const extra = document.createElement("div");
    extra.getBoundingClientRect = s.page.getBoundingClientRect;
    s.view.designLayer!.mount(1, extra);
    s.designer.setTool("text");
    const capture = extra.querySelector(".design-capture")!;
    capture.dispatchEvent(pointer("pointerdown", 250, 420));
    capture.dispatchEvent(pointer("pointerup", 250, 420));
    await flushPromises();
    expect(s.designer.modelFor(1)!.get(1)).toHaveLength(1);
    s.view.designLayer!.unmount(1);
    s.apply.mockResolvedValueOnce("refused" as DesignResult);
    await s.designer.flush(1);
    await flushPromises();
    // What the engine did not make is not in the model, mounted or not.
    expect([...s.designer.modelFor(1)!.values()].flat()).toEqual([]);
  });

  it("does not wait for itself when a change under way asks for the changes to be sent", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    s.apply.mockImplementationOnce(async () => {
      // The page tools settle the document first, which asks the designer to send what it holds.
      s.pageOps.isSettling = true;
      try {
        await expect(s.designer.flush(1)).resolves.toBe(true);
      } finally {
        s.pageOps.isSettling = false;
      }
      return "done" as DesignResult;
    });
    key(s.page.querySelector<HTMLElement>(".design-box")!, "Delete");
    await expect(s.designer.flush(1)).resolves.toBe(true);
  });

  it("makes a caller from outside wait for the batch that is on its way", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    let finish!: () => void;
    s.apply.mockImplementationOnce(() => new Promise<DesignResult>((resolve) => (finish = () => resolve("done"))));
    key(s.page.querySelector<HTMLElement>(".design-box")!, "Delete");
    const first = s.designer.flush(1);
    await flushPromises();
    let outside = false;
    void s.designer.flush(1).then(() => (outside = true));
    await flushPromises();
    expect(outside).toBe(false);
    finish();
    await first;
    await flushPromises();
    expect(outside).toBe(true);
  });
});

describe("the keyboard", () => {
  it("is given back to the picked box after the document was loaded again", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    const box = () => s.page.querySelector<HTMLElement>(".design-box")!;
    box().focus();
    key(box(), "Enter");
    s.apply.mockImplementationOnce(async (doc: number, ops: DesignOp[], label: string) => {
      s.applied.push({ doc, ops, label });
      // The pages are laid out again: the layer is made again and the boxes with it.
      const make = s.view.setDesign.mock.calls[0][0] as (p: unknown[]) => DesignLayer;
      const old = s.view.designLayer!;
      old.destroy();
      s.view.designLayer = make([SIZE, SIZE]);
      s.view.designLayer.inherit(old);
      s.page.replaceChildren();
      s.view.designLayer.mount(0, s.page);
      return "done" as DesignResult;
    });
    key(box(), "ArrowDown");
    await s.designer.flush(1);
    await flushPromises();
    expect(document.activeElement).toBe(s.page.querySelector(".design-box"));
  });
});

describe("the document changing under the designer", () => {
  it("reads the fields of a page again after a step back", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    const before = s.list.mock.calls.length;
    s.listed.length = 0;
    s.designer.pagesChanged(1);
    // What the window knew is about other pages now.
    expect([...s.designer.modelFor(1)!.values()].flat()).toEqual([]);
    // The view lays its pages out again: the layer is made again and asks for the widgets.
    s.view.designLayer?.destroy();
    s.view.designLayer = (s.view.setDesign.mock.calls[0][0] as (p: unknown[]) => DesignLayer)([SIZE]);
    s.page.replaceChildren();
    s.view.designLayer.mount(0, s.page);
    await flushPromises();
    expect(s.list.mock.calls.length).toBeGreaterThan(before);
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(0);
  });

  it("shows no stale boxes after a step back when the engine cannot be asked", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    s.list.mockRejectedValue({ code: "io", message: "no" });
    s.designer.pagesChanged(1);
    s.view.designLayer?.destroy();
    s.view.designLayer = (s.view.setDesign.mock.calls[0][0] as (p: unknown[]) => DesignLayer)([SIZE]);
    s.page.replaceChildren();
    s.view.designLayer.mount(0, s.page);
    await flushPromises();
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(0);
  });
});

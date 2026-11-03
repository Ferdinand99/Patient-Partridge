// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { emptyProps, type DesignOp, type DesignWidgetInfo, type Rect } from "../lib/formdesign";
import { DesignLayer } from "../viewer/designlayer";
import { BATCH_REST_MS, FormDesigner, type DesignHost, type DesignTarget } from "./formdesigner";

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

interface Setup {
  designer: FormDesigner;
  host: DesignHost;
  page: HTMLElement;
  applied: { ops: DesignOp[]; label: string }[];
  apply: ReturnType<typeof vi.fn>;
  list: ReturnType<typeof vi.fn>;
  check: ReturnType<typeof vi.fn>;
  notice: ReturnType<typeof vi.fn>;
  view: { designLayer: DesignLayer | null; setDesign: ReturnType<typeof vi.fn> };
  target: DesignTarget;
  listed: DesignWidgetInfo[];
}

function setup(initial: DesignWidgetInfo[] = []): Setup {
  document.body.innerHTML = `<aside id="design-panel" hidden></aside><div id="page"></div>`;
  const page = document.getElementById("page") as HTMLElement;
  page.getBoundingClientRect = () => ({ left: 0, top: 0, width: SIZE.width, height: SIZE.height, right: SIZE.width, bottom: SIZE.height, x: 0, y: 0, toJSON: () => ({}) });
  const state = { listed: initial };
  const applied: Setup["applied"] = [];
  const view: {
    currentPageIndex: number;
    designLayer: DesignLayer | null;
    setDesign: ReturnType<typeof vi.fn>;
    setDesignTool: ReturnType<typeof vi.fn>;
  } = { currentPageIndex: 0, designLayer: null, setDesign: vi.fn(), setDesignTool: vi.fn() };
  view.setDesignTool.mockImplementation((tool: Parameters<DesignLayer["setTool"]>[0]) => view.designLayer?.setTool(tool));
  view.setDesign.mockImplementation((make: ((pages: typeof SIZE[]) => DesignLayer) | null) => {
    view.designLayer?.destroy();
    view.designLayer = make ? make([SIZE]) : null;
    page.replaceChildren();
    view.designLayer?.mount(0, page);
  });
  const target = { doc: 1, name: "form.pdf", view, history: {}, pages: [SIZE] } as unknown as DesignTarget;
  const check = vi.fn(async () => ["existing"]);
  const list = vi.fn(async () => structuredClone(state.listed));
  const apply = vi.fn(async (ops: DesignOp[], label: string) => {
    applied.push({ ops, label });
    return true;
  });
  const notice = vi.fn(async () => ({ button: "ok" }));
  const host: DesignHost = {
    target: () => target,
    api: { designCheck: check, designList: list } as unknown as DesignHost["api"],
    ops: { canDesign: true, applyDesign: apply } as unknown as DesignHost["ops"],
    pageText: async () => ({ text: "", rects: [] }),
    notice,
    announce: vi.fn(),
    changed: vi.fn(),
    pendingChanged: vi.fn(),
  };
  const designer = new FormDesigner(document, host);
  return Object.assign({ designer, host, page, applied, apply, list, check, notice, view, target }, state) as unknown as Setup;
}

const flushPromises = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
};

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

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
    s.page.querySelector(".design-capture")!.dispatchEvent(pointer("pointerdown", 250, 420));
    s.page.querySelector(".design-capture")!.dispatchEvent(pointer("pointerup", 250, 420));
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
    const capture = s.page.querySelector(".design-capture")!;
    capture.dispatchEvent(pointer("pointerdown", 250, 420));
    capture.dispatchEvent(pointer("pointerup", 250, 420));
    // At once the page shows it, and nothing has gone to the engine.
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(2);
    expect(s.apply).not.toHaveBeenCalled();
    expect(s.designer.tool).toBe("select");
    expect(s.designer.isPicked({ name: "Text 2", widget: 0 })).toBe(true);
    vi.advanceTimersByTime(BATCH_REST_MS + 1);
    await flushPromises();
    expect(s.applied).toHaveLength(1);
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
    const capture = s.page.querySelector(".design-capture")!;
    capture.dispatchEvent(pointer("pointerdown", 250, 420));
    capture.dispatchEvent(pointer("pointerup", 250, 420));
    await s.designer.flush(1);
    const op = s.applied[0].ops[0];
    expect(op.op === "add" && op.field.props.choices).toEqual(["Option 1", "Option 2", "Option 3"]);
  });

  it("adds the clicks of the radio tool to one group until Escape", async () => {
    const s = setup();
    await s.designer.openMode();
    s.designer.setTool("radio");
    const capture = () => s.page.querySelector(".design-capture")!;
    for (const x of [100, 200, 300]) {
      capture().dispatchEvent(pointer("pointerdown", x, 420));
      capture().dispatchEvent(pointer("pointerup", x, 420));
    }
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
});

describe("moving", () => {
  it("makes a run of arrow keys one change", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    const box = () => s.page.querySelector<HTMLElement>(".design-box")!;
    for (let i = 0; i < 5; i++) {
      box().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    }
    expect(s.apply).not.toHaveBeenCalled();
    await s.designer.flush(1);
    expect(s.applied).toHaveLength(1);
    expect(s.applied[0].ops).toEqual([{ op: "move", name: "a", widget: 0, page: 0, rect: [105, 100, 205, 150] }]);
  });

  it("deletes the picked field, or one button of a group", async () => {
    const s = setup([item("a", [100, 100, 200, 150]), item("size", [10, 10, 24, 24], { kind: "radio", widget: 0, widgets: 2 }), item("size", [40, 10, 54, 24], { kind: "radio", widget: 1, widgets: 2 })]);
    await s.designer.openMode();
    await flushPromises();
    const boxes = () => [...s.page.querySelectorAll<HTMLElement>(".design-box")];
    const [a, first] = boxes();
    a.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true, cancelable: true }));
    first.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true, cancelable: true }));
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
  const change = (id: RegExp, value: string | boolean) => {
    const control = [...panel().querySelectorAll<HTMLInputElement>("input, select, textarea")].find((c) => id.test(c.id))!;
    if (typeof value === "boolean") control.checked = value;
    else control.value = value;
    control.dispatchEvent(new Event("change", { bubbles: true }));
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

  it("puts the fields of the page in a new Tab order", async () => {
    const s = await picked();
    panel().querySelectorAll<HTMLButtonElement>(".design-order-down")[0].click();
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
});

describe("when the engine refuses", () => {
  it("reads the fields again, and drops what it did not make", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    s.apply.mockResolvedValueOnce(false);
    s.page.querySelector<HTMLElement>(".design-box")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true, cancelable: true }));
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(0);
    const before = s.list.mock.calls.length;
    await s.designer.flush(1);
    await flushPromises();
    expect(s.list.mock.calls.length).toBeGreaterThan(before);
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(1);
    expect(s.designer.hasPending(1)).toBe(false);
  });

  it("does not wait for itself when a change under way asks for the changes to be sent", async () => {
    const s = setup([item("a", [100, 100, 200, 150])]);
    await s.designer.openMode();
    await flushPromises();
    s.apply.mockImplementationOnce(async () => {
      // The page tools settle the document first, which asks the designer to send what it holds.
      await s.designer.flush(1);
      return true;
    });
    s.page.querySelector<HTMLElement>(".design-box")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true, cancelable: true }));
    await expect(s.designer.flush(1)).resolves.toBe(true);
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
    // The view lays its pages out again: the layer is made again and asks for the widgets.
    s.view.designLayer?.destroy();
    s.view.designLayer = (s.view.setDesign.mock.calls[0][0] as (p: unknown[]) => DesignLayer)([SIZE]);
    s.page.replaceChildren();
    s.view.designLayer.mount(0, s.page);
    await flushPromises();
    expect(s.list.mock.calls.length).toBeGreaterThan(before);
    expect(s.page.querySelectorAll(".design-box")).toHaveLength(0);
  });
});

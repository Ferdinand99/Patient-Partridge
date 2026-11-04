// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { emptyProps, noLines, type DesignWidgetInfo, type Rect } from "../lib/formdesign";
import { DesignLayer, snapPoint, type DesignLayerEvents } from "./designlayer";

const SIZE = { width: 612, height: 792 };

function item(name: string, rect: Rect, extra: Partial<DesignWidgetInfo> = {}): DesignWidgetInfo {
  return { name, kind: "text", widget: 0, widgets: 1, rect, export: "", editable: true, props: emptyProps(), ...extra };
}

function pointer(type: string, x: number, y: number, extra: Partial<PointerEventInit> = {}): PointerEvent {
  const init = { clientX: x, clientY: y, button: 0, pointerId: 1, bubbles: true, cancelable: true, ...extra };
  return typeof PointerEvent === "function"
    ? new PointerEvent(type, init)
    : (Object.assign(new MouseEvent(type, init), { pointerId: 1 }) as PointerEvent);
}

let page: HTMLElement;
let events: DesignLayerEvents & {
  place: ReturnType<typeof vi.fn>;
  select: ReturnType<typeof vi.fn>;
  transform: ReturnType<typeof vi.fn>;
  remove: ReturnType<typeof vi.fn>;
  edit: ReturnType<typeof vi.fn>;
  pageReady: ReturnType<typeof vi.fn>;
};
let layer: DesignLayer;

/** A page that is 612 by 792 pixels at the corner of the window: a pixel is a point. */
function makePage(): HTMLElement {
  const el = document.createElement("div");
  el.getBoundingClientRect = () => ({ left: 0, top: 0, width: SIZE.width, height: SIZE.height, right: SIZE.width, bottom: SIZE.height, x: 0, y: 0, toJSON: () => ({}) });
  document.body.append(el);
  return el;
}

beforeEach(() => {
  document.body.replaceChildren();
  page = makePage();
  events = {
    pageReady: vi.fn(),
    place: vi.fn(),
    select: vi.fn(),
    transform: vi.fn(),
    remove: vi.fn(),
    edit: vi.fn(),
    textLines: vi.fn(async () => noLines()),
  } as unknown as typeof events;
  layer = new DesignLayer([SIZE], events);
  layer.mount(0, page);
});

const boxes = () => [...page.querySelectorAll<HTMLElement>(".design-box")];

describe("the boxes", () => {
  it("asks for the widgets of a page that is mounted, and shows them where the engine says", () => {
    expect(events.pageReady).toHaveBeenCalledWith(0);
    layer.show(0, [item("Name", [61.2, 79.2, 306, 118.8]), item("size", [10, 10, 24, 24], { kind: "radio", widget: 1, widgets: 3 })]);
    const [a, b] = boxes();
    expect(a.style.left).toBe("10%");
    expect(a.style.top).toBe("10%");
    expect(a.style.width).toBe("40%");
    expect(a.style.height).toBe("5%");
    expect(a.getAttribute("aria-label")).toBe("Name, text field");
    expect(b.getAttribute("aria-label")).toBe("size, radio button, button 2 of 3");
    expect(a.textContent).toBe("Name");
  });

  it("shows a name as text, never as markup", () => {
    layer.show(0, [item("<img src=x onerror=alert(1)>", [10, 10, 100, 30])]);
    expect(page.querySelector("img")).toBeNull();
    expect(boxes()[0].textContent).toBe("<img src=x onerror=alert(1)>");
  });

  it("marks a field the designer cannot change", () => {
    layer.show(0, [item("old", [10, 10, 100, 30], { editable: false })]);
    expect(boxes()[0].classList.contains("design-box-locked")).toBe(true);
    expect(boxes()[0].getAttribute("aria-label")).toMatch(/cannot be changed here/);
  });

  it("takes its boxes away with the page", () => {
    layer.show(0, [item("a", [10, 10, 100, 30])]);
    layer.unmount(0);
    expect(page.querySelector(".design-layer")).toBeNull();
  });
});

describe("picking and moving", () => {
  beforeEach(() => layer.show(0, [item("a", [100, 100, 200, 150]), item("b", [100, 300, 200, 340])]));

  it("picks a box on a press and shows handles on it", () => {
    const first = boxes()[0];
    first.dispatchEvent(pointer("pointerdown", 150, 120));
    expect(events.select).toHaveBeenCalledWith(0, { name: "a", widget: 0 });
    // The box that was pressed is the box that is there afterwards: a drag can go on.
    expect(boxes()[0]).toBe(first);
    expect(boxes()[0].querySelectorAll(".design-handle")).toHaveLength(8);
    expect(boxes()[1].querySelectorAll(".design-handle")).toHaveLength(0);
    expect(boxes()[0].getAttribute("aria-current")).toBe("true");
  });

  it("moves a box by the distance the pointer went, and tells it once, at the end", () => {
    const box = boxes()[0];
    box.dispatchEvent(pointer("pointerdown", 150, 120));
    box.dispatchEvent(pointer("pointermove", 190, 160));
    expect(events.transform).not.toHaveBeenCalled();
    // While it is dragged the box shows where it will be.
    expect(box.style.left).toBe(`${((140 / 612) * 100).toFixed(3)}%`);
    box.dispatchEvent(pointer("pointerup", 190, 160));
    expect(events.transform).toHaveBeenCalledTimes(1);
    expect(events.transform).toHaveBeenCalledWith(0, { name: "a", widget: 0 }, [140, 140, 240, 190]);
  });

  it("does not move a box for a press that did not go anywhere", () => {
    const box = boxes()[0];
    box.dispatchEvent(pointer("pointerdown", 150, 120));
    box.dispatchEvent(pointer("pointermove", 151, 121));
    box.dispatchEvent(pointer("pointerup", 151, 121));
    expect(events.transform).not.toHaveBeenCalled();
  });

  it("snaps to the edge of the page within six pixels, and Alt turns that off", () => {
    const first = boxes()[0];
    first.dispatchEvent(pointer("pointerdown", 150, 120));
    // The box would start 3 points from the left edge of the page.
    first.dispatchEvent(pointer("pointermove", 53, 120));
    first.dispatchEvent(pointer("pointerup", 53, 120));
    expect(events.transform).toHaveBeenLastCalledWith(0, { name: "a", widget: 0 }, [0, 100, 100, 150]);
    layer.show(0, [item("a", [100, 100, 200, 150]), item("b", [100, 300, 200, 340])]);
    const again = boxes()[0];
    again.dispatchEvent(pointer("pointerdown", 150, 120));
    again.dispatchEvent(pointer("pointermove", 53, 120, { altKey: true }));
    again.dispatchEvent(pointer("pointerup", 53, 120, { altKey: true }));
    expect(events.transform).toHaveBeenLastCalledWith(0, { name: "a", widget: 0 }, [3, 100, 103, 150]);
  });

  it("snaps to the edge of another field", () => {
    const box = boxes()[0];
    box.dispatchEvent(pointer("pointerdown", 150, 120));
    // 3 points above the top of field b (300) when the bottom edge comes to rest: the bottom of a is 150 + 147 = 297.
    box.dispatchEvent(pointer("pointermove", 150, 267));
    box.dispatchEvent(pointer("pointerup", 150, 267));
    expect(events.transform).toHaveBeenLastCalledWith(0, { name: "a", widget: 0 }, [100, 250, 200, 300]);
  });

  it("keeps the box on the page", () => {
    const box = boxes()[0];
    box.dispatchEvent(pointer("pointerdown", 150, 120));
    box.dispatchEvent(pointer("pointermove", 5000, 120, { altKey: true }));
    box.dispatchEvent(pointer("pointerup", 5000, 120, { altKey: true }));
    expect(events.transform).toHaveBeenLastCalledWith(0, { name: "a", widget: 0 }, [512, 100, 612, 150]);
  });

  it("resizes with a handle", () => {
    layer.select(0, { name: "a", widget: 0 });
    const handle = boxes()[0].querySelector<HTMLElement>('[data-handle="se"]')!;
    handle.dispatchEvent(pointer("pointerdown", 200, 150));
    handle.dispatchEvent(pointer("pointermove", 230, 170, { altKey: true }));
    handle.dispatchEvent(pointer("pointerup", 230, 170, { altKey: true }));
    expect(events.transform).toHaveBeenLastCalledWith(0, { name: "a", widget: 0 }, [100, 100, 230, 170]);
  });

  it("leaves a field it cannot change where it is", () => {
    layer.show(0, [item("old", [100, 100, 200, 150], { editable: false })]);
    const box = boxes()[0];
    box.dispatchEvent(pointer("pointerdown", 150, 120));
    box.dispatchEvent(pointer("pointermove", 250, 220));
    box.dispatchEvent(pointer("pointerup", 250, 220));
    expect(events.select).toHaveBeenCalled();
    expect(events.transform).not.toHaveBeenCalled();
  });

  it("does nothing while the document is being changed", () => {
    layer.setHeld(true);
    const box = boxes()[0];
    box.dispatchEvent(pointer("pointerdown", 150, 120));
    box.dispatchEvent(pointer("pointermove", 250, 220));
    box.dispatchEvent(pointer("pointerup", 250, 220));
    expect(events.select).not.toHaveBeenCalled();
    expect(events.transform).not.toHaveBeenCalled();
    expect(page.querySelector<HTMLElement>(".design-layer")!.inert).toBe(true);
  });
});

describe("the keyboard", () => {
  beforeEach(() => layer.show(0, [item("a", [100, 100, 200, 150])]));

  const key = (init: KeyboardEventInit) => boxes()[0].dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));

  it("nudges by a point, by ten with Shift, and resizes with Alt", () => {
    key({ key: "ArrowRight" });
    expect(events.transform).toHaveBeenLastCalledWith(0, { name: "a", widget: 0 }, [101, 100, 201, 150]);
    key({ key: "ArrowUp", shiftKey: true });
    expect(events.transform).toHaveBeenLastCalledWith(0, { name: "a", widget: 0 }, [100, 90, 200, 140]);
    key({ key: "ArrowDown", altKey: true });
    expect(events.transform).toHaveBeenLastCalledWith(0, { name: "a", widget: 0 }, [100, 100, 200, 151]);
  });

  it("deletes with Delete, edits with Enter once picked, and lets go with Escape", () => {
    key({ key: "Delete" });
    expect(events.remove).toHaveBeenCalledWith(0, { name: "a", widget: 0 });
    key({ key: "Enter" });
    expect(events.select).toHaveBeenCalledWith(0, { name: "a", widget: 0 });
    layer.select(0, { name: "a", widget: 0 });
    key({ key: "Enter" });
    expect(events.edit).toHaveBeenCalledWith(0, { name: "a", widget: 0 });
    key({ key: "Escape" });
    expect(events.select).toHaveBeenLastCalledWith(0, null);
  });

  it("leaves Escape to the window while a tool that places fields is on, and lets go of the pick otherwise", () => {
    layer.select(0, { name: "a", widget: 0 });
    layer.setTool("radio");
    events.select.mockClear();
    const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    boxes()[0].dispatchEvent(escape);
    // Not taken: it goes on up to the window, which ends the tool.
    expect(escape.defaultPrevented).toBe(false);
    expect(events.select).not.toHaveBeenCalled();
    layer.setTool("select");
    key({ key: "Escape" });
    expect(events.select).toHaveBeenLastCalledWith(0, null);
  });

  it("says where the keyboard is on the picked box, and gives it back after the boxes are made again", () => {
    layer.select(0, { name: "a", widget: 0 });
    expect(layer.focusState()).toBeNull();
    boxes()[0].focus();
    expect(layer.focusState()).toEqual({ handle: null });
    layer.show(0, [item("a", [100, 100, 200, 150])]);
    expect(document.activeElement).toBe(boxes()[0]);
    const handle = boxes()[0].querySelector<HTMLElement>('[data-handle="se"]')!;
    handle.focus();
    expect(layer.focusState()).toEqual({ handle: "se" });
    // The layer is made again (the pages were laid out again): the pick is the same, the focus is lost.
    const again = new DesignLayer([SIZE], events);
    again.inherit(layer);
    layer.destroy();
    const next = makePage();
    again.mount(0, next);
    again.show(0, [item("a", [100, 100, 200, 150])]);
    expect(document.activeElement).not.toBe(next.querySelector(".design-box"));
    expect(again.restoreFocus({ handle: "se" })).toBe(true);
    expect(document.activeElement).toBe(next.querySelector('[data-handle="se"]'));
    // A layer that waits for a change takes no focus.
    again.setHeld(true);
    expect(again.restoreFocus({ handle: null })).toBe(false);
  });

  it("is a group of a box and its handles, so that the handles are buttons a screen reader finds", () => {
    layer.select(0, { name: "a", widget: 0 });
    expect(boxes()[0].getAttribute("role")).toBe("group");
    expect(boxes()[0].getAttribute("aria-current")).toBe("true");
    expect(boxes()[0].querySelectorAll('[role="button"]').length).toBe(8);
    layer.select(null, null);
    expect(boxes()[0].hasAttribute("aria-current")).toBe(false);
  });

  it("ends a drag whose box was made again, so that the next press starts a new one", () => {
    const box = boxes()[0];
    box.dispatchEvent(pointer("pointerdown", 150, 120));
    box.dispatchEvent(pointer("pointermove", 190, 160));
    // The page is shown again meanwhile: the box under the pointer is gone.
    layer.show(0, [item("a", [100, 100, 200, 150])]);
    expect(events.transform).not.toHaveBeenCalled();
    boxes()[0].dispatchEvent(pointer("pointerdown", 150, 120));
    boxes()[0].dispatchEvent(pointer("pointermove", 190, 160));
    boxes()[0].dispatchEvent(pointer("pointerup", 190, 160));
    expect(events.transform).toHaveBeenCalledTimes(1);
  });

  it("resizes from a handle with the arrow keys", () => {
    layer.select(0, { name: "a", widget: 0 });
    const handle = boxes()[0].querySelector<HTMLElement>('[data-handle="w"]')!;
    expect(handle.getAttribute("aria-label")).toBe("Resize a from the left edge");
    handle.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true }));
    expect(events.transform).toHaveBeenLastCalledWith(0, { name: "a", widget: 0 }, [99, 100, 200, 150]);
  });

  it("does not move a field it cannot change", () => {
    layer.show(0, [item("old", [100, 100, 200, 150], { editable: false })]);
    boxes()[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true }));
    boxes()[0].dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true, cancelable: true }));
    expect(events.transform).not.toHaveBeenCalled();
    expect(events.remove).not.toHaveBeenCalled();
  });
});

describe("placing", () => {
  it("covers the page only while a tool of a field is picked", () => {
    const capture = page.querySelector<HTMLElement>(".design-capture")!;
    expect(capture.hidden).toBe(true);
    layer.setTool("text");
    expect(capture.hidden).toBe(false);
    layer.setTool("select");
    expect(capture.hidden).toBe(true);
  });

  it("makes a box of the default size around a click", () => {
    layer.setTool("text");
    const capture = page.querySelector<HTMLElement>(".design-capture")!;
    capture.dispatchEvent(pointer("pointerdown", 250, 420));
    capture.dispatchEvent(pointer("pointerup", 250, 420));
    expect(events.place).toHaveBeenCalledWith(0, [170, 409, 330, 431]);
  });

  it("makes the box the reader drags, from any corner", () => {
    layer.setTool("signature");
    const capture = page.querySelector<HTMLElement>(".design-capture")!;
    capture.dispatchEvent(pointer("pointerdown", 400, 300, { altKey: true }));
    capture.dispatchEvent(pointer("pointermove", 200, 260, { altKey: true }));
    expect(page.querySelector<HTMLElement>(".design-ghost")!.hidden).toBe(false);
    capture.dispatchEvent(pointer("pointerup", 200, 260, { altKey: true }));
    expect(events.place).toHaveBeenCalledWith(0, [200, 260, 400, 300]);
    expect(page.querySelector<HTMLElement>(".design-ghost")!.hidden).toBe(true);
  });

  it("places nothing with the select tool or while the document is being changed", () => {
    const capture = page.querySelector<HTMLElement>(".design-capture")!;
    capture.dispatchEvent(pointer("pointerdown", 300, 400));
    capture.dispatchEvent(pointer("pointerup", 300, 400));
    layer.setTool("text");
    layer.setHeld(true);
    capture.dispatchEvent(pointer("pointerdown", 300, 400));
    capture.dispatchEvent(pointer("pointerup", 300, 400));
    expect(events.place).not.toHaveBeenCalled();
  });

  it("snaps the corners of a dragged box to the lines", () => {
    expect(snapPoint([3, 297], { xs: [{ value: 0, source: "page" }], ys: [{ value: 300, source: "text" }] }, 6)).toEqual([0, 300]);
    expect(snapPoint([30, 250], { xs: [{ value: 0, source: "page" }], ys: [{ value: 300, source: "text" }] }, 6)).toEqual([30, 250]);
  });
});

describe("a layer that replaces another", () => {
  it("keeps the tool, the pick and the hold", () => {
    layer.setTool("combo");
    layer.select(0, { name: "a", widget: 0 });
    layer.setHeld(true);
    const next = new DesignLayer([SIZE], events);
    next.inherit(layer);
    expect(next.currentTool).toBe("combo");
    expect(next.picked).toEqual({ page: 0, key: { name: "a", widget: 0 } });
  });
});

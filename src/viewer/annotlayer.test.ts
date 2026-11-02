// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Annotation } from "../lib/annotations";
import { AnnotationLayer, type AnnotLayerEvents } from "./annotlayer";

const PAGE = { width: 612, height: 792 };

function annotation(over: Partial<Annotation> = {}): Annotation {
  return {
    id: 1,
    kind: "highlight",
    can: { editText: true, recolour: true, move: true, remove: true, restore: true },
    rect: [61.2, 79.2, 183.6, 158.4],
    color: [255, 235, 59, 255],
    text: "",
    author: "",
    ...over,
  };
}

function setup(list: Annotation[] = [], size = PAGE) {
  const events = {
    onSelect: vi.fn(),
    onPlace: vi.fn(),
    onStroke: vi.fn(),
    onMove: vi.fn(),
    onEdit: vi.fn(),
    onLoadFailed: vi.fn(),
  } satisfies AnnotLayerEvents;
  const load = vi.fn((_page: number) => Promise.resolve(list));
  const layer = new AnnotationLayer([size, size], load, events);
  const pageEl = document.createElement("div");
  document.body.append(pageEl);
  return { layer, events, load, pageEl };
}

/** Gives a page element a place on the screen: `scale` pixels to the point. */
function place(el: HTMLElement, scale: number, size = PAGE, left = 40, top = 30) {
  el.getBoundingClientRect = () =>
    ({ left, top, width: size.width * scale, height: size.height * scale, right: 0, bottom: 0, x: left, y: top }) as DOMRect;
}

function pointer(type: string, x: number, y: number, extra: Record<string, unknown> = {}): PointerEvent {
  const e = new MouseEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true, cancelable: true }) as unknown as Record<string, unknown>;
  Object.assign(e, { pointerId: 1, ...extra });
  return e as unknown as PointerEvent;
}

beforeEach(() => {
  document.body.replaceChildren();
  // jsdom has no pointer capture.
  HTMLElement.prototype.setPointerCapture = vi.fn();
  HTMLElement.prototype.releasePointerCapture = vi.fn();
});

describe("the boxes", () => {
  it("are placed with percentages of the page, so they need no placing again at another zoom", async () => {
    const { layer, pageEl } = setup([annotation()]);
    layer.mount(0, pageEl);
    layer.pageDrawn(0);
    await vi.waitFor(() => expect(pageEl.querySelectorAll(".annot")).toHaveLength(1));
    const box = pageEl.querySelector<HTMLElement>(".annot");
    expect(parseFloat(box?.style.left ?? "")).toBeCloseTo(10, 6);
    expect(parseFloat(box?.style.top ?? "")).toBeCloseTo(10, 6);
    expect(parseFloat(box?.style.width ?? "")).toBeCloseTo(20, 6);
    expect(parseFloat(box?.style.height ?? "")).toBeCloseTo(10, 6);
  });

  it("are named for a screen reader, and the text of the file is only text", async () => {
    const hostile = "<img src=x onerror=alert(1)>";
    const { layer, pageEl } = setup([
      annotation({ id: 1, kind: "note", text: hostile, author: "<b>Bob</b>" }),
      annotation({ id: 2, kind: "ink" }),
    ]);
    layer.mount(1, pageEl);
    layer.pageDrawn(1);
    await vi.waitFor(() => expect(pageEl.querySelectorAll(".annot")).toHaveLength(2));
    const [note, ink] = [...pageEl.querySelectorAll<HTMLElement>(".annot")];
    expect(note.getAttribute("aria-label")).toBe(`Note by <b>Bob</b>, page 2: ${hostile}`);
    expect(note.title).toBe(hostile);
    expect(ink.getAttribute("aria-label")).toBe("Drawing, page 2");
    // Nothing the file said became an element.
    expect(pageEl.querySelector("img, b")).toBeNull();
    expect(note.children).toHaveLength(0);
    expect(note.textContent).toBe("");
  });

  it("report a press, and a double press asks for the text", async () => {
    const { layer, pageEl, events } = setup([annotation({ id: 4 })]);
    layer.mount(0, pageEl);
    layer.pageDrawn(0);
    await vi.waitFor(() => expect(pageEl.querySelector(".annot")).not.toBeNull());
    const box = pageEl.querySelector<HTMLElement>(".annot") as HTMLElement;
    box.dispatchEvent(pointer("pointerdown", 100, 100));
    expect(events.onSelect).toHaveBeenCalledWith(0, 4);
    box.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    expect(events.onEdit).toHaveBeenCalledWith(0, 4);
    // Focus with the keyboard picks it too.
    events.onSelect.mockClear();
    box.dispatchEvent(new FocusEvent("focus"));
    expect(events.onSelect).toHaveBeenCalledWith(0, 4);
  });

  it("show which one is picked", async () => {
    const { layer, pageEl } = setup([annotation({ id: 1 }), annotation({ id: 2 })]);
    layer.mount(0, pageEl);
    layer.pageDrawn(0);
    await vi.waitFor(() => expect(pageEl.querySelectorAll(".annot")).toHaveLength(2));
    layer.setSelected({ page: 0, id: 2 });
    const pressed = [...pageEl.querySelectorAll(".annot")].map((b) => b.getAttribute("aria-pressed"));
    expect(pressed).toEqual(["false", "true"]);
    layer.setSelected(null);
    expect([...pageEl.querySelectorAll(".annot")].map((b) => b.getAttribute("aria-pressed"))).toEqual(["false", "false"]);
  });

  it("let go of the pick when the annotation is gone from the list", async () => {
    const list = [annotation({ id: 1 })];
    const { layer, pageEl, events, load } = setup(list);
    layer.mount(0, pageEl);
    layer.pageDrawn(0);
    await vi.waitFor(() => expect(pageEl.querySelectorAll(".annot")).toHaveLength(1));
    layer.setSelected({ page: 0, id: 1 });
    load.mockResolvedValueOnce([]);
    await layer.reload(0);
    expect(pageEl.querySelectorAll(".annot")).toHaveLength(0);
    expect(events.onSelect).toHaveBeenCalledWith(0, null);
  });

  it("are not kept for a page that is off screen, and are asked for again", async () => {
    const { layer, pageEl, load } = setup([annotation()]);
    layer.mount(0, pageEl);
    layer.pageDrawn(0);
    await vi.waitFor(() => expect(layer.list(0)).toHaveLength(1));
    layer.unmount(0);
    expect(layer.list(0)).toHaveLength(0);
    layer.mount(0, pageEl);
    layer.pageDrawn(0);
    await vi.waitFor(() => expect(load).toHaveBeenCalledTimes(2));
  });

  it("tell the receiver when a page cannot be listed, and carry on", async () => {
    const { layer, pageEl, events, load } = setup();
    load.mockRejectedValueOnce({ code: "corrupt", message: "bad" });
    layer.mount(0, pageEl);
    layer.pageDrawn(0);
    await vi.waitFor(() => expect(events.onLoadFailed).toHaveBeenCalled());
    expect(pageEl.querySelectorAll(".annot")).toHaveLength(0);
  });
});

describe("the tools", () => {
  it("give the page a tool for the style sheet to act on", () => {
    const { layer, pageEl } = setup();
    layer.mount(0, pageEl);
    const root = pageEl.querySelector<HTMLElement>(".annot-layer");
    expect(root?.dataset.tool).toBe("select");
    layer.setTool("pen");
    expect(root?.dataset.tool).toBe("pen");
    // A page mounted later has the tool too.
    const second = document.createElement("div");
    layer.mount(1, second);
    expect(second.querySelector<HTMLElement>(".annot-layer")?.dataset.tool).toBe("pen");
  });

  for (const scale of [0.5, 1, 2, 3.25]) {
    it(`the pen gives the same points on the page at a scale of ${scale}`, () => {
      const { layer, pageEl, events } = setup();
      place(pageEl, scale);
      layer.mount(0, pageEl);
      layer.setTool("pen");
      layer.setInk([0, 0, 255, 255], 3);
      const capture = pageEl.querySelector<HTMLElement>(".annot-capture") as HTMLElement;
      const at = (x: number, y: number): [number, number] => [40 + x * scale, 30 + y * scale];
      capture.dispatchEvent(pointer("pointerdown", ...at(100, 100)));
      capture.dispatchEvent(pointer("pointermove", ...at(150, 130)));
      capture.dispatchEvent(pointer("pointermove", ...at(300, 200)));
      // The line is shown while it is drawn, in the colour and the thickness of the bar, in points.
      const line = pageEl.querySelector("polyline") as SVGPolylineElement;
      expect(line.getAttribute("stroke")).toBe("rgb(0 0 255)");
      expect(line.getAttribute("stroke-width")).toBe("3");
      expect(line.getAttribute("points")).toContain("100.00,100.00");
      capture.dispatchEvent(pointer("pointerup", ...at(300, 200)));
      expect(events.onStroke).toHaveBeenCalledTimes(1);
      const [page, points] = events.onStroke.mock.calls[0] as [number, [number, number][]];
      expect(page).toBe(0);
      expect(points[0][0]).toBeCloseTo(100, 6);
      expect(points[0][1]).toBeCloseTo(100, 6);
      expect(points.at(-1)?.[0]).toBeCloseTo(300, 6);
      expect(points.at(-1)?.[1]).toBeCloseTo(200, 6);
      // The line goes when the owner says the drawing is made.
      expect(line.getAttribute("points")).not.toBeNull();
      layer.endStroke(0);
      expect(line.getAttribute("points")).toBeNull();
    });
  }

  it("the pen on a landscape page uses the size of that page", () => {
    const size = { width: 792, height: 612 };
    const { layer, pageEl, events } = setup([], size);
    place(pageEl, 1.5, size);
    layer.mount(0, pageEl);
    layer.setTool("pen");
    const capture = pageEl.querySelector<HTMLElement>(".annot-capture") as HTMLElement;
    capture.dispatchEvent(pointer("pointerdown", 40 + 792 * 1.5, 30 + 612 * 1.5));
    capture.dispatchEvent(pointer("pointerup", 40 + 792 * 1.5, 30 + 612 * 1.5));
    const [, points] = events.onStroke.mock.calls[0] as [number, [number, number][]];
    expect(points[0]).toEqual([792, 612]);
    expect(pageEl.querySelector("svg")?.getAttribute("viewBox")).toBe("0 0 792 612");
  });

  it("a click without a move is a dot", () => {
    const { layer, pageEl, events } = setup();
    place(pageEl, 1);
    layer.mount(0, pageEl);
    layer.setTool("pen");
    const capture = pageEl.querySelector<HTMLElement>(".annot-capture") as HTMLElement;
    capture.dispatchEvent(pointer("pointerdown", 140, 130));
    capture.dispatchEvent(pointer("pointerup", 140, 130));
    expect(events.onStroke.mock.calls[0][1]).toEqual([[100, 100]]);
  });

  it("Escape in the middle of a stroke throws it away", () => {
    const { layer, pageEl, events } = setup();
    place(pageEl, 1);
    layer.mount(0, pageEl);
    layer.setTool("pen");
    const capture = pageEl.querySelector<HTMLElement>(".annot-capture") as HTMLElement;
    capture.dispatchEvent(pointer("pointerdown", 140, 130));
    expect(layer.abort()).toBe(true);
    capture.dispatchEvent(pointer("pointerup", 140, 130));
    expect(events.onStroke).not.toHaveBeenCalled();
    expect(layer.abort()).toBe(false);
  });

  it("the note and stamp tools report a click as a place on the page, and a drag as nothing", () => {
    const { layer, pageEl, events } = setup();
    place(pageEl, 2);
    layer.mount(0, pageEl);
    layer.setTool("note");
    const capture = pageEl.querySelector<HTMLElement>(".annot-capture") as HTMLElement;
    capture.dispatchEvent(pointer("pointerdown", 40 + 2 * 200, 30 + 2 * 300));
    capture.dispatchEvent(pointer("pointerup", 40 + 2 * 200, 30 + 2 * 300));
    expect(events.onPlace).toHaveBeenCalledWith(0, 200, 300, { x: 440, y: 630 });
    events.onPlace.mockClear();
    capture.dispatchEvent(pointer("pointerdown", 100, 100));
    capture.dispatchEvent(pointer("pointerup", 160, 160));
    expect(events.onPlace).not.toHaveBeenCalled();
  });

  it("a box is dragged by the distance the pointer moved, in points", async () => {
    const { layer, pageEl, events } = setup([annotation({ id: 3, kind: "note" })]);
    place(pageEl, 2);
    layer.mount(0, pageEl);
    layer.pageDrawn(0);
    await vi.waitFor(() => expect(pageEl.querySelector(".annot")).not.toBeNull());
    const box = pageEl.querySelector<HTMLElement>(".annot") as HTMLElement;
    box.dispatchEvent(pointer("pointerdown", 200, 200));
    window.dispatchEvent(pointer("pointermove", 230, 200));
    expect(box.style.transform).toBe("translate(30px, 0px)");
    window.dispatchEvent(pointer("pointerup", 260, 220));
    expect(events.onMove).toHaveBeenCalledWith(0, 3, 30, 10);
    expect(box.style.transform).toBe("");
  });

  it("a box that cannot move is not dragged", async () => {
    const { layer, pageEl, events } = setup([
      annotation({ id: 3, can: { editText: true, recolour: true, move: false, remove: true, restore: true } }),
    ]);
    place(pageEl, 1);
    layer.mount(0, pageEl);
    layer.pageDrawn(0);
    await vi.waitFor(() => expect(pageEl.querySelector(".annot")).not.toBeNull());
    const box = pageEl.querySelector<HTMLElement>(".annot") as HTMLElement;
    box.dispatchEvent(pointer("pointerdown", 100, 100));
    window.dispatchEvent(pointer("pointermove", 200, 100));
    window.dispatchEvent(pointer("pointerup", 200, 100));
    expect(events.onMove).not.toHaveBeenCalled();
  });
});

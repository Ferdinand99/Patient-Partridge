// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Annotation } from "../lib/annotations";
import { AnnotationLayer, type AnnotLayerEvents } from "./annotlayer";

const PAGE = { width: 612, height: 792 };

function signature(over: Partial<Annotation> = {}, resize = true): Annotation {
  return {
    id: 7,
    kind: "signature",
    can: { editText: false, recolour: false, move: true, remove: true, restore: true, locked: false, resize },
    rect: [100, 200, 260, 264],
    color: [20, 20, 22, 255],
    text: "Signature picture",
    author: "",
    ...over,
  };
}

function setup(list: Annotation[]) {
  const events = {
    onSelect: vi.fn(),
    onPlace: vi.fn(),
    onStroke: vi.fn(),
    onMove: vi.fn(),
    onEdit: vi.fn(),
    onResize: vi.fn(),
  } satisfies AnnotLayerEvents;
  const layer = new AnnotationLayer([PAGE], () => Promise.resolve(list), events);
  const pageEl = document.createElement("div");
  document.body.append(pageEl);
  // One pixel to the point, the page at (40, 30).
  pageEl.getBoundingClientRect = () => ({ left: 40, top: 30, width: PAGE.width, height: PAGE.height, right: 0, bottom: 0, x: 40, y: 30 }) as DOMRect;
  layer.mount(0, pageEl);
  return { layer, events, pageEl };
}

function pointer(type: string, x: number, y: number): PointerEvent {
  const e = new MouseEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true, cancelable: true }) as unknown as Record<string, unknown>;
  e.pointerId = 1;
  return e as unknown as PointerEvent;
}

beforeEach(() => {
  document.body.replaceChildren();
  HTMLElement.prototype.setPointerCapture = vi.fn();
  HTMLElement.prototype.releasePointerCapture = vi.fn();
});

async function shown(list: Annotation[]) {
  const t = setup(list);
  t.layer.pageDrawn(0);
  await vi.waitFor(() => expect(t.pageEl.querySelector(".annot")).not.toBeNull());
  return t;
}

describe("the corner of a picture of a signature", () => {
  it("is on a picture that can be resized and on nothing else", async () => {
    const t = await shown([signature(), signature({ id: 8, kind: "stamp", can: { ...signature().can, resize: false } })]);
    expect(t.pageEl.querySelectorAll(".annot-handle")).toHaveLength(1);
    expect(t.pageEl.querySelector('[data-id="7"] .annot-handle')).not.toBeNull();
    expect(t.pageEl.querySelector('[data-id="8"] .annot-handle')).toBeNull();
    expect(t.pageEl.querySelector(".annot-handle")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("is dragged, and the new box is handed on when the pointer is let go: the width follows, the proportions stay", async () => {
    const t = await shown([signature()]);
    const handle = t.pageEl.querySelector(".annot-handle") as HTMLElement;
    handle.dispatchEvent(pointer("pointerdown", 40 + 260, 30 + 264));
    window.dispatchEvent(pointer("pointermove", 40 + 260 + 80, 30 + 264 + 5));
    // While it moves the box shows the new size.
    const box = t.pageEl.querySelector<HTMLElement>('[data-id="7"]')!;
    expect(parseFloat(box.style.width)).toBeCloseTo((240 / 612) * 100, 1);
    window.dispatchEvent(pointer("pointerup", 40 + 260 + 80, 30 + 264 + 5));
    expect(t.events.onResize).toHaveBeenCalledTimes(1);
    const [page, id, rect] = t.events.onResize.mock.calls[0];
    expect([page, id]).toEqual([0, 7]);
    expect(rect[0]).toBe(100);
    expect(rect[1]).toBe(200);
    expect(rect[2] - rect[0]).toBeCloseTo(240, 3);
    expect((rect[2] - rect[0]) / (rect[3] - rect[1])).toBeCloseTo(2.5, 5);
    // Picking it is what pressing the corner does first, and it is not a move of the box.
    expect(t.events.onSelect).toHaveBeenCalledWith(0, 7);
    expect(t.events.onMove).not.toHaveBeenCalled();
  });

  it("is a shrink when dragged the other way, not smaller than the least a picture can be", async () => {
    const t = await shown([signature()]);
    const handle = t.pageEl.querySelector(".annot-handle") as HTMLElement;
    handle.dispatchEvent(pointer("pointerdown", 300, 294));
    window.dispatchEvent(pointer("pointermove", 300 - 600, 294));
    window.dispatchEvent(pointer("pointerup", 300 - 600, 294));
    const rect = t.events.onResize.mock.calls[0][2];
    expect(rect[2] - rect[0]).toBeGreaterThanOrEqual(8);
    expect(rect[3] - rect[1]).toBeGreaterThanOrEqual(8 - 0.001);
  });

  it("is kept inside the page", async () => {
    const t = await shown([signature({ rect: [500, 700, 580, 720] })]);
    const handle = t.pageEl.querySelector(".annot-handle") as HTMLElement;
    handle.dispatchEvent(pointer("pointerdown", 40 + 580, 30 + 720));
    window.dispatchEvent(pointer("pointermove", 40 + 580 + 900, 30 + 720));
    window.dispatchEvent(pointer("pointerup", 40 + 580 + 900, 30 + 720));
    const rect = t.events.onResize.mock.calls[0][2];
    expect(rect[2]).toBeLessThanOrEqual(612);
    expect(rect[3]).toBeLessThanOrEqual(792);
  });

  it("hands on nothing for a press that did not move, and puts the box back when Escape is pressed", async () => {
    const t = await shown([signature()]);
    const handle = t.pageEl.querySelector(".annot-handle") as HTMLElement;
    handle.dispatchEvent(pointer("pointerdown", 300, 294));
    window.dispatchEvent(pointer("pointerup", 300, 294));
    expect(t.events.onResize).not.toHaveBeenCalled();
    const box = t.pageEl.querySelector<HTMLElement>('[data-id="7"]')!;
    const before = box.style.width;
    handle.dispatchEvent(pointer("pointerdown", 300, 294));
    window.dispatchEvent(pointer("pointermove", 400, 294));
    expect(box.style.width).not.toBe(before);
    expect(t.layer.abort()).toBe(true);
    expect(box.style.width).toBe(before);
    window.dispatchEvent(pointer("pointerup", 400, 294));
    expect(t.events.onResize).not.toHaveBeenCalled();
  });

  it("is dropped when the pages start to change", async () => {
    const t = await shown([signature()]);
    const handle = t.pageEl.querySelector(".annot-handle") as HTMLElement;
    handle.dispatchEvent(pointer("pointerdown", 300, 294));
    window.dispatchEvent(pointer("pointermove", 400, 294));
    t.layer.setHeld(true);
    window.dispatchEvent(pointer("pointerup", 400, 294));
    expect(t.events.onResize).not.toHaveBeenCalled();
    expect(t.pageEl.querySelector<HTMLElement>(".annot-boxes")?.inert).toBe(true);
  });

  it("does nothing while another tool is in use", async () => {
    const t = await shown([signature()]);
    t.layer.setTool("pen");
    const handle = t.pageEl.querySelector(".annot-handle") as HTMLElement;
    handle.dispatchEvent(pointer("pointerdown", 300, 294));
    window.dispatchEvent(pointer("pointermove", 400, 294));
    window.dispatchEvent(pointer("pointerup", 400, 294));
    expect(t.events.onResize).not.toHaveBeenCalled();
  });
});

describe("the signature tool on a page", () => {
  it("takes a click and tells where, like the note and the stamp", async () => {
    const t = setup([]);
    t.layer.setTool("signature");
    const capture = t.pageEl.querySelector<HTMLElement>(".annot-capture")!;
    expect(t.pageEl.querySelector<HTMLElement>(".annot-layer")?.dataset.tool).toBe("signature");
    capture.dispatchEvent(pointer("pointerdown", 40 + 120, 30 + 90));
    capture.dispatchEvent(pointer("pointerup", 40 + 120, 30 + 90));
    expect(t.events.onPlace).toHaveBeenCalledWith(0, 120, 90, { x: 160, y: 120 });
  });

  it("takes no click while the pages are being changed", () => {
    const t = setup([]);
    t.layer.setTool("signature");
    t.layer.setHeld(true);
    const capture = t.pageEl.querySelector<HTMLElement>(".annot-capture")!;
    expect(capture.inert).toBe(true);
  });
});

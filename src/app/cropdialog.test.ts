// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { askCrop, type CropRequest } from "./cropdialog";

const letter = { width: 612, height: 792 };

beforeEach(() => {
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
    () => ({ putImageData: vi.fn(), drawImage: vi.fn() }) as unknown as CanvasRenderingContext2D,
  );
  // Pointer capture does not exist in jsdom.
  HTMLElement.prototype.setPointerCapture = vi.fn();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  document.body.replaceChildren();
});

function open(over: Partial<CropRequest> = {}) {
  const layer = document.createElement("div");
  layer.hidden = true;
  document.body.append(layer);
  const render = vi.fn((_w: number, _h: number) => Promise.resolve(new ArrayBuffer(_w * _h * 4)));
  const answer = askCrop({
    layer,
    name: "book.pdf",
    page: 2,
    size: letter,
    scopes: [
      { id: "picked", label: "Page 3", pages: [2] },
      { id: "all", label: "All 10 pages", pages: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9] },
    ],
    render,
    ...over,
  });
  const form = layer.querySelector("form") as HTMLFormElement;
  const input = (id: string) => form.querySelector<HTMLInputElement>(`#crop-${id}`) as HTMLInputElement;
  const stage = form.querySelector<HTMLElement>(".crop-stage") as HTMLElement;
  const frame = form.querySelector<HTMLElement>(".crop-box") as HTMLElement;
  const submit = form.querySelector<HTMLButtonElement>('button[type="submit"]') as HTMLButtonElement;
  const cancel = [...form.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent === "Cancel") as HTMLButtonElement;
  const type = (id: string, value: string) => {
    const el = input(id);
    el.value = value;
    el.dispatchEvent(new Event("input", { bubbles: true }));
  };
  const pointer = (type: string, el: Element, x: number, y: number) =>
    el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, clientX: x, clientY: y }));
  return { layer, form, answer, input, stage, frame, submit, cancel, type, pointer, render };
}

describe("the crop box", () => {
  it("says what a crop does and does not do, and starts with a small margin all round", () => {
    const c = open();
    expect(c.form.textContent).toMatch(/does not remove anything/);
    expect(c.form.textContent).toMatch(/still in the file/);
    // Half an inch in millimetres.
    for (const side of ["left", "top", "right", "bottom"]) expect(c.input(side).value).toBe("12.7");
    expect(c.layer.hidden).toBe(false);
    expect(c.form.getAttribute("role")).toBe("dialog");
    expect(c.form.getAttribute("aria-modal")).toBe("true");
    expect(document.activeElement).toBe(c.input("left"));
  });

  it("asks for a picture of the page, big enough for the screen, and copes when it does not come", async () => {
    const c = open({ render: vi.fn(() => Promise.reject(new Error("no engine"))) });
    await Promise.resolve();
    expect(c.stage.querySelector("canvas")).not.toBeNull();
    c.cancel.click();
    expect(await c.answer).toBeNull();
  });

  it("the numbers move the frame and the answer is in points", async () => {
    const c = open();
    c.type("left", "10");
    c.type("top", "20");
    c.type("right", "0");
    c.type("bottom", "5.5");
    c.submit.click();
    const answer = await c.answer;
    expect(answer?.pages).toEqual([2]);
    const mm = 72 / 25.4;
    expect(answer?.margins[0]).toBeCloseTo(10 * mm, 5);
    expect(answer?.margins[1]).toBeCloseTo(20 * mm, 5);
    expect(answer?.margins[2]).toBe(0);
    expect(answer?.margins[3]).toBeCloseTo(5.5 * mm, 5);
    expect(c.layer.querySelector("form")).toBeNull();
    expect(c.layer.hidden).toBe(true);
  });

  it("the frame follows the numbers in the picture", () => {
    const c = open();
    c.type("left", "25.4");
    // The picture is at most 340 high, the page is 792 points: 25.4 mm is 72 points.
    const scale = Math.min(340 / 612, 340 / 792);
    expect(parseFloat(c.frame.style.left)).toBeCloseTo(72 * scale, 3);
  });

  it("numbers that leave too little page, or less than nothing, are held back", async () => {
    const c = open();
    c.type("left", "-5");
    c.type("right", "100000");
    c.type("top", "abc");
    c.submit.click();
    const answer = await c.answer;
    expect(answer?.margins[0]).toBe(0);
    expect(612 - (answer?.margins[0] ?? 0) - (answer?.margins[2] ?? 0)).toBeGreaterThanOrEqual(8 - 1e-9);
    expect(Number.isFinite(answer?.margins[1])).toBe(true);
  });

  it("dragging an edge or a corner changes only its margins", async () => {
    const c = open();
    const scale = Math.min(340 / 612, 340 / 792);
    const handle = (name: string) => c.form.querySelector(`.crop-handle[data-handle="${name}"]`) as Element;
    // The right edge to the right by 20 points: the right margin shrinks.
    c.pointer("pointerdown", handle("e"), 500, 100);
    c.pointer("pointermove", c.stage, 500 + 20 * scale, 100);
    c.pointer("pointerup", c.stage, 500 + 20 * scale, 100);
    // The top left corner in by 10 and 30 points.
    c.pointer("pointerdown", handle("nw"), 10, 10);
    c.pointer("pointermove", c.stage, 10 + 10 * scale, 10 + 30 * scale);
    c.pointer("pointerup", c.stage, 10 + 10 * scale, 10 + 30 * scale);
    c.submit.click();
    const answer = await c.answer;
    expect(answer?.margins[0]).toBeCloseTo(36 + 10, 3);
    expect(answer?.margins[1]).toBeCloseTo(36 + 30, 3);
    expect(answer?.margins[2]).toBeCloseTo(36 - 20, 3);
    expect(answer?.margins[3]).toBeCloseTo(36, 3);
  });

  it("dragging the frame moves it without changing its size, and stops at the page", async () => {
    const c = open();
    c.pointer("pointerdown", c.frame, 100, 100);
    c.pointer("pointermove", c.stage, 100 - 5000, 100 - 5000);
    c.pointer("pointerup", c.stage, 0, 0);
    c.submit.click();
    const answer = await c.answer;
    expect(answer?.margins[0]).toBe(0);
    expect(answer?.margins[1]).toBe(0);
    expect(612 - answer!.margins[0] - answer!.margins[2]).toBeCloseTo(612 - 72, 3);
  });

  it("the page choice decides which pages are cropped", async () => {
    const c = open();
    const radios = [...c.form.querySelectorAll<HTMLInputElement>('input[name="crop-scope"]')];
    expect(radios.map((r) => r.checked)).toEqual([true, false]);
    radios[1].checked = true;
    c.submit.click();
    expect((await c.answer)?.pages).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  });

  it("with no margin at all there is nothing to crop", async () => {
    const c = open();
    for (const side of ["left", "top", "right", "bottom"]) c.type(side, "0");
    expect(c.submit.disabled).toBe(true);
    c.submit.click();
    c.form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    expect(c.layer.querySelector("form")).not.toBeNull();
    c.type("left", "3");
    expect(c.submit.disabled).toBe(false);
    c.cancel.click();
    expect(await c.answer).toBeNull();
  });

  it("Escape closes it without an answer, and the focus goes back where it was", async () => {
    const button = document.createElement("button");
    document.body.append(button);
    button.focus();
    const c = open();
    c.layer.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(await c.answer).toBeNull();
    expect(c.layer.querySelector("form")).toBeNull();
    expect(document.activeElement).toBe(button);
  });

  it("a very small or very odd page still gets a box that works", async () => {
    for (const size of [
      { width: 20, height: 20 },
      { width: 14400, height: 10 },
      { width: 10, height: 14400 },
    ]) {
      const c = open({ size });
      expect(Number.isFinite(parseFloat(c.frame.style.width))).toBe(true);
      expect(parseFloat(c.frame.style.width)).toBeGreaterThan(0);
      c.cancel.click();
      expect(await c.answer).toBeNull();
    }
  });
});

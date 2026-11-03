// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  MAX_SAVED,
  MAX_SAVED_PNG_BYTES,
  MAX_STROKES,
  MAX_TEXT_CHARS,
  PICTURE_WORDS,
  toBase64,
  type SavedBody,
  type SavedPicture,
} from "../lib/signpicture";
import {
  askSignaturePicture,
  type DecodedImage,
  type PictureAnswer,
  type PictureStore,
  type PictureTools,
} from "./signpicture";

let layer: HTMLElement;

beforeEach(() => {
  document.body.replaceChildren();
  layer = document.createElement("div");
  layer.hidden = true;
  document.body.append(layer);
  // jsdom has no canvas and no pointer capture.
  HTMLCanvasElement.prototype.getContext = vi.fn(() => null) as never;
  HTMLElement.prototype.setPointerCapture = vi.fn();
  HTMLElement.prototype.releasePointerCapture = vi.fn();
  URL.createObjectURL = vi.fn(() => "blob:test");
  URL.revokeObjectURL = vi.fn();
});

function pixels(w: number, h: number, rgba: [number, number, number, number]): DecodedImage {
  const data = new Uint8ClampedArray(w * h * 4);
  for (let i = 0; i < w * h; i++) data.set(rgba, i * 4);
  return { width: w, height: h, rgba: data };
}

function savedDraw(id = "m1", name = "Mine"): SavedPicture {
  return { id, name, created: 1, kind: "draw", width: 100, height: 40, line: 3, strokes: [[[5, 5], [90, 30]]] };
}

function memoryStore(initial: SavedPicture[] = []) {
  let items = [...initial];
  const calls: string[] = [];
  const store: PictureStore = {
    list: vi.fn(() => Promise.resolve([...items])),
    add: vi.fn((name: string, body: SavedBody) => {
      calls.push(`add ${name} ${body.kind}`);
      const kept = { id: `n${items.length + 1}`, name, created: 2, ...body } as SavedPicture;
      items = [...items, kept];
      return Promise.resolve(kept);
    }),
    rename: vi.fn((id: string, name: string) => {
      calls.push(`rename ${id} ${name}`);
      items = items.map((i) => (i.id === id ? { ...i, name } : i));
      return Promise.resolve(items.find((i) => i.id === id) as SavedPicture);
    }),
    remove: vi.fn((id: string) => {
      calls.push(`remove ${id}`);
      items = items.filter((i) => i.id !== id);
      return Promise.resolve();
    }),
  };
  return { store, calls, items: () => items };
}

function tools(over: Partial<PictureTools> = {}): PictureTools {
  return {
    store: null,
    decode: vi.fn(() => Promise.resolve(pixels(4, 2, [10, 20, 30, 255]))),
    encodePng: vi.fn(() => Promise.resolve(new Uint8Array(100))),
    scale: vi.fn((_image: DecodedImage, w: number, h: number) => Promise.resolve(pixels(w, h, [1, 2, 3, 255]))),
    measure: vi.fn(() => Promise.resolve(5)),
    ...over,
  };
}

function open(t: PictureTools = tools()) {
  const answer = askSignaturePicture({ layer, tools: t });
  const form = layer.querySelector("form") as HTMLFormElement;
  const q = <T extends Element>(sel: string) => form.querySelector(sel) as T;
  return { answer, form, q, t };
}

function pointer(type: string, x: number, y: number, id = 1): PointerEvent {
  const e = new MouseEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true, cancelable: true }) as unknown as Record<string, unknown>;
  e.pointerId = id;
  return e as unknown as PointerEvent;
}

/** A stroke over the drawing surface, which is 520 by 180 on the screen too. */
function draw(canvas: HTMLCanvasElement, points: [number, number][]) {
  canvas.getBoundingClientRect = () => ({ left: 0, top: 0, width: 520, height: 180, right: 520, bottom: 180, x: 0, y: 0 }) as DOMRect;
  canvas.dispatchEvent(pointer("pointerdown", points[0][0], points[0][1]));
  for (const p of points.slice(1)) canvas.dispatchEvent(pointer("pointermove", p[0], p[1]));
  canvas.dispatchEvent(pointer("pointerup", points.at(-1)![0], points.at(-1)![1]));
}

const button = (root: ParentNode, label: string) =>
  [...root.querySelectorAll("button")].find((b) => b.textContent === label || b.getAttribute("aria-label") === label) as HTMLButtonElement;

const flush = () => new Promise((r) => setTimeout(r, 10));

/** The first bytes of a PNG file: the signature and the header chunk, with the size it declares. */
function pngHead(width: number, height: number): Uint8Array {
  const out = new Uint8Array(33);
  out.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52]);
  const view = new DataView(out.buffer);
  view.setUint32(16, width);
  view.setUint32(20, height);
  return out;
}

function pngFile(name = "sig.png", width = 4, height = 2): File {
  return new File([pngHead(width, height) as BlobPart], name, { type: "image/png" });
}

describe("the box says what a picture of a signature is", () => {
  it("states that it is a picture, not a digital signature, and does not show who placed it", () => {
    const { form } = open();
    expect(form.textContent).toContain("A picture is not a digital signature");
    expect(form.textContent).toContain("does not show who placed it");
    expect(form.getAttribute("role")).toBe("dialog");
    expect(form.getAttribute("aria-modal")).toBe("true");
    expect(layer.hidden).toBe(false);
  });

  it("makes no promise of validity anywhere in what it shows", () => {
    const { form } = open(tools({ store: memoryStore([savedDraw()]).store }));
    for (const tab of ["Draw", "Type", "Image", "Saved"]) {
      button(form, tab).click();
      expect(form.textContent ?? "").not.toMatch(new RegExp("\\b(legal|legally|court|guarantee|valid|binding|qualified|advanced|eidas|compliant|certified|" + "ad" + "obe|acro" + "bat)\\b", "i"));
    }
  });

  it("closes with nothing on Cancel and on Escape, and gives the focus back", async () => {
    const opener = document.createElement("button");
    document.body.append(opener);
    opener.focus();
    const a = open();
    button(a.form, "Cancel").click();
    expect(await a.answer).toBeNull();
    expect(layer.querySelector("form")).toBeNull();
    expect(layer.hidden).toBe(true);
    expect(document.activeElement).toBe(opener);
    const b = open();
    layer.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(await b.answer).toBeNull();
  });
});

describe("drawing", () => {
  it("is not ready until something is drawn, and then gives a picture cut to the lines", async () => {
    const { answer, form, q } = open();
    const go = button(form, "Use on the page");
    expect(go.disabled).toBe(true);
    draw(q("canvas.sigpic-surface"), [[100, 50], [150, 90], [220, 60]]);
    expect(go.disabled).toBe(false);
    expect(q(".sigpic-row .sigpic-status").textContent).toBe("1 of 64 lines.");
    form.requestSubmit();
    const result = (await answer) as PictureAnswer;
    expect(result.picture.kind).toBe("draw");
    if (result.picture.kind !== "draw") return;
    // The thickness is the middle one (4), so the box has 3 units around the lines. The corners
    // are cut when the lines are rounded, so the box is a little smaller than the raw points.
    expect(result.picture.line).toBe(4);
    expect(result.picture.width).toBeCloseTo(120 + 6, 0);
    expect(result.picture.height).toBeGreaterThan(30);
    expect(result.picture.height).toBeLessThanOrEqual(40 + 6);
    expect(result.picture.color).toEqual([20, 20, 22, 255]);
    expect(result.picture.aspect).toBeCloseTo(result.picture.width / result.picture.height, 5);
  });

  it("takes the thickness and the colour that are picked", async () => {
    const { answer, form, q } = open();
    button(form, "Thick").click();
    button(form, "Blue").click();
    draw(q("canvas.sigpic-surface"), [[10, 10], [60, 10]]);
    form.requestSubmit();
    const result = (await answer) as PictureAnswer;
    expect(result.picture.kind === "draw" && result.picture.line).toBe(7);
    expect(result.picture.color).toEqual([24, 56, 150, 255]);
  });

  it("takes a line back with Undo line and everything with Clear", () => {
    const { form, q } = open();
    const surface = q<HTMLCanvasElement>("canvas.sigpic-surface");
    draw(surface, [[10, 10], [60, 10]]);
    draw(surface, [[10, 40], [60, 40]]);
    expect(q(".sigpic-row .sigpic-status").textContent).toBe("2 of 64 lines.");
    button(form, "Undo line").click();
    expect(q(".sigpic-row .sigpic-status").textContent).toBe("1 of 64 lines.");
    button(form, "Clear").click();
    expect(button(form, "Use on the page").disabled).toBe(true);
    expect(button(form, "Clear").disabled).toBe(true);
  });

  it("holds the drawing to the most lines and says so", async () => {
    const { answer, form, q } = open();
    const surface = q<HTMLCanvasElement>("canvas.sigpic-surface");
    for (let i = 0; i < MAX_STROKES + 5; i++) draw(surface, [[10 + (i % 50) * 8, 20], [14 + (i % 50) * 8, 60]]);
    expect(q(".sigpic-row .sigpic-status").textContent).toBe(PICTURE_WORDS.full);
    form.requestSubmit();
    const result = (await answer) as PictureAnswer;
    expect(result.picture.kind === "draw" && result.picture.strokes.length).toBe(MAX_STROKES);
  });

  it("drops a line that the pointer cancelled", () => {
    const { form, q } = open();
    const surface = q<HTMLCanvasElement>("canvas.sigpic-surface");
    surface.getBoundingClientRect = () => ({ left: 0, top: 0, width: 520, height: 180, right: 520, bottom: 180, x: 0, y: 0 }) as DOMRect;
    surface.dispatchEvent(pointer("pointerdown", 10, 10));
    surface.dispatchEvent(pointer("pointermove", 90, 50));
    surface.dispatchEvent(pointer("pointercancel", 90, 50));
    expect(button(form, "Use on the page").disabled).toBe(true);
  });
});

describe("typing", () => {
  it("gives the name in the font that is picked", async () => {
    const t = tools({ measure: vi.fn(() => Promise.resolve(6.5)) });
    const { answer, form, q } = open(t);
    button(form, "Type").click();
    const input = q<HTMLInputElement>("input.sigpic-text");
    input.value = "  Anna   Alvorlig ";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    form.querySelector<HTMLButtonElement>('[data-font="kristi"]')!.click();
    await flush();
    expect(q(".sigpic-preview").textContent).toBe("Anna Alvorlig");
    expect(q<HTMLElement>(".sigpic-preview").style.fontFamily).toContain("Partridge Kristi");
    form.requestSubmit();
    const result = (await answer) as PictureAnswer;
    expect(result.picture).toMatchObject({ kind: "type", font: "kristi", text: "Anna Alvorlig", aspect: 6.5 });
    expect(t.measure).toHaveBeenCalledWith("Anna Alvorlig", "kristi");
  });

  it("is not ready with no name, with only spaces, or with characters that hide", () => {
    const { form, q } = open();
    button(form, "Type").click();
    const input = q<HTMLInputElement>("input.sigpic-text");
    const go = button(form, "Use on the page");
    for (const bad of ["", "    ", "​‮\t"]) {
      input.value = bad;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      expect(go.disabled, JSON.stringify(bad)).toBe(true);
    }
    input.value = "A";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(go.disabled).toBe(false);
  });

  it("holds the name to the most characters, also when more is pasted", async () => {
    const { answer, form, q } = open();
    button(form, "Type").click();
    const input = q<HTMLInputElement>("input.sigpic-text");
    expect(input.maxLength).toBe(MAX_TEXT_CHARS);
    input.value = "ab".repeat(40);
    input.dispatchEvent(new Event("input", { bubbles: true }));
    expect(form.querySelectorAll(".sigpic-panel")[1].querySelector(".sigpic-status")?.textContent).toBe(PICTURE_WORDS.tooLong);
    form.requestSubmit();
    const result = (await answer) as PictureAnswer;
    expect(result.picture.kind === "type" && Array.from(result.picture.text).length).toBe(MAX_TEXT_CHARS);
  });

  it("submits with Enter in the field", async () => {
    const { answer, form, q } = open();
    button(form, "Type").click();
    const input = q<HTMLInputElement>("input.sigpic-text");
    input.value = "Anna";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    expect(((await answer) as PictureAnswer).picture.kind).toBe("type");
  });
});

describe("importing an image", () => {
  function pick(form: HTMLFormElement, file: File) {
    const input = form.querySelector<HTMLInputElement>("input.sigpic-file")!;
    Object.defineProperty(input, "files", { value: [file], configurable: true });
    input.dispatchEvent(new Event("change", { bubbles: true }));
  }

  it("reads the file into pixels in the window and hands them over", async () => {
    const t = tools();
    const { answer, form } = open(t);
    button(form, "Image").click();
    const go = button(form, "Use on the page");
    expect(go.disabled).toBe(true);
    pick(form, pngFile("sig.png"));
    await flush();
    expect(t.decode).toHaveBeenCalledTimes(1);
    expect(go.disabled).toBe(false);
    form.requestSubmit();
    const result = (await answer) as PictureAnswer;
    expect(result.picture.kind).toBe("image");
    if (result.picture.kind !== "image") return;
    expect([result.picture.width, result.picture.height]).toEqual([4, 2]);
    expect(result.picture.rgba).toHaveLength(32);
    expect(result.picture.aspect).toBe(2);
  });

  it("can make the white see-through, and is not ready when nothing is left to see", async () => {
    const t = tools({ decode: vi.fn(() => Promise.resolve(pixels(3, 3, [255, 255, 255, 255]))) });
    const { answer, form } = open(t);
    button(form, "Image").click();
    pick(form, pngFile("white.png"));
    await flush();
    const go = button(form, "Use on the page");
    expect(go.disabled).toBe(false);
    const box = form.querySelector<HTMLInputElement>('.sigpic-check input[type="checkbox"]')!;
    box.checked = true;
    box.dispatchEvent(new Event("change", { bubbles: true }));
    // Everything was white: nothing is left.
    expect(go.disabled).toBe(true);
    box.checked = false;
    box.dispatchEvent(new Event("change", { bubbles: true }));
    expect(go.disabled).toBe(false);
    form.requestSubmit();
    expect(((await answer) as PictureAnswer).picture.kind).toBe("image");
  });

  it("makes the near-white pixels of a picture with a drawing in it see-through", async () => {
    const mixed: DecodedImage = { width: 2, height: 1, rgba: new Uint8ClampedArray([255, 255, 255, 255, 10, 10, 10, 255]) };
    const { answer, form } = open(tools({ decode: vi.fn(() => Promise.resolve(mixed)) }));
    button(form, "Image").click();
    pick(form, pngFile("pen.png"));
    await flush();
    const box = form.querySelector<HTMLInputElement>('.sigpic-check input[type="checkbox"]')!;
    box.checked = true;
    box.dispatchEvent(new Event("change", { bubbles: true }));
    form.requestSubmit();
    const result = (await answer) as PictureAnswer;
    expect(result.picture.kind === "image" && Array.from(result.picture.rgba)).toEqual([255, 255, 255, 0, 10, 10, 10, 255]);
  });

  it("refuses a file that is too big, a drawing file and a file that is not a picture", async () => {
    const t = tools({ decode: vi.fn(() => Promise.reject(new Error("no"))) });
    const { form, q } = open(t);
    button(form, "Image").click();
    const status = () => q<HTMLElement>("#" + form.querySelector("[role=tabpanel]:not([hidden])")!.id + " .sigpic-status").textContent;
    const big = new File([new Uint8Array(1)], "big.png", { type: "image/png" });
    Object.defineProperty(big, "size", { value: 13 * 1024 * 1024 });
    pick(form, big);
    expect(status()).toBe(PICTURE_WORDS.imageTooBig);
    pick(form, new File(["<svg/>"], "pen.svg", { type: "image/svg+xml" }));
    expect(status()).toBe(PICTURE_WORDS.imageFailed);
    expect(t.decode).not.toHaveBeenCalled();
    pick(form, new File([new Uint8Array(10)], "x.png", { type: "image/png" }));
    await flush();
    expect(status()).toBe(PICTURE_WORDS.imageFailed);
    expect(button(form, "Use on the page").disabled).toBe(true);
  });

  it("refuses pixels that do not fit their size", async () => {
    const t = tools({ decode: vi.fn(() => Promise.resolve({ width: 4, height: 4, rgba: new Uint8ClampedArray(7) })) });
    const { form } = open(t);
    button(form, "Image").click();
    pick(form, pngFile("x.png", 4, 4));
    await flush();
    expect(button(form, "Use on the page").disabled).toBe(true);
  });

  it("reads the size in the header first and decodes nothing that declares a picture too big to decode", async () => {
    const t = tools();
    const { form, q } = open(t);
    button(form, "Image").click();
    const status = () => q<HTMLElement>("#" + form.querySelector("[role=tabpanel]:not([hidden])")!.id + " .sigpic-status").textContent;
    // A file of 33 bytes that says it is a picture of 30000 by 30000 pixels.
    const bomb = pngFile("bomb.png", 30_000, 30_000);
    expect(bomb.size).toBe(33);
    pick(form, bomb);
    await flush();
    expect(status()).toBe(PICTURE_WORDS.imageTooBig);
    expect(t.decode).not.toHaveBeenCalled();
    // One side too long, or too many pixels in all.
    pick(form, pngFile("wide.png", 8_193, 1));
    await flush();
    expect(status()).toBe(PICTURE_WORDS.imageTooBig);
    pick(form, pngFile("square.png", 7_000, 7_000));
    await flush();
    expect(status()).toBe(PICTURE_WORDS.imageTooBig);
    expect(t.decode).not.toHaveBeenCalled();
    // A header that holds no size is refused as not being a picture.
    pick(form, new File([new Uint8Array(40)], "zeros.png", { type: "image/png" }));
    await flush();
    expect(status()).toBe(PICTURE_WORDS.imageFailed);
    expect(t.decode).not.toHaveBeenCalled();
    // A GIF whose screen is 1 by 1 and whose frame is 16000 by 16000 (a few hundred bytes).
    const frame = (w: number, h: number) => [0x2c, 0, 0, 0, 0, w & 255, w >> 8, h & 255, h >> 8, 0, 2, 2, 0x44, 0x01, 0];
    const gif = (...frames: number[][]) =>
      new Uint8Array([0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 255, 255, 255, ...frames.flat(), 0x3b]);
    pick(form, new File([gif(frame(16_000, 16_000)) as BlobPart], "bomb.gif", { type: "image/gif" }));
    await flush();
    expect(status()).toBe(PICTURE_WORDS.imageTooBig);
    // A GIF that is cut short is refused as not being read.
    pick(form, new File([gif(frame(10, 10)).subarray(0, 28) as BlobPart], "cut.gif", { type: "image/gif" }));
    await flush();
    expect(status()).toBe(PICTURE_WORDS.imageFailed);
    expect(t.decode).not.toHaveBeenCalled();
    // At the limit it is read.
    pick(form, pngFile("limit.png", 6_000, 6_000));
    await flush();
    expect(t.decode).toHaveBeenCalledTimes(1);
  });
});

describe("saved signatures", () => {
  it("are listed with a way to use, rename and delete each", async () => {
    const m = memoryStore([savedDraw("a", "First"), { id: "b", name: "Second", created: 2, kind: "type", font: "allura", text: "Ann" }]);
    const { form } = open(tools({ store: m.store }));
    button(form, "Saved").click();
    await flush();
    const rows = form.querySelectorAll(".sigpic-saved-row");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("First");
    expect(rows[1].querySelector(".sigpic-sample-text")?.textContent).toBe("Ann");
    expect(form.querySelector(".sigpic-sample svg polyline")).not.toBeNull();
    expect(form.textContent).toContain("Anyone who can read that folder can use them");
  });

  it("gives a saved drawing to the page as it was drawn", async () => {
    const m = memoryStore([savedDraw("a", "First")]);
    const { answer, form } = open(tools({ store: m.store }));
    button(form, "Saved").click();
    await flush();
    button(form, "Use First").click();
    const result = (await answer) as PictureAnswer;
    expect(result.picture).toMatchObject({ kind: "draw", width: 100, height: 40, line: 3, aspect: 2.5 });
  });

  it("gives a saved name with the aspect measured in its font", async () => {
    const m = memoryStore([{ id: "b", name: "Typed", created: 2, kind: "type", font: "allura", text: "Ann" }]);
    const t = tools({ store: m.store, measure: vi.fn(() => Promise.resolve(3.5)) });
    const { answer, form } = open(t);
    button(form, "Saved").click();
    await flush();
    button(form, "Use Typed").click();
    const result = (await answer) as PictureAnswer;
    expect(result.picture).toMatchObject({ kind: "type", font: "allura", text: "Ann", aspect: 3.5 });
    expect(t.measure).toHaveBeenCalledWith("Ann", "allura");
  });

  it("gives a saved image back as pixels, read by the window", async () => {
    const png = toBase64(new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10, 1, 2, 3]));
    const m = memoryStore([{ id: "i", name: "Scan", created: 3, kind: "image", width: 4, height: 2, png }]);
    const t = tools({ store: m.store });
    const { answer, form } = open(t);
    button(form, "Saved").click();
    await flush();
    expect(form.querySelector(".sigpic-sample-image")).not.toBeNull();
    button(form, "Use Scan").click();
    const result = (await answer) as PictureAnswer;
    expect(result.picture).toMatchObject({ kind: "image", width: 4, height: 2 });
    expect(t.decode).toHaveBeenCalledTimes(1);
  });

  it("is renamed and deleted through the store, one at a time", async () => {
    const m = memoryStore([savedDraw("a", "First"), savedDraw("b", "Second")]);
    const { form } = open(tools({ store: m.store }));
    button(form, "Saved").click();
    await flush();
    button(form, "Rename First").click();
    const input = form.querySelector<HTMLInputElement>("input.sigpic-rename")!;
    input.value = "  Formal ";
    button(form, "Rename").click();
    await flush();
    expect(m.calls).toEqual(["rename a Formal"]);
    expect(form.textContent).toContain("Formal");
    button(form, "Delete Second").click();
    await flush();
    expect(m.calls).toEqual(["rename a Formal", "remove b"]);
    expect(form.querySelectorAll(".sigpic-saved-row")).toHaveLength(1);
  });

  it("is not renamed to nothing", async () => {
    const m = memoryStore([savedDraw("a", "First")]);
    const { form } = open(tools({ store: m.store }));
    button(form, "Saved").click();
    await flush();
    button(form, "Rename First").click();
    form.querySelector<HTMLInputElement>("input.sigpic-rename")!.value = "   ";
    button(form, "Rename").click();
    await flush();
    expect(m.calls).toEqual([]);
    expect(form.textContent).toContain("A name has one to sixty characters.");
  });

  it("says so when there are none, and when saving is not possible", async () => {
    const none = open(tools({ store: memoryStore().store }));
    button(none.form, "Saved").click();
    await flush();
    expect(none.form.textContent).toContain("No signatures are saved yet.");
    layer.replaceChildren();
    const nostore = open(tools({ store: null }));
    button(nostore.form, "Saved").click();
    expect(nostore.form.textContent).toContain("Saved signatures are not available.");
    expect(nostore.form.querySelector(".sigpic-keep")?.hasAttribute("hidden")).toBe(true);
  });
});

describe("keeping a signature for later", () => {
  it("saves a drawing under the name that is given, and then hands it over", async () => {
    const m = memoryStore();
    const { answer, form, q } = open(tools({ store: m.store }));
    draw(q("canvas.sigpic-surface"), [[10, 10], [80, 40]]);
    const keep = form.querySelector<HTMLInputElement>('.sigpic-keep input[type="checkbox"]')!;
    keep.checked = true;
    keep.dispatchEvent(new Event("change", { bubbles: true }));
    q<HTMLInputElement>("input.sigpic-name").value = "Mine";
    form.requestSubmit();
    await answer;
    expect(m.calls).toEqual(["add Mine draw"]);
  });

  it("saves a typed name by its text and its font only", async () => {
    const m = memoryStore();
    const { answer, form, q } = open(tools({ store: m.store }));
    button(form, "Type").click();
    q<HTMLInputElement>("input.sigpic-text").value = "Anna";
    q<HTMLInputElement>("input.sigpic-text").dispatchEvent(new Event("input", { bubbles: true }));
    const keep = form.querySelector<HTMLInputElement>('.sigpic-keep input[type="checkbox"]')!;
    keep.checked = true;
    keep.dispatchEvent(new Event("change", { bubbles: true }));
    q<HTMLInputElement>("input.sigpic-name").value = "Typed";
    form.requestSubmit();
    await answer;
    expect(vi.mocked(m.store.add).mock.calls[0]).toEqual(["Typed", { kind: "type", font: "great-vibes", text: "Anna" }]);
  });

  it("saves an image as a PNG, made smaller until it fits", async () => {
    const m = memoryStore();
    let call = 0;
    const t = tools({
      store: m.store,
      encodePng: vi.fn(() => Promise.resolve(new Uint8Array(call++ < 2 ? MAX_SAVED_PNG_BYTES + 1 : 1_000))),
    });
    const { answer, form } = open(t);
    button(form, "Image").click();
    const input = form.querySelector<HTMLInputElement>("input.sigpic-file")!;
    Object.defineProperty(input, "files", { value: [pngFile("s.png")], configurable: true });
    input.dispatchEvent(new Event("change", { bubbles: true }));
    await flush();
    const keep = form.querySelector<HTMLInputElement>('.sigpic-keep input[type="checkbox"]')!;
    keep.checked = true;
    keep.dispatchEvent(new Event("change", { bubbles: true }));
    form.querySelector<HTMLInputElement>("input.sigpic-name")!.value = "Scan";
    form.requestSubmit();
    await answer;
    expect(t.scale).toHaveBeenCalledTimes(2);
    expect(t.encodePng).toHaveBeenCalledTimes(3);
    const [name, body] = vi.mocked(m.store.add).mock.calls[0];
    expect(name).toBe("Scan");
    expect(body.kind).toBe("image");
    expect(body.kind === "image" && body.png.startsWith("AAAA")).toBe(true);
    // The picture that is placed is the one that was chosen, not the smaller one that was kept.
    expect(t.decode).toHaveBeenCalledTimes(1);
  });

  it("stays open and says why when the name is missing or the store refuses", async () => {
    const m = memoryStore();
    vi.mocked(m.store.add).mockRejectedValueOnce({ code: "signature_store_full", message: "x" });
    const { answer, form, q } = open(tools({ store: m.store }));
    draw(q("canvas.sigpic-surface"), [[10, 10], [80, 40]]);
    const keep = form.querySelector<HTMLInputElement>('.sigpic-keep input[type="checkbox"]')!;
    keep.checked = true;
    keep.dispatchEvent(new Event("change", { bubbles: true }));
    form.requestSubmit();
    await flush();
    expect(q(".sigpic-keep .sigpic-status").textContent).toBe("Give the saved signature a name.");
    expect(layer.querySelector("form")).toBe(form);
    q<HTMLInputElement>("input.sigpic-name").value = "Mine";
    form.requestSubmit();
    await flush();
    expect(q(".sigpic-keep .sigpic-status").textContent).toBe(PICTURE_WORDS.storeFull);
    expect(layer.querySelector("form")).toBe(form);
    expect(button(form, "Use on the page").disabled).toBe(false);
    // Without the tick it goes on.
    keep.checked = false;
    keep.dispatchEvent(new Event("change", { bubbles: true }));
    form.requestSubmit();
    expect(((await answer) as PictureAnswer).picture.kind).toBe("draw");
  });

  it("refuses to save when as many are saved as can be", async () => {
    const full = Array.from({ length: MAX_SAVED }, (_, i) => savedDraw(`s${i}`, `Name ${i}`));
    const m = memoryStore(full);
    const { form, q } = open(tools({ store: m.store }));
    await flush();
    draw(q("canvas.sigpic-surface"), [[10, 10], [80, 40]]);
    const keep = form.querySelector<HTMLInputElement>('.sigpic-keep input[type="checkbox"]')!;
    keep.checked = true;
    keep.dispatchEvent(new Event("change", { bubbles: true }));
    q<HTMLInputElement>("input.sigpic-name").value = "One more";
    form.requestSubmit();
    await flush();
    expect(m.store.add).not.toHaveBeenCalled();
    expect(q(".sigpic-keep .sigpic-status").textContent).toBe(PICTURE_WORDS.storeFull);
  });
});

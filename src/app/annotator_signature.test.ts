// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Annotation } from "../lib/annotations";
import type { Chosen, Rect } from "../lib/signpicture";
import type { DocumentView } from "../viewer/viewer";
import { Annotator, PAGES_BUSY_NOTICE, type AnnotatorApi, type AnnotatorHost } from "./annotator";
import { EditHistory } from "./history";

const INK: [number, number, number, number] = [20, 20, 22, 255];

function drawn(): Chosen {
  return { kind: "draw", width: 100, height: 40, line: 3, strokes: [[[5, 5], [90, 30]]], color: INK, aspect: 2.5 };
}

function imported(): Chosen {
  return { kind: "image", width: 4, height: 2, rgba: new Uint8ClampedArray(32).fill(9), color: INK, aspect: 2 };
}

function signature(id: number, rect: Rect, over: Partial<Annotation["can"]> = {}): Annotation {
  return {
    id,
    kind: "signature",
    can: { editText: false, recolour: false, move: true, remove: true, restore: true, locked: false, resize: true, ...over },
    rect,
    color: INK,
    text: "Signature picture",
    author: "",
  };
}

function setup(initial: Annotation[] = []) {
  const list = new Map<string, Annotation>();
  for (const a of initial) list.set(`0:${a.id}`, a);
  const layer = {
    setInk: vi.fn(),
    setSelected: vi.fn(),
    abort: vi.fn(() => false),
    endStroke: vi.fn(),
    setReading: vi.fn(),
    find: (page: number, id: number) => list.get(`${page}:${id}`),
    boxElement: () => null,
  };
  const raw = {
    annotationLayer: layer,
    hasSelection: false,
    setAnnotationTool: vi.fn(),
    invalidatePage: vi.fn(),
    whenRedrawn: vi.fn(() => Promise.resolve()),
    pageSizeOf: () => ({ width: 612, height: 792 }),
    centreOfView: () => ({ page: 0, x: 300, y: 400 }),
  };
  const history = new EditHistory();
  const calls: string[] = [];
  let next = 100;
  const api: AnnotatorApi = {
    addMarkup: vi.fn(),
    addNote: vi.fn(),
    addInk: vi.fn(),
    addStamp: vi.fn(),
    updateAnnotation: vi.fn((_d, page, id, change) => {
      calls.push(`update ${page} ${id} ${JSON.stringify(change)}`);
      const old = list.get(`${page}:${id}`) as Annotation;
      const rect: Rect = [old.rect[0] + (change.dx ?? 0), old.rect[1] + (change.dy ?? 0), old.rect[2] + (change.dx ?? 0), old.rect[3] + (change.dy ?? 0)];
      const moved = { ...old, rect };
      list.set(`${page}:${id}`, moved);
      return Promise.resolve(moved);
    }),
    removeAnnotation: vi.fn((_d, page, id) => {
      calls.push(`remove ${page} ${id}`);
      return Promise.resolve();
    }),
    restoreAnnotation: vi.fn((_d, page, id) => {
      calls.push(`restore ${page} ${id}`);
      return Promise.resolve(list.get(`${page}:${id}`) ?? signature(id, [0, 0, 10, 10]));
    }),
    addSignature: vi.fn((_d: number, page: number, rect: Rect, _c: unknown, picture: { kind: string }) => {
      calls.push(`addSignature ${page} ${rect.map((v) => Math.round(v)).join(",")} ${picture.kind}`);
      const a = signature(next++, rect);
      list.set(`${page}:${a.id}`, a);
      return Promise.resolve(a);
    }),
    addSignatureImage: vi.fn((_d: number, page: number, rect: Rect, _c: unknown, w: number, h: number) => {
      calls.push(`addSignatureImage ${page} ${rect.map((v) => Math.round(v)).join(",")} ${w}x${h}`);
      const a = signature(next++, rect);
      list.set(`${page}:${a.id}`, a);
      return Promise.resolve(a);
    }),
    // The engine keeps the corner and the width, and the proportions the picture has.
    placeSignature: vi.fn((_d: number, page: number, id: number, rect: Rect) => {
      calls.push(`place ${page} ${id} ${rect.map((v) => Math.round(v * 10) / 10).join(",")}`);
      const old = list.get(`${page}:${id}`) as Annotation;
      const aspect = (old.rect[2] - old.rect[0]) / (old.rect[3] - old.rect[1]);
      const w = rect[2] - rect[0];
      const placed = { ...old, rect: [rect[0], rect[1], rect[0] + w, rect[1] + w / aspect] as Rect };
      list.set(`${page}:${id}`, placed);
      return Promise.resolve(placed);
    }),
  };
  const popovers = document.createElement("div");
  document.body.append(popovers);
  const host: AnnotatorHost & { told: string[]; failures: unknown[] } = {
    popovers,
    told: [],
    failures: [],
    announce(text) {
      this.told.push(text);
    },
    ask: () => Promise.resolve({ button: "delete" }),
    failed(error) {
      this.failures.push(error);
    },
    changed: vi.fn(),
    pageChanged: vi.fn(),
  };
  const annotator = new Annotator(7, raw as unknown as DocumentView, history, api, host);
  return { annotator, history, api, host, calls, raw, list, layer };
}

beforeEach(() => {
  document.body.replaceChildren();
});

describe("placing a picture of a signature", () => {
  it("puts the picture the person made where they click, as one edit that undo takes back and redo puts back", async () => {
    const t = setup();
    t.annotator.setSignature(drawn());
    expect(t.annotator.tool).toBe("signature");
    expect(t.raw.setAnnotationTool).toHaveBeenLastCalledWith("signature");
    t.annotator.onPlace(0, 300, 400, { x: 0, y: 0 });
    await t.annotator.idle();
    // 160 points wide, 64 high (the proportions of 100 by 40), its middle at the click.
    expect(t.calls).toEqual(["addSignature 0 220,368,380,432 draw"]);
    expect(t.api.addSignature).toHaveBeenCalledWith(7, 0, [220, 368, 380, 432], INK, expect.objectContaining({ kind: "draw", width: 100 }));
    expect(t.history.dirty).toBe(true);
    expect(t.history.undoLabel).toBe("Signature picture");
    expect(t.host.told.at(-1)).toContain("Signature picture placed.");
    // The pointer is back to picking, and the new picture is the one picked.
    expect(t.annotator.tool).toBe("select");
    expect(t.annotator.selected).toEqual({ page: 0, id: 100 });
    expect(t.raw.invalidatePage).toHaveBeenCalledWith(0);
    expect(t.host.pageChanged).toHaveBeenCalledWith(0);

    await t.history.undo();
    expect(t.calls.at(-1)).toBe("remove 0 100");
    expect(t.history.dirty).toBe(false);
    await t.history.redo();
    expect(t.calls.at(-1)).toBe("restore 0 100");
    expect(t.history.dirty).toBe(true);
  });

  it("sends an imported picture as pixels, with its size", async () => {
    const t = setup();
    t.annotator.setSignature(imported());
    t.annotator.onPlace(0, 100, 100, { x: 0, y: 0 });
    await t.annotator.idle();
    expect(t.api.addSignatureImage).toHaveBeenCalledTimes(1);
    const args = vi.mocked(t.api.addSignatureImage!).mock.calls[0];
    expect(args.slice(0, 2)).toEqual([7, 0]);
    expect(args[4]).toBe(4);
    expect(args[5]).toBe(2);
    expect(Array.from(args[6])).toEqual(Array(32).fill(9));
    expect(t.api.addSignature).not.toHaveBeenCalled();
  });

  it("places at the middle of what is shown when Enter is pressed", async () => {
    const t = setup();
    t.annotator.setSignature(drawn());
    expect(t.annotator.handleKey(new KeyboardEvent("keydown", { key: "Enter" }))).toBe(true);
    await t.annotator.idle();
    expect(t.calls).toEqual(["addSignature 0 220,368,380,432 draw"]);
  });

  it("does nothing when no picture was made", async () => {
    const t = setup();
    t.annotator.setTool("signature");
    t.annotator.onPlace(0, 100, 100, { x: 0, y: 0 });
    await t.annotator.idle();
    expect(t.calls).toEqual([]);
    expect(t.history.dirty).toBe(false);
  });

  it("makes no edit when the engine refuses, and reports it", async () => {
    const t = setup();
    const refusal = { code: "annotation_refused", message: "no" };
    vi.mocked(t.api.addSignature!).mockRejectedValueOnce(refusal);
    t.annotator.setSignature(drawn());
    t.annotator.onPlace(0, 300, 400, { x: 0, y: 0 });
    await t.annotator.idle();
    expect(t.history.canUndo).toBe(false);
    expect(t.history.dirty).toBe(false);
    expect(t.host.failures).toEqual([refusal]);
  });

  it("marks the document as changed when a failure is not a clean refusal", async () => {
    const t = setup();
    vi.mocked(t.api.addSignatureImage!).mockRejectedValueOnce({ code: "engine_stopped", message: "gone" });
    t.annotator.setSignature(imported());
    t.annotator.onPlace(0, 300, 400, { x: 0, y: 0 });
    await t.annotator.idle();
    expect(t.history.dirty).toBe(true);
    expect(t.host.failures).toHaveLength(1);
  });

  it("says when the app cannot place pictures", async () => {
    const t = setup();
    delete (t.api as { addSignature?: unknown }).addSignature;
    t.annotator.setSignature(drawn());
    t.annotator.onPlace(0, 300, 400, { x: 0, y: 0 });
    await t.annotator.idle();
    expect(t.host.told).toContain("Signature pictures are not available.");
  });
});

describe("the pages are being changed", () => {
  it("holds the tool back: nothing is placed, and the person is told why", async () => {
    const t = setup();
    t.annotator.setSignature(drawn());
    t.annotator.setHeld(true);
    t.annotator.onPlace(0, 300, 400, { x: 0, y: 0 });
    await t.annotator.idle();
    expect(t.calls).toEqual([]);
    expect(t.host.told).toContain(PAGES_BUSY_NOTICE);
    expect(t.history.dirty).toBe(false);
    // Nothing stays begun: going on works again.
    t.annotator.setHeld(false);
    t.annotator.onPlace(0, 300, 400, { x: 0, y: 0 });
    await t.annotator.idle();
    expect(t.calls).toEqual(["addSignature 0 220,368,380,432 draw"]);
  });

  it("does not switch the tool on while it is held", () => {
    const t = setup();
    t.annotator.setHeld(true);
    t.annotator.setSignature(drawn());
    expect(t.annotator.tool).toBe("select");
    expect(t.host.told).toContain(PAGES_BUSY_NOTICE);
    expect(t.annotator.blockedReason).toBe(PAGES_BUSY_NOTICE);
  });

  it("does not resize while it is held, with the pointer or with the keys", async () => {
    const t = setup([signature(5, [100, 100, 260, 164])]);
    t.annotator.select(0, 5);
    t.annotator.setHeld(true);
    await t.annotator.resizeTo(0, 5, [100, 100, 300, 100]);
    expect(t.annotator.handleKey(new KeyboardEvent("keydown", { key: "+" }))).toBe(true);
    await t.annotator.idle();
    expect(t.calls).toEqual([]);
    expect(t.host.told).toContain(PAGES_BUSY_NOTICE);
  });

  it("drops a drag in progress when the hold begins", () => {
    const t = setup();
    t.annotator.setHeld(true);
    expect(t.layer.abort).toHaveBeenCalled();
  });
});

describe("making a picture bigger or smaller", () => {
  const start: Rect = [100, 100, 260, 164];

  it("asks the engine, and keeps the box it answers with for undo and redo", async () => {
    const t = setup([signature(5, start)]);
    await t.annotator.resizeTo(0, 5, [100, 100, 340, 100]);
    expect(t.calls).toEqual(["place 0 5 100,100,340,100"]);
    expect(t.history.undoLabel).toBe("Resize signature picture");
    expect(t.history.dirty).toBe(true);
    // The engine kept the proportions: 240 wide, 96 high.
    expect(t.list.get("0:5")?.rect).toEqual([100, 100, 340, 196]);
    await t.history.undo();
    expect(t.calls.at(-1)).toBe("place 0 5 100,100,260,164");
    expect(t.history.dirty).toBe(false);
    await t.history.redo();
    expect(t.calls.at(-1)).toBe("place 0 5 100,100,340,196");
  });

  it("makes no edit when the box did not change", async () => {
    const t = setup([signature(5, start)]);
    await t.annotator.resizeTo(0, 5, [100, 100, 260, 100]);
    expect(t.history.canUndo).toBe(false);
    expect(t.history.dirty).toBe(false);
  });

  it("is not done to a picture that cannot be resized, nor to one that is not there", async () => {
    const t = setup([signature(5, start, { resize: false }), { ...signature(6, start, { resize: false }), kind: "stamp" }]);
    await t.annotator.resizeTo(0, 5, [100, 100, 340, 100]);
    await t.annotator.resizeTo(0, 6, [100, 100, 340, 100]);
    await t.annotator.resizeTo(0, 99, [100, 100, 340, 100]);
    expect(t.calls).toEqual([]);
  });

  it("is done by the plus and minus keys in steps of a tenth", async () => {
    const t = setup([signature(5, start)]);
    t.annotator.select(0, 5);
    expect(t.annotator.handleKey(new KeyboardEvent("keydown", { key: "+" }))).toBe(true);
    await t.annotator.idle();
    expect(t.calls).toEqual(["place 0 5 100,100,276,170.4"]);
    expect(t.annotator.handleKey(new KeyboardEvent("keydown", { key: "-" }))).toBe(true);
    await t.annotator.idle();
    expect(t.calls).toHaveLength(2);
    expect(t.history.undoLabel).toBe("Resize signature picture");
  });

  it("leaves the plus key to the page for anything that cannot be resized", () => {
    const t = setup([signature(5, start, { resize: false })]);
    t.annotator.select(0, 5);
    expect(t.annotator.handleKey(new KeyboardEvent("keydown", { key: "+" }))).toBe(false);
  });

  it("undoes in the order things were done: the resize before the placing", async () => {
    const t = setup();
    t.annotator.setSignature(drawn());
    t.annotator.onPlace(0, 300, 400, { x: 0, y: 0 });
    await t.annotator.idle();
    await t.annotator.resizeTo(0, 100, [220, 368, 440, 368]);
    expect(t.history.undoLabel).toBe("Resize signature picture");
    await t.history.undo();
    expect(t.history.undoLabel).toBe("Signature picture");
    await t.history.undo();
    expect(t.calls.at(-1)).toBe("remove 0 100");
  });
});

describe("moving a picture", () => {
  it("is done like the move of a note, and taken back like it", async () => {
    const t = setup([signature(5, [100, 100, 260, 164])]);
    t.annotator.onMove(0, 5, 30, 40);
    await t.annotator.idle();
    expect(t.calls).toEqual(['update 0 5 {"dx":30,"dy":40}']);
    expect(t.history.undoLabel).toBe("Move");
    await t.history.undo();
    expect(t.calls.at(-1)).toBe('update 0 5 {"dx":-30,"dy":-40}');
  });

  it("is done with the arrow keys", async () => {
    const t = setup([signature(5, [100, 100, 260, 164])]);
    t.annotator.select(0, 5);
    expect(t.annotator.handleKey(new KeyboardEvent("keydown", { key: "ArrowRight" }))).toBe(true);
    await t.annotator.idle();
    expect(t.calls).toEqual(['update 0 5 {"dx":4,"dy":0}']);
  });

  it("is deleted with Delete and put back by undo, without a question", async () => {
    const t = setup([signature(5, [100, 100, 260, 164])]);
    t.annotator.select(0, 5);
    t.annotator.handleKey(new KeyboardEvent("keydown", { key: "Delete" }));
    await t.annotator.idle();
    await vi.waitFor(() => expect(t.calls).toContain("remove 0 5"));
    expect(t.history.undoLabel).toBe("Delete signature picture");
    await t.history.undo();
    expect(t.calls.at(-1)).toBe("restore 0 5");
  });
});

describe("the words about a picture", () => {
  it("do not offer to edit text for it, and say what it is when it is picked", () => {
    const t = setup([signature(5, [100, 100, 260, 164])]);
    t.annotator.select(0, 5);
    expect(t.annotator.pickedReason).toBeNull();
    expect(t.host.told.at(-1)).toContain("Signature picture selected on page 1.");
  });
});

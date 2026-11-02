// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Annotation, Rgba } from "../lib/annotations";
import type { DocumentView } from "../viewer/viewer";
import { Annotator, type AnnotatorApi, type AnnotatorHost } from "./annotator";
import { EditHistory } from "./history";

const YELLOW: Rgba = [255, 235, 59, 255];

function annotation(over: Partial<Annotation> = {}): Annotation {
  return {
    id: 1,
    kind: "highlight",
    can: { editText: true, recolour: true, move: false, remove: true, restore: true },
    rect: [10, 20, 30, 40],
    color: YELLOW,
    text: "",
    author: "",
    ...over,
  };
}

/** The parts of a view that the annotator uses, with a list of what it was asked to do. */
function fakeView(initial: Annotation[] = []) {
  const log: string[] = [];
  const list = new Map<string, Annotation>();
  for (const a of initial) list.set(`0:${a.id}`, a);
  let ranges: { page: number; start: number; end: number }[] = [];
  let truncated = false;
  const layer = {
    setInk: vi.fn(),
    setSelected: vi.fn(),
    abort: vi.fn(() => false),
    endStroke: vi.fn(),
    find: (page: number, id: number) => list.get(`${page}:${id}`),
    boxElement: () => null,
  };
  const view = {
    annotationLayer: layer,
    hasSelection: true,
    setAnnotationTool: vi.fn((tool: string) => log.push(`tool:${tool}`)),
    selectedRanges: vi.fn(() => Promise.resolve({ ranges, truncated })),
    clearSelection: vi.fn(() => log.push("clearSelection")),
    invalidatePage: vi.fn((page: number) => log.push(`invalidate:${page}`)),
    whenRedrawn: vi.fn(() => Promise.resolve()),
    pageSizeOf: () => ({ width: 612, height: 792 }),
    centreOfView: () => ({ page: 0, x: 300, y: 400 }),
  };
  return {
    view: view as unknown as DocumentView,
    raw: view,
    layer,
    log,
    list,
    setRanges(r: typeof ranges, t = false) {
      ranges = r;
      truncated = t;
    },
  };
}

function setup(initial: Annotation[] = []) {
  const v = fakeView(initial);
  const history = new EditHistory();
  let next = 100;
  const calls: string[] = [];
  const api: AnnotatorApi = {
    addMarkup: vi.fn((_d, page, kind, _c, start, end) => {
      calls.push(`addMarkup ${page} ${kind} ${start}-${end}`);
      return Promise.resolve(annotation({ id: next++, kind }));
    }),
    addNote: vi.fn((_d, page, x, y, _c, text) => {
      calls.push(`addNote ${page} ${Math.round(x)},${Math.round(y)} ${text}`);
      const a = annotation({ id: next++, kind: "note", text, can: { editText: true, recolour: false, move: true, remove: true, restore: true } });
      v.list.set(`${page}:${a.id}`, a);
      return Promise.resolve(a);
    }),
    addInk: vi.fn((_d, page, _c, width, strokes) => {
      calls.push(`addInk ${page} ${width} ${strokes.length}`);
      return Promise.resolve(annotation({ id: next++, kind: "ink" }));
    }),
    addStamp: vi.fn((_d, page, x, y, stamp) => {
      calls.push(`addStamp ${page} ${stamp} ${Math.round(x)},${Math.round(y)}`);
      return Promise.resolve(annotation({ id: next++, kind: "stamp" }));
    }),
    updateAnnotation: vi.fn((_d, page, id, change) => {
      calls.push(`update ${page} ${id} ${JSON.stringify(change)}`);
      return Promise.resolve(annotation({ id }));
    }),
    removeAnnotation: vi.fn((_d, page, id) => {
      calls.push(`remove ${page} ${id}`);
      return Promise.resolve();
    }),
    restoreAnnotation: vi.fn((_d, page, id) => {
      calls.push(`restore ${page} ${id}`);
      return Promise.resolve(annotation({ id }));
    }),
  };
  const popovers = document.createElement("div");
  document.body.append(popovers);
  const host: AnnotatorHost & { asked: string[]; told: string[]; failures: unknown[]; answer: string } = {
    popovers,
    asked: [],
    told: [],
    failures: [],
    answer: "delete",
    announce(text) {
      this.told.push(text);
    },
    ask(options) {
      this.asked.push(options.title);
      return Promise.resolve({ button: this.answer });
    },
    failed(error) {
      this.failures.push(error);
    },
    changed: vi.fn(),
    pageChanged: vi.fn(),
  };
  const annotator = new Annotator(7, v.view, history, api, host);
  return { annotator, history, api, host, calls, ...v };
}

beforeEach(() => {
  document.body.replaceChildren();
});

describe("marking the selected text", () => {
  it("adds one annotation for each page of the selection, as one edit that undo takes back and redo puts back", async () => {
    const t = setup();
    t.setRanges([
      { page: 0, start: 4, end: 19 },
      { page: 1, start: 0, end: 8 },
    ]);
    await t.annotator.markSelection("highlight");
    expect(t.calls).toEqual(["addMarkup 0 highlight 4-19", "addMarkup 1 highlight 0-8"]);
    expect(t.history.dirty).toBe(true);
    expect(t.history.undoLabel).toBe("Highlight");
    expect(t.log).toContain("clearSelection");
    expect(t.host.told.at(-1)).toBe("Highlight added on pages 1, 2.");

    t.calls.length = 0;
    await t.history.undo();
    expect(t.calls).toEqual(["remove 1 101", "remove 0 100"]);
    expect(t.history.dirty).toBe(false);
    await t.history.redo();
    expect(t.calls.slice(2)).toEqual(["restore 0 100", "restore 1 101"]);
    expect(t.history.dirty).toBe(true);
    // Every page is drawn again after each of them.
    expect(t.raw.invalidatePage).toHaveBeenCalledWith(0);
    expect(t.raw.invalidatePage).toHaveBeenCalledWith(1);
    expect(t.host.pageChanged).toHaveBeenCalledWith(1);
  });

  it("uses the colour that is chosen", async () => {
    const t = setup();
    t.setRanges([{ page: 0, start: 0, end: 3 }]);
    t.annotator.setColor([64, 156, 255, 255]);
    await t.annotator.markSelection("underline");
    expect(t.api.addMarkup).toHaveBeenCalledWith(7, 0, "underline", [64, 156, 255, 255], 0, 3);
  });

  it("does nothing, and says so, when no text is selected", async () => {
    const t = setup();
    t.setRanges([]);
    await t.annotator.markSelection("highlight");
    expect(t.calls).toEqual([]);
    expect(t.history.dirty).toBe(false);
    expect(t.host.told).toContain("Select some text first.");
  });

  it("refuses a selection of too many pages with a question the person answers", async () => {
    const t = setup();
    t.setRanges([{ page: 0, start: 0, end: 4 }], true);
    await t.annotator.markSelection("highlight");
    expect(t.calls).toEqual([]);
    expect(t.host.asked).toEqual(["Too much text is selected"]);
  });

  it("makes no edit when the engine refuses, and reports it", async () => {
    const t = setup();
    t.setRanges([{ page: 0, start: 0, end: 4 }]);
    const refusal = { code: "annotation_refused", message: "no" };
    vi.mocked(t.api.addMarkup).mockRejectedValueOnce(refusal);
    await t.annotator.markSelection("highlight");
    expect(t.history.dirty).toBe(false);
    expect(t.history.canUndo).toBe(false);
    expect(t.host.failures).toEqual([refusal]);
  });

  it("keeps the pages that were marked when a later page fails", async () => {
    const t = setup();
    t.setRanges([
      { page: 0, start: 0, end: 4 },
      { page: 1, start: 0, end: 4 },
    ]);
    vi.mocked(t.api.addMarkup).mockImplementationOnce(() => Promise.resolve(annotation({ id: 50 })));
    vi.mocked(t.api.addMarkup).mockRejectedValueOnce({ code: "annotation_refused", message: "no" });
    await t.annotator.markSelection("strikeout");
    // The first page was marked, so it is an edit that can be taken back; the failure is reported.
    expect(t.history.dirty).toBe(true);
    expect(t.host.failures).toHaveLength(1);
    await t.history.undo();
    expect(t.calls).toContain("remove 0 50");
  });

  it("marks the selection at once when a markup tool is picked", async () => {
    const t = setup();
    t.setRanges([{ page: 0, start: 2, end: 6 }]);
    t.annotator.setTool("highlight");
    await t.annotator.idle();
    await vi.waitFor(() => expect(t.calls).toEqual(["addMarkup 0 highlight 2-6"]));
  });
});

describe("notes, stamps and drawings", () => {
  it("adds a drawing, with the colour and the width of the bar, and clears the line afterwards", async () => {
    const t = setup();
    t.annotator.setPenWidth(6);
    t.annotator.onStroke(2, [
      [1, 2],
      [3, 4],
    ]);
    await t.annotator.idle();
    expect(t.calls).toEqual(["addInk 2 6 1"]);
    expect(t.layer.endStroke).toHaveBeenCalledWith(2);
    expect(t.history.undoLabel).toBe("Drawing");
  });

  it("keeps the stamp on the page and picks it", async () => {
    const t = setup();
    t.annotator.setStamp("confidential");
    expect(t.annotator.tool).toBe("stamp");
    t.annotator.onPlace(0, 5, 5, { x: 0, y: 0 });
    await vi.waitFor(() => expect(t.calls).toHaveLength(1));
    await t.annotator.idle();
    // Centred on the click, then pushed back onto the page.
    expect(t.calls[0]).toMatch(/^addStamp 0 confidential 0,0$/);
    expect(t.annotator.tool).toBe("select");
    expect(t.annotator.selected).toEqual({ page: 0, id: 100 });
  });

  it("asks for the text of a note in a box, adds it when it is saved and not when it is left", async () => {
    const t = setup();
    t.annotator.setTool("note");
    t.annotator.onPlace(0, 100, 200, { x: 50, y: 60 });
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    expect(area).not.toBeNull();
    expect(t.host.popovers.querySelector("[role=dialog]")?.getAttribute("aria-label")).toBe("New note");
    area.value = "Hello <b>world</b>";
    area.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true }));
    await vi.waitFor(() => expect(t.calls).toEqual(["addNote 0 90,190 Hello <b>world</b>"]));
    expect(t.host.popovers.querySelector("[role=dialog]")).toBeNull();
    expect(t.history.dirty).toBe(true);

    // Left with Escape: no note, nothing to undo beyond the first.
    t.annotator.setTool("note");
    t.annotator.onPlace(0, 10, 10, { x: 0, y: 0 });
    (t.host.popovers.querySelector("textarea") as HTMLTextAreaElement).dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
    );
    await t.annotator.idle();
    expect(t.calls).toHaveLength(1);
  });

  it("cuts the text of a note to the length the engine takes, in bytes", async () => {
    const t = setup();
    t.annotator.setTool("note");
    t.annotator.onPlace(0, 10, 10, { x: 0, y: 0 });
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    area.value = "✓".repeat(2000);
    area.dispatchEvent(new Event("input"));
    expect(new TextEncoder().encode(area.value).length).toBeLessThanOrEqual(4096);
    area.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true }));
    await vi.waitFor(() => expect(t.calls).toHaveLength(1));
  });
});

describe("changing and removing", () => {
  const withNote = () => {
    const note = annotation({
      id: 5,
      kind: "note",
      text: "before",
      can: { editText: true, recolour: false, move: true, remove: true, restore: true },
    });
    const t = setup([note]);
    t.annotator.select(0, 5);
    return t;
  };

  it("moves a note, and undo moves it back by the same amount", async () => {
    const t = withNote();
    await t.annotator.moveBy(0, 5, 12, -3);
    expect(t.calls).toEqual(['update 0 5 {"dx":12,"dy":-3}']);
    await t.history.undo();
    expect(t.calls[1]).toBe('update 0 5 {"dx":-12,"dy":3}');
    await t.history.redo();
    expect(t.calls[2]).toBe('update 0 5 {"dx":12,"dy":-3}');
  });

  it("does not move what cannot be moved, nor by nothing", async () => {
    const t = setup([annotation({ id: 9 })]);
    await t.annotator.moveBy(0, 9, 5, 5);
    const n = withNote();
    await n.annotator.moveBy(0, 5, 0, 0);
    expect(t.calls).toEqual([]);
    expect(n.calls).toEqual([]);
  });

  it("changes the text and takes it back to the old text", async () => {
    const t = withNote();
    const done = t.annotator.editText(0, 5);
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    expect(area.value).toBe("before");
    area.value = "after";
    (t.host.popovers.querySelector("form") as HTMLFormElement).dispatchEvent(new Event("submit", { cancelable: true }));
    await done;
    await t.annotator.idle();
    expect(t.calls).toEqual(['update 0 5 {"text":"after"}']);
    await t.history.undo();
    expect(t.calls[1]).toBe('update 0 5 {"text":"before"}');
  });

  it("makes no edit when the text did not change", async () => {
    const t = withNote();
    const done = t.annotator.editText(0, 5);
    (t.host.popovers.querySelector("form") as HTMLFormElement).dispatchEvent(new Event("submit", { cancelable: true }));
    await done;
    await t.annotator.idle();
    expect(t.calls).toEqual([]);
    expect(t.history.dirty).toBe(false);
  });

  it("deletes the picked annotation and undo restores it with the same id", async () => {
    const t = withNote();
    await t.annotator.removeSelected();
    expect(t.calls).toEqual(["remove 0 5"]);
    expect(t.history.undoLabel).toBe("Delete note");
    expect(t.host.asked).toEqual([]);
    await t.history.undo();
    expect(t.calls[1]).toBe("restore 0 5");
    expect(t.history.dirty).toBe(false);
    await t.history.redo();
    expect(t.calls[2]).toBe("remove 0 5");
  });

  it("asks before deleting what undo cannot bring back, and does nothing on Cancel", async () => {
    const drawing = annotation({
      id: 8,
      kind: "ink",
      can: { editText: true, recolour: false, move: false, remove: true, restore: false },
    });
    const t = setup([drawing]);
    t.annotator.select(0, 8);
    t.host.answer = "cancel";
    await t.annotator.removeSelected();
    expect(t.host.asked).toEqual(["Delete this annotation?"]);
    expect(t.calls).toEqual([]);
    t.host.answer = "delete";
    await t.annotator.removeSelected();
    expect(t.calls).toEqual(["remove 0 8"]);
    expect(t.history.dirty).toBe(true);
  });

  it("recolours a picked highlight when a colour is chosen, once, and not for the same colour", async () => {
    const t = setup([annotation({ id: 3 })]);
    t.annotator.select(0, 3);
    t.annotator.setColor(YELLOW);
    await t.annotator.idle();
    expect(t.calls).toEqual([]);
    t.annotator.setColor([126, 217, 87, 255]);
    await vi.waitFor(() => expect(t.calls).toEqual(['update 0 3 {"color":[126,217,87,255]}']));
    await t.history.undo();
    expect(t.calls[1]).toBe('update 0 3 {"color":[255,235,59,255]}');
  });

  it("reports a failed change and leaves the history alone", async () => {
    const t = withNote();
    vi.mocked(t.api.updateAnnotation).mockRejectedValueOnce({ code: "no_such_annotation", message: "gone" });
    await t.annotator.moveBy(0, 5, 4, 0);
    expect(t.history.canUndo).toBe(false);
    expect(t.host.failures).toHaveLength(1);
  });

  it("serialises changes: the second waits for the first", async () => {
    const t = withNote();
    const order: string[] = [];
    let release: () => void = () => undefined;
    vi.mocked(t.api.updateAnnotation).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          order.push("first started");
          release = () => {
            order.push("first done");
            resolve(annotation({ id: 5 }));
          };
        }),
    );
    const first = t.annotator.moveBy(0, 5, 1, 0);
    const second = t.annotator.moveBy(0, 5, 2, 0);
    await Promise.resolve();
    expect(t.api.updateAnnotation).toHaveBeenCalledTimes(1);
    release();
    await first;
    await second;
    expect(t.api.updateAnnotation).toHaveBeenCalledTimes(2);
    expect(order).toEqual(["first started", "first done"]);
  });
});

describe("the keyboard", () => {
  const key = (k: string, extra: KeyboardEventInit = {}) => new KeyboardEvent("keydown", { key: k, ...extra });

  it("Delete takes the picked annotation off, the arrows move a note, Enter opens its text", async () => {
    const note = annotation({
      id: 5,
      kind: "note",
      text: "x",
      can: { editText: true, recolour: false, move: true, remove: true, restore: true },
    });
    const t = setup([note]);
    t.annotator.select(0, 5);
    expect(t.annotator.handleKey(key("ArrowRight"))).toBe(true);
    expect(t.annotator.handleKey(key("ArrowDown", { shiftKey: true }))).toBe(true);
    await t.annotator.idle();
    expect(t.calls).toEqual(['update 0 5 {"dx":4,"dy":0}', 'update 0 5 {"dx":0,"dy":24}']);
    expect(t.annotator.handleKey(key("Enter"))).toBe(true);
    expect(t.host.popovers.querySelector("[role=dialog]")).not.toBeNull();
    t.annotator.handleKey(key("Escape"));
    expect(t.host.popovers.querySelector("[role=dialog]")).toBeNull();
    expect(t.annotator.handleKey(key("Delete"))).toBe(true);
    await vi.waitFor(() => expect(t.calls.at(-1)).toBe("remove 0 5"));
  });

  it("leaves keys alone when nothing is picked", () => {
    const t = setup();
    for (const k of ["Delete", "Backspace", "Enter", "ArrowLeft", "ArrowUp"]) expect(t.annotator.handleKey(key(k))).toBe(false);
  });

  it("does not take the arrows for what cannot move", () => {
    const t = setup([annotation({ id: 2 })]);
    t.annotator.select(0, 2);
    expect(t.annotator.handleKey(key("ArrowLeft"))).toBe(false);
  });

  it("Escape lets go of the annotation first and then of the tool", () => {
    const t = setup([annotation({ id: 2 })]);
    t.annotator.select(0, 2);
    t.annotator.setTool("pen");
    expect(t.annotator.handleKey(key("Escape"))).toBe(true);
    expect(t.annotator.selected).toBeNull();
    expect(t.annotator.tool).toBe("pen");
    expect(t.annotator.handleKey(key("Escape"))).toBe(true);
    expect(t.annotator.tool).toBe("select");
    expect(t.annotator.handleKey(key("Escape"))).toBe(false);
  });

  it("Enter with the note tool puts a note in the middle of what is shown", async () => {
    const t = setup();
    t.annotator.setTool("note");
    expect(t.annotator.handleKey(key("Enter"))).toBe(true);
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    area.value = "keyboard";
    area.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true }));
    await vi.waitFor(() => expect(t.calls).toEqual(["addNote 0 290,390 keyboard"]));
  });
});

// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Annotation, Rgba } from "../lib/annotations";
import type { DocumentView } from "../viewer/viewer";
import { Annotator, annotationNotice, movedDistance, type AnnotatorApi, type AnnotatorHost } from "./annotator";
import { EditHistory } from "./history";

const YELLOW: Rgba = [255, 235, 59, 255];

function annotation(over: Partial<Annotation> = {}): Annotation {
  return {
    id: 1,
    kind: "highlight",
    can: { editText: true, recolour: true, move: false, remove: true, restore: true, locked: false },
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
    setReading: vi.fn(),
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
      const a = annotation({ id: next++, kind: "note", text, can: { editText: true, recolour: false, move: true, remove: true, restore: true, locked: false } });
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
      // A move shows in the box the layer holds, as it does once the page is listed again.
      const old = v.list.get(`${page}:${id}`);
      const [dx, dy] = [change.dx ?? 0, change.dy ?? 0];
      const rect: Annotation["rect"] = old ? [old.rect[0] + dx, old.rect[1] + dy, old.rect[2] + dx, old.rect[3] + dy] : [10, 20, 30, 40];
      const next = annotation({ ...(old ?? {}), id, rect });
      v.list.set(`${page}:${id}`, next);
      return Promise.resolve(next);
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
  const host: AnnotatorHost & { asked: string[]; details: string[]; told: string[]; failures: unknown[]; answer: string } = {
    popovers,
    asked: [],
    details: [],
    told: [],
    failures: [],
    answer: "delete",
    announce(text) {
      this.told.push(text);
    },
    ask(options) {
      this.asked.push(options.title);
      this.details.push(options.detail);
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
      can: { editText: true, recolour: false, move: true, remove: true, restore: true, locked: false },
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
      can: { editText: true, recolour: false, move: false, remove: true, restore: false, locked: false },
    });
    const t = setup([drawing]);
    t.annotator.select(0, 8);
    t.host.answer = "cancel";
    await t.annotator.removeSelected();
    expect(t.host.asked).toEqual(["Delete this annotation?"]);
    // The words say what is true: Undo cannot, Redo is cleared, a step back over a delete, a move or
    // added pages loads an older copy, and a turn or a crop never does.
    const words = t.host.details[0];
    expect(words).toMatch(/Undo cannot bring it back/);
    expect(words).toMatch(/clears Redo/);
    expect(words).toMatch(/delete, a move or added pages that you made before it loads the document as it was then/);
    expect(words).toMatch(/a turn or a crop never does/);
    expect(t.calls).toEqual([]);
    t.host.answer = "delete";
    await t.annotator.removeSelected();
    expect(t.calls).toEqual(["remove 0 8"]);
    expect(t.history.dirty).toBe(true);
  });

  it("clears what could be made again when an annotation is deleted for good, and keeps what can be taken back", async () => {
    const drawing = annotation({
      id: 8,
      kind: "ink",
      can: { editText: true, recolour: false, move: false, remove: true, restore: false, locked: false },
    });
    const t = setup([drawing]);
    const log: string[] = [];
    t.history.push({ label: "first", undo: () => void log.push("undo first"), redo: () => void log.push("redo first") });
    t.history.push({ label: "second", undo: () => void log.push("undo second"), redo: () => void log.push("redo second") });
    await t.history.undo();
    expect(t.history.canRedo).toBe(true);
    t.annotator.select(0, 8);
    t.host.answer = "delete";
    await t.annotator.removeSelected();
    // Redo has nothing to make again; Undo still takes back the edit before the delete.
    expect(t.history.canRedo).toBe(false);
    expect(await t.history.redo()).toBe(false);
    expect(t.history.canUndo).toBe(true);
    await t.history.undo();
    expect(log).toEqual(["undo second", "undo first"]);
    expect(t.history.canRedo).toBe(true);
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
    await vi.waitFor(() => expect(t.api.updateAnnotation).toHaveBeenCalledTimes(1));
    await new Promise((resolve) => setTimeout(resolve, 20));
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
      can: { editText: true, recolour: false, move: true, remove: true, restore: true, locked: false },
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

describe("changes that are running, undo, and the unsaved state", () => {
  it("undo waits for a change that is running and then takes back that change, not an older one", async () => {
    const t = setup();
    t.history.push({ label: "Old", undo: vi.fn(), redo: vi.fn() });
    let finish: (a: Annotation) => void = () => undefined;
    vi.mocked(t.api.addInk).mockImplementationOnce(
      () =>
        new Promise<Annotation>((resolve) => {
          finish = resolve;
        }),
    );
    t.annotator.onStroke(0, [
      [1, 1],
      [5, 5],
    ]);
    expect(t.annotator.unsaved).toBe(true);
    // Undo is asked for while the drawing is still on its way to the engine.
    const undone = t.annotator.sequenced(() => t.history.undo());
    await vi.waitFor(() => expect(t.api.addInk).toHaveBeenCalled());
    finish(annotation({ id: 500, kind: "ink" }));
    await undone;
    expect(t.calls).toContain("remove 0 500");
    expect(t.history.undoLabel).toBe("Old");
    expect(t.history.redoLabel).toBe("Drawing");
    expect(t.host.failures).toEqual([]);
    expect(t.annotator.unsaved).toBe(false);
  });

  it("a change that is running or waiting counts as unsaved until it is in the history", async () => {
    const t = setup();
    let finish: (a: Annotation) => void = () => undefined;
    vi.mocked(t.api.addInk).mockImplementationOnce(
      () =>
        new Promise<Annotation>((resolve) => {
          finish = resolve;
        }),
    );
    t.annotator.onStroke(0, [[1, 1]]);
    expect(t.annotator.unsaved).toBe(true);
    expect(t.history.dirty).toBe(false);
    const done = t.annotator.flush();
    await vi.waitFor(() => expect(t.api.addInk).toHaveBeenCalled());
    finish(annotation({ id: 501, kind: "ink" }));
    await done;
    expect(t.annotator.unsaved).toBe(false);
    expect(t.history.dirty).toBe(true);
  });

  it("a change that fails in the middle leaves the document marked as changed, a clean refusal does not", async () => {
    const t = setup();
    vi.mocked(t.api.addInk).mockRejectedValueOnce({ code: "annotation_refused", message: "no" });
    await t.annotator.flush();
    t.annotator.onStroke(0, [[1, 1]]);
    await t.annotator.flush();
    expect(t.history.dirty).toBe(false);
    vi.mocked(t.api.addInk).mockRejectedValueOnce({ code: "internal", message: "broke" });
    t.annotator.onStroke(0, [[1, 1]]);
    await t.annotator.flush();
    expect(t.history.dirty).toBe(true);
    expect(t.host.failures).toHaveLength(2);
    // It stays changed until a save names a state.
    t.history.markSaved();
    expect(t.history.dirty).toBe(false);
  });

  it("a document that forbids annotating blocks the tools and says why", async () => {
    const t = setup();
    vi.mocked(t.api.addInk).mockRejectedValueOnce({ code: "annotations_not_allowed", message: "no" });
    t.annotator.onStroke(0, [[1, 1]]);
    await t.annotator.flush();
    expect(t.annotator.blockedReason).toContain("does not allow");
    expect(t.history.dirty).toBe(false);
  });

  it("text typed in the box of a note counts as unsaved, and a flush saves it as the note", async () => {
    const t = setup();
    t.annotator.setTool("note");
    t.annotator.onPlace(0, 100, 100, { x: 0, y: 0 });
    expect(t.annotator.unsaved).toBe(false);
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    area.value = "typed";
    area.dispatchEvent(new Event("input", { bubbles: true }));
    expect(t.annotator.unsaved).toBe(true);
    await t.annotator.flush();
    expect(t.calls).toEqual(["addNote 0 90,90 typed"]);
    expect(t.host.popovers.querySelector("textarea")).toBeNull();
    expect(t.annotator.unsaved).toBe(false);
  });

  it("an open box with no new text is closed by a flush and makes no note", async () => {
    const t = setup();
    t.annotator.setTool("note");
    t.annotator.onPlace(0, 100, 100, { x: 0, y: 0 });
    await t.annotator.flush();
    expect(t.calls).toEqual([]);
    expect(t.host.popovers.querySelector("textarea")).toBeNull();
  });

  it("an open box does not hold up undo", async () => {
    const t = setup();
    t.history.push({ label: "Old", undo: vi.fn(), redo: vi.fn() });
    t.annotator.setTool("note");
    t.annotator.onPlace(0, 100, 100, { x: 0, y: 0 });
    await t.annotator.sequenced(() => t.history.undo());
    expect(t.history.undoLabel).toBeNull();
    // The box is still there, with the person's text to come.
    expect(t.host.popovers.querySelector("textarea")).not.toBeNull();
  });

  it("an undo that failed on the second page can be tried again and does not repeat the first", async () => {
    const t = setup();
    t.setRanges([
      { page: 0, start: 0, end: 4 },
      { page: 1, start: 0, end: 4 },
    ]);
    await t.annotator.markSelection("highlight");
    let calls = 0;
    vi.mocked(t.api.removeAnnotation).mockImplementation((_d, page, id) => {
      calls++;
      t.calls.push(`remove ${page} ${id}`);
      return calls === 2 ? Promise.reject({ code: "internal", message: "broke" }) : Promise.resolve();
    });
    await expect(t.annotator.sequenced(() => t.history.undo())).rejects.toBeTruthy();
    expect(t.history.undoLabel).toBe("Highlight");
    expect(t.history.dirty).toBe(true);
    await t.annotator.sequenced(() => t.history.undo());
    const removals = t.calls.filter((c) => c.startsWith("remove"));
    expect(removals.filter((c) => c === "remove 1 101")).toHaveLength(1);
    expect(removals.filter((c) => c === "remove 0 100")).toHaveLength(2);
    expect(t.history.undoLabel).toBeNull();
  });
});

describe("annotations the file or the document locks", () => {
  const locked = annotation({
    id: 9,
    kind: "stamp",
    can: { editText: false, recolour: false, move: false, remove: false, restore: false, locked: true },
  });

  it("says that a locked annotation is locked when it is picked, and why it cannot be removed", async () => {
    const t = setup([locked]);
    t.annotator.select(0, 9);
    expect(t.host.told.at(-1)).toContain("locked in the file");
    expect(t.annotator.pickedReason).toContain("locked");
    await t.annotator.removeSelected();
    expect(t.host.told.at(-1)).toContain("locked in the file");
    expect(t.calls).toEqual([]);
  });
});

describe("a save", () => {
  it("holds back changes while the file is written, and they run when it lets go", async () => {
    const t = setup();
    const release = t.annotator.hold();
    t.annotator.onStroke(0, [[1, 1]]);
    await Promise.resolve();
    await Promise.resolve();
    expect(t.calls).toEqual([]);
    expect(t.annotator.unsaved).toBe(true);
    release();
    await t.annotator.flush();
    expect(t.calls).toEqual(["addInk 0 3 1"]);
    expect(t.annotator.unsaved).toBe(false);
  });
});

describe("the bar is closed", () => {
  const key = (k: string) => new KeyboardEvent("keydown", { key: k });
  const noteBox = () =>
    annotation({
      id: 5,
      kind: "note",
      text: "x",
      can: { editText: true, recolour: false, move: true, remove: true, restore: true, locked: false },
    });

  it("lets go of what is picked and takes none of the keys", async () => {
    const t = setup([noteBox()]);
    t.annotator.select(0, 5);
    expect(t.annotator.selected).not.toBeNull();
    t.annotator.setReading(true);
    expect(t.annotator.selected).toBeNull();
    for (const k of ["ArrowDown", "ArrowRight", "Delete", "Backspace", "Enter", "Escape"]) {
      expect(t.annotator.handleKey(key(k))).toBe(false);
    }
    // Nothing can be picked behind a closed bar either.
    t.annotator.select(0, 5);
    expect(t.annotator.selected).toBeNull();
    await t.annotator.idle();
    expect(t.calls).toEqual([]);
    expect(t.history.dirty).toBe(false);
    // Opened again, the keys work on what is picked then.
    t.annotator.setReading(false);
    t.annotator.select(0, 5);
    expect(t.annotator.handleKey(key("ArrowDown"))).toBe(true);
  });

  it("does not pick the note that a box places after the bar was closed", async () => {
    const t = setup();
    t.annotator.setTool("note");
    t.annotator.onPlace(0, 100, 100, { x: 0, y: 0 });
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    area.value = "late";
    t.annotator.setReading(true);
    await t.annotator.flush();
    expect(t.calls).toEqual(["addNote 0 90,90 late"]);
    expect(t.annotator.selected).toBeNull();
    expect(t.annotator.tool).toBe("select");
  });
});

describe("what the engine refuses or holds back", () => {
  const movable = () =>
    annotation({
      id: 5,
      kind: "note",
      can: { editText: true, recolour: false, move: true, remove: true, restore: true, locked: false },
    });

  it("undoes a move by the distance the engine really moved the note", async () => {
    const note = movable();
    const t = setup([note]);
    t.annotator.select(0, 5);
    // The engine keeps the note on its page: asked for 12, it moves 2.
    vi.mocked(t.api.updateAnnotation).mockImplementationOnce((_d, page, id, change) => {
      t.calls.push(`update ${page} ${id} ${JSON.stringify(change)}`);
      const next = annotation({ ...note, rect: [12, 20, 32, 40] });
      t.list.set(`${page}:${id}`, next);
      return Promise.resolve(next);
    });
    await t.annotator.moveBy(0, 5, 12, 0);
    await t.history.undo();
    expect(t.calls[1]).toBe('update 0 5 {"dx":-2,"dy":0}');
    await t.history.redo();
    expect(t.calls[2]).toBe('update 0 5 {"dx":2,"dy":0}');
  });

  it("makes no edit for a move that did not move anything", async () => {
    const note = movable();
    const t = setup([note]);
    vi.mocked(t.api.updateAnnotation).mockImplementationOnce(() => Promise.resolve(annotation({ ...note })));
    await t.annotator.moveBy(0, 5, 12, 0);
    expect(t.history.canUndo).toBe(false);
    expect(t.history.dirty).toBe(false);
  });

  it("opens the box again with the typed text when the engine refuses the note", async () => {
    const t = setup();
    t.annotator.setTool("note");
    t.annotator.onPlace(0, 100, 100, { x: 0, y: 0 });
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    area.value = "precious words";
    vi.mocked(t.api.addNote).mockRejectedValueOnce({ code: "annotations_not_allowed", message: "no" });
    (t.host.popovers.querySelector("form") as HTMLFormElement).dispatchEvent(new Event("submit", { cancelable: true }));
    await vi.waitFor(() => expect(t.host.failures).toHaveLength(1));
    await vi.waitFor(() => expect(t.host.popovers.querySelector("textarea")).not.toBeNull());
    const again = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    expect(again.value).toBe("precious words");
    // The text in the box still counts as unsaved.
    expect(t.annotator.unsaved).toBe(true);
  });

  it("opens the box of a comment again, with the draft, when the engine refuses the change", async () => {
    const t = setup([annotation({ id: 3, text: "old" })]);
    const done = t.annotator.editText(0, 3);
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    area.value = "new words";
    vi.mocked(t.api.updateAnnotation).mockRejectedValueOnce({ code: "annotation_refused", message: "no" });
    (t.host.popovers.querySelector("form") as HTMLFormElement).dispatchEvent(new Event("submit", { cancelable: true }));
    await done;
    await vi.waitFor(() => expect(t.host.popovers.querySelector("textarea")).not.toBeNull());
    expect((t.host.popovers.querySelector("textarea") as HTMLTextAreaElement).value).toBe("new words");
    expect(t.annotator.unsaved).toBe(true);
  });

  it("does not open the box again after a failure that is not a refusal", async () => {
    const t = setup();
    t.annotator.setTool("note");
    t.annotator.onPlace(0, 100, 100, { x: 0, y: 0 });
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    area.value = "text";
    vi.mocked(t.api.addNote).mockRejectedValueOnce({ code: "engine_stopped", message: "gone" });
    (t.host.popovers.querySelector("form") as HTMLFormElement).dispatchEvent(new Event("submit", { cancelable: true }));
    await vi.waitFor(() => expect(t.host.failures).toHaveLength(1));
    await t.annotator.idle();
    expect(t.host.popovers.querySelector("textarea")).toBeNull();
  });

  it("says that some pages are marked when a later page is refused", async () => {
    const t = setup();
    t.setRanges([
      { page: 0, start: 0, end: 4 },
      { page: 1, start: 0, end: 4 },
    ]);
    vi.mocked(t.api.addMarkup).mockImplementationOnce(() => Promise.resolve(annotation({ id: 50 })));
    vi.mocked(t.api.addMarkup).mockRejectedValueOnce({ code: "annotation_refused", message: "no" });
    await t.annotator.markSelection("strikeout");
    expect((t.host.failures[0] as { code: string }).code).toBe("annotation_partial");
  });

  it("deleting what came with the file leaves earlier changes undoable and the document changed", async () => {
    const drawing = annotation({
      id: 8,
      kind: "ink",
      can: { editText: true, recolour: false, move: false, remove: true, restore: false, locked: false },
    });
    const t = setup([drawing]);
    const undone = vi.fn();
    t.history.push({ label: "Earlier", undo: undone, redo: vi.fn() });
    t.history.markSaved();
    expect(t.history.dirty).toBe(false);
    t.annotator.select(0, 8);
    await t.annotator.removeSelected();
    expect(t.calls).toEqual(["remove 0 8"]);
    expect(t.history.dirty).toBe(true);
    expect(t.history.undoLabel).toBe("Earlier");
    await t.history.undo();
    expect(undone).toHaveBeenCalled();
    expect(t.history.dirty).toBe(true);
  });

  it("the warning before such a delete does not promise redaction, for any kind of annotation", async () => {
    const asked: string[] = [];
    for (const kind of ["highlight", "other"] as const) {
      const a = annotation({
        id: 4,
        kind,
        can: { editText: false, recolour: false, move: false, remove: true, restore: false, locked: false },
      });
      const t = setup([a]);
      t.host.ask = (options) => {
        asked.push(options.detail ?? "");
        return Promise.resolve({ button: "cancel" });
      };
      t.annotator.select(0, 4);
      await t.annotator.removeSelected();
    }
    expect(asked[0]).toContain("not redaction");
    expect(asked[0]).not.toContain("gone from the file");
    expect(asked[1]).toContain("not redaction");
    expect(asked[1]).not.toContain("gone from the file");
    expect(asked[1]).not.toContain("words of a text box");
  });

  it("a flush also waits for work that was asked for while it waited", async () => {
    const t = setup();
    let finish: (a: Annotation) => void = () => undefined;
    vi.mocked(t.api.addInk).mockImplementationOnce(
      () =>
        new Promise<Annotation>((resolve) => {
          finish = resolve;
        }),
    );
    t.annotator.onStroke(0, [[1, 1]]);
    const done = t.annotator.flush();
    await vi.waitFor(() => expect(t.api.addInk).toHaveBeenCalled());
    // A second change arrives while the flush waits.
    t.annotator.onStroke(0, [[2, 2]]);
    finish(annotation({ id: 600, kind: "ink" }));
    await done;
    expect(t.api.addInk).toHaveBeenCalledTimes(2);
    expect(t.annotator.unsaved).toBe(false);
  });

  it("says why the colour of a highlight cannot be changed when its picture has no plain colour", () => {
    const stuck = annotation({
      id: 6,
      can: { editText: true, recolour: false, move: false, remove: true, restore: false, locked: false },
    });
    const t = setup([stuck]);
    t.annotator.select(0, 6);
    expect(t.annotator.pickedReason).toContain("colour");
    t.annotator.setColor([126, 217, 87, 255]);
    expect(t.host.told.at(-1)).toContain("colour");
    expect(t.calls).toEqual([]);
  });
});

describe("moves that follow each other, and notes cut at the page edge", () => {
  const key = (k: string, extra: KeyboardEventInit = {}) => new KeyboardEvent("keydown", { key: k, ...extra });
  const movable = (rect: Annotation["rect"]) =>
    annotation({
      id: 5,
      kind: "note",
      rect,
      can: { editText: true, recolour: false, move: true, remove: true, restore: true, locked: false },
    });

  /**
   * An engine that holds the true box of the note, answers with the box cut to the page, and moves
   * a note no further out of the page than it was. The list of the layer is NOT brought up to date,
   * as in the window, where it comes some time after the change.
   */
  function lateList(shown: Annotation["rect"], truth: Annotation["rect"]) {
    const t = setup([movable(shown)]);
    const box = { l: truth[0], t: truth[1], r: truth[2], b: truth[3] };
    const floor = Math.min(0, box.l);
    vi.mocked(t.api.updateAnnotation).mockImplementation((_d, page, id, change) => {
      t.calls.push(`update ${page} ${id} ${JSON.stringify(change)}`);
      const dx = Math.min(Math.max(change.dx ?? 0, floor - box.l), 612 - box.r);
      box.l += dx;
      box.r += dx;
      return Promise.resolve(movable([Math.max(box.l, 0), box.t, Math.min(box.r, 612), box.b]));
    });
    return { t, box };
  }

  it("undoes two quick arrow presses by 4 each, so the note ends where it began", async () => {
    const { t, box } = lateList([10, 20, 30, 40], [10, 20, 30, 40]);
    t.annotator.select(0, 5);
    // Both presses come before the layer has heard of the first move.
    expect(t.annotator.handleKey(key("ArrowRight"))).toBe(true);
    expect(t.annotator.handleKey(key("ArrowRight"))).toBe(true);
    await t.annotator.idle();
    expect(box.l).toBe(18);
    await t.history.undo();
    await t.history.undo();
    expect(t.calls.slice(2)).toEqual(['update 0 5 {"dx":-4,"dy":0}', 'update 0 5 {"dx":-4,"dy":0}']);
    expect(box.l).toBe(10);
    expect(t.history.dirty).toBe(false);
    await t.history.redo();
    await t.history.redo();
    expect(box.l).toBe(18);
  });

  it("makes an edit for a note cut by the page edge, and undo puts it back to where the file has it", async () => {
    // The file has the note 10 points outside the page; the list shows it cut at 0.
    const { t, box } = lateList([0, 20, 10, 40], [-10, 20, 10, 40]);
    t.annotator.select(0, 5);
    await t.annotator.moveBy(0, 5, 4, 0);
    expect(box.l).toBe(-6);
    // The left edge did not move in the answer; the right edge did. The move counts.
    expect(t.history.canUndo).toBe(true);
    expect(t.history.dirty).toBe(true);
    await t.annotator.moveBy(0, 5, 24, 0);
    expect(box.l).toBe(18);
    await t.history.undo();
    await t.history.undo();
    expect(t.calls.slice(2)).toEqual(['update 0 5 {"dx":-24,"dy":0}', 'update 0 5 {"dx":-4,"dy":0}']);
    expect(box.l).toBe(-10);
    expect(t.history.dirty).toBe(false);
  });

  it("measures the move by the edge that changed most", () => {
    // Cut at the left: only the right edge tells.
    expect(movedDistance([0, 20, 10, 40], [0, 20, 14, 40], 4, 0)).toEqual([4, 0]);
    // Stopped by the right edge of the page: asked for 12, moved 2.
    expect(movedDistance([590, 20, 610, 40], [592, 20, 612, 40], 12, 0)).toEqual([2, 0]);
    // Within rounding of what was asked: exactly what was asked.
    expect(movedDistance([10, 20, 30, 40], [14.0625, 20, 34.0625, 40], 4, 0)).toEqual([4, 0]);
    // Nothing moved.
    expect(movedDistance([10, 20, 30, 40], [10, 20, 30, 40], 4, 0)).toEqual([0, 0]);
    // No number to go by: what was asked.
    expect(movedDistance([10, 20, 30, 40], [Number.NaN, 20, Number.NaN, 40], 4, 0)).toEqual([4, 0]);
  });
});

describe("edits of an annotation that was deleted for good", () => {
  it("undo skips the earlier edits of a file's annotation that was deleted, and goes on to the ones below", async () => {
    const file = annotation({
      id: 7,
      kind: "highlight",
      text: "old",
      can: { editText: true, recolour: true, move: false, remove: true, restore: false, locked: false },
    });
    const t = setup([file]);
    const below = vi.fn();
    t.history.push({ label: "Earlier", undo: below, redo: vi.fn() });
    t.history.markSaved();
    // An edit of the comment of the annotation, then its deletion.
    const done = t.annotator.editText(0, 7);
    const area = t.host.popovers.querySelector("textarea") as HTMLTextAreaElement;
    area.value = "new";
    (t.host.popovers.querySelector("form") as HTMLFormElement).dispatchEvent(new Event("submit", { cancelable: true }));
    await done;
    await t.annotator.idle();
    t.annotator.select(0, 7);
    await t.annotator.removeSelected();
    expect(t.calls).toEqual(['update 0 7 {"text":"new"}', "remove 0 7"]);
    // Undo reaches the edit of the text, which has nothing left to change, and does not ask the engine.
    expect(t.history.undoLabel).toBe("Edit text");
    await t.history.undo();
    expect(t.calls).toHaveLength(2);
    // The edit below it can be undone, and the document stays marked as changed.
    expect(t.history.undoLabel).toBe("Earlier");
    await t.history.undo();
    expect(below).toHaveBeenCalled();
    expect(t.history.dirty).toBe(true);
    // Redo skips it the same way.
    await t.history.redo();
    await t.history.redo();
    expect(t.calls).toHaveLength(2);
  });

  it("an edit of another annotation is still undone through the engine", async () => {
    const other = annotation({
      id: 8,
      kind: "highlight",
      can: { editText: true, recolour: true, move: false, remove: true, restore: false, locked: false },
    });
    const t = setup([other]);
    t.annotator.select(0, 8);
    t.annotator.setColor([126, 217, 87, 255]);
    await t.annotator.idle();
    await t.history.undo();
    expect(t.calls.at(-1)).toContain('"color":[255,235,59,255]');
  });
});

describe("markup that is done on some pages only", () => {
  it("leaves the marks that were made, as one edit that Undo takes back, and says so", async () => {
    const t = setup();
    t.setRanges([
      { page: 0, start: 0, end: 4 },
      { page: 1, start: 0, end: 4 },
    ]);
    vi.mocked(t.api.addMarkup).mockImplementationOnce(() => Promise.resolve(annotation({ id: 50 })));
    vi.mocked(t.api.addMarkup).mockRejectedValueOnce({ code: "annotation_refused", message: "no" });
    await t.annotator.markSelection("highlight");
    // The outcome: page 0 is marked, the document counts as changed and Undo takes the mark off.
    expect(t.history.dirty).toBe(true);
    expect(t.history.undoLabel).toBe("Highlight");
    await t.history.undo();
    expect(t.calls.filter((c) => c.startsWith("remove"))).toEqual(["remove 0 50"]);
    // The notice that the person reads says that it was done in part, not the opposite.
    const error = t.host.failures[0] as { code: string; message: string };
    const notice = annotationNotice(error, "book.pdf");
    expect(notice.title).toBe("Only some of the text was marked");
    expect(notice.title).not.toContain("not changed");
    expect(notice.detail).toContain("are marked");
    expect(notice.detail).toContain("Undo takes all of them back");
    expect(notice.detail).not.toContain("as it was before");
    expect(notice.detail).not.toContain("may have been made");
  });

  it("tells a clean refusal as one, and anything else as of unknown effect", () => {
    const clean = annotationNotice({ code: "annotation_refused", message: "x" }, "book.pdf");
    expect(clean.detail).toContain("The document is as it was before this change.");
    const unknown = annotationNotice({ code: "internal", message: "x" }, "book.pdf");
    expect(unknown.title).toBe("The annotation was not changed");
    expect(unknown.detail).toContain("Part of the change may have been made");
  });
});

describe("annotations and changes of a form in one history", () => {
  /** A change of a form, as the form layer records it: it only has to leave a trace. */
  const formEdit = (calls: string[], label: string) => ({
    label,
    undo: () => void calls.push(`undo ${label}`),
    redo: () => void calls.push(`redo ${label}`),
  });

  it("takes both kinds back and makes them again in the order they were made", async () => {
    const t = setup();
    t.history.push(formEdit(t.calls, "Fill in Name"));
    t.annotator.onStroke(0, [[1, 1]]);
    await t.annotator.idle();
    t.history.push(formEdit(t.calls, "Tick Agree"));
    t.annotator.onStroke(1, [[2, 2]]);
    await t.annotator.idle();
    expect(t.calls).toEqual(["addInk 0 3 1", "addInk 1 3 1"]);
    expect(t.history.undoLabel).toBe("Drawing");
    expect(t.history.dirty).toBe(true);

    // Undo, one step at a time: the drawing on page 1, the check box, the drawing on page 0, the
    // form field.
    t.calls.length = 0;
    for (let i = 0; i < 4; i++) await t.history.undo();
    expect(t.calls).toEqual(["remove 1 101", "undo Tick Agree", "remove 0 100", "undo Fill in Name"]);
    expect(t.history.dirty).toBe(false);
    expect(t.history.undoLabel).toBeNull();

    // Redo goes the other way over the same four, and the page of an annotation is drawn again.
    t.calls.length = 0;
    t.log.length = 0;
    for (let i = 0; i < 4; i++) await t.history.redo();
    expect(t.calls).toEqual(["redo Fill in Name", "restore 0 100", "redo Tick Agree", "restore 1 101"]);
    expect(t.log.filter((l) => l.startsWith("invalidate:"))).toEqual(["invalidate:0", "invalidate:1"]);
    expect(t.history.dirty).toBe(true);
    expect(t.history.undoLabel).toBe("Drawing");
  });

  it("counts a change of a form after a save as a change, and Undo of it as back at the save", async () => {
    const t = setup();
    t.annotator.onStroke(0, [[1, 1]]);
    await t.annotator.idle();
    t.history.markSaved(t.history.revision);
    expect(t.history.dirty).toBe(false);
    t.history.push(formEdit(t.calls, "Fill in Name"));
    expect(t.history.dirty).toBe(true);
    await t.history.undo();
    expect(t.history.dirty).toBe(false);
    // The drawing made before the save can still be taken back; then the document differs from
    // the saved file.
    await t.history.undo();
    expect(t.history.dirty).toBe(true);
    await t.history.redo();
    expect(t.history.dirty).toBe(false);
  });

  it("holds an annotation back during a save and lets a form change through, then both are in the history", async () => {
    const t = setup();
    const release = t.annotator.hold();
    t.annotator.onStroke(0, [[1, 1]]);
    t.history.push(formEdit(t.calls, "Fill in Name"));
    await Promise.resolve();
    expect(t.calls).toEqual([]);
    expect(t.history.undoLabel).toBe("Fill in Name");
    release();
    await t.annotator.flush();
    expect(t.calls).toEqual(["addInk 0 3 1"]);
    expect(t.history.undoLabel).toBe("Drawing");
    await t.history.undo();
    await t.history.undo();
    expect(t.calls.slice(1)).toEqual(["remove 0 100", "undo Fill in Name"]);
  });
});

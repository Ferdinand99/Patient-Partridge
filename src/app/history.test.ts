import { describe, expect, it, vi } from "vitest";
import { EditHistory, HISTORY_LIMIT, type Edit } from "./history";

/** An edit that adds `n` to a counter, so the order of undo and redo can be seen in the value. */
function counter() {
  const state = { value: 0, log: [] as string[] };
  const edit = (label: string, n: number): Edit => ({
    label,
    undo: () => {
      state.value -= n;
      state.log.push(`undo ${label}`);
    },
    redo: () => {
      state.value += n;
      state.log.push(`redo ${label}`);
    },
  });
  /** Makes the change and records it, as a feature would. */
  const apply = (history: EditHistory, label: string, n: number) => {
    state.value += n;
    history.push(edit(label, n));
  };
  return { state, apply };
}

describe("EditHistory: undo and redo", () => {
  it("starts empty and clean", () => {
    const h = new EditHistory();
    expect(h.canUndo).toBe(false);
    expect(h.canRedo).toBe(false);
    expect(h.dirty).toBe(false);
    expect(h.undoLabel).toBeNull();
    expect(h.redoLabel).toBeNull();
  });

  it("takes edits back in the opposite order and makes them again in the original order", async () => {
    const h = new EditHistory();
    const { state, apply } = counter();
    apply(h, "one", 1);
    apply(h, "two", 10);
    apply(h, "three", 100);
    expect(state.value).toBe(111);
    expect(h.undoLabel).toBe("three");

    expect(await h.undo()).toBe(true);
    expect(await h.undo()).toBe(true);
    expect(state.value).toBe(1);
    expect(h.undoLabel).toBe("one");
    expect(h.redoLabel).toBe("two");

    expect(await h.redo()).toBe(true);
    expect(state.value).toBe(11);
    expect(state.log).toEqual(["undo three", "undo two", "redo two"]);
  });

  it("does nothing, and says so, when there is nothing to undo or redo", async () => {
    const h = new EditHistory();
    const { apply } = counter();
    expect(await h.undo()).toBe(false);
    expect(await h.redo()).toBe(false);
    apply(h, "one", 1);
    expect(await h.redo()).toBe(false);
    expect(await h.undo()).toBe(true);
    expect(await h.undo()).toBe(false);
  });

  it("forgets what could be redone when a new edit is made", async () => {
    const h = new EditHistory();
    const { state, apply } = counter();
    apply(h, "one", 1);
    apply(h, "two", 10);
    await h.undo();
    expect(h.canRedo).toBe(true);
    apply(h, "other", 1000);
    expect(h.canRedo).toBe(false);
    expect(await h.redo()).toBe(false);
    await h.undo();
    await h.undo();
    expect(state.value).toBe(0);
  });

  it("keeps the position where it was when an undo fails, and passes the error on", async () => {
    const h = new EditHistory();
    h.push({ label: "a", undo: () => undefined, redo: () => undefined });
    h.push({
      label: "b",
      undo: () => Promise.reject(new Error("the engine said no")),
      redo: () => undefined,
    });
    await expect(h.undo()).rejects.toThrow("the engine said no");
    expect(h.undoLabel).toBe("b");
    // The history is not stuck busy: the other edit can still be reached once this one is fixed.
    expect(h.canUndo).toBe(true);
  });

  it("does not start a second undo while one is running, and refuses a push in the middle of it", async () => {
    const h = new EditHistory();
    let release!: () => void;
    const slow = new Promise<void>((resolve) => (release = resolve));
    h.push({ label: "a", undo: () => undefined, redo: () => undefined });
    h.push({ label: "b", undo: () => slow, redo: () => undefined });
    const first = h.undo();
    expect(h.canUndo).toBe(false);
    expect(await h.undo()).toBe(false);
    expect(await h.redo()).toBe(false);
    expect(() => h.push({ label: "c", undo: () => undefined, redo: () => undefined })).toThrow(
      "still running",
    );
    release();
    expect(await first).toBe(true);
    expect(h.undoLabel).toBe("a");
    expect(h.canUndo).toBe(true);
  });

  it("forgets the oldest edits past the limit", async () => {
    const h = new EditHistory(3);
    const { state, apply } = counter();
    for (let i = 1; i <= 5; i++) apply(h, `e${i}`, 1);
    expect(state.value).toBe(5);
    let undone = 0;
    while (await h.undo()) undone++;
    expect(undone).toBe(3);
    expect(state.value).toBe(2);
    expect(HISTORY_LIMIT).toBeGreaterThanOrEqual(100);
  });
});

describe("EditHistory: unsaved changes", () => {
  it("is dirty after an edit, clean after a save, and dirty again after the next edit", () => {
    const h = new EditHistory();
    const { apply } = counter();
    apply(h, "one", 1);
    expect(h.dirty).toBe(true);
    h.markSaved();
    expect(h.dirty).toBe(false);
    apply(h, "two", 1);
    expect(h.dirty).toBe(true);
  });

  it("is clean again when the edits since the save are all taken back, and dirty when more is taken back", async () => {
    const h = new EditHistory();
    const { apply } = counter();
    apply(h, "one", 1);
    h.markSaved();
    apply(h, "two", 1);
    expect(h.dirty).toBe(true);
    await h.undo();
    expect(h.dirty).toBe(false);
    await h.undo();
    expect(h.dirty).toBe(true);
    await h.redo();
    expect(h.dirty).toBe(false);
  });

  it("stays dirty for good when the saved state can no longer be reached", async () => {
    const h = new EditHistory();
    const { apply } = counter();
    apply(h, "one", 1);
    apply(h, "two", 1);
    h.markSaved();
    await h.undo();
    // A new edit replaces "two", which was the saved state.
    apply(h, "three", 1);
    expect(h.dirty).toBe(true);
    await h.undo();
    expect(h.dirty).toBe(true);
    await h.redo();
    expect(h.dirty).toBe(true);
    // Only a new save makes it clean.
    h.markSaved();
    expect(h.dirty).toBe(false);
  });

  it("stays dirty when the saved state falls off the end of the history", () => {
    const h = new EditHistory(2);
    const { apply } = counter();
    h.markSaved(); // position 0: the file as it was opened
    apply(h, "one", 1);
    apply(h, "two", 1);
    expect(h.dirty).toBe(true);
    apply(h, "three", 1);
    // The opened state is three edits back, and only two are kept.
    expect(h.dirty).toBe(true);
  });

  it("counts an edit made while the save was running as unsaved", () => {
    const h = new EditHistory();
    const { apply } = counter();
    apply(h, "one", 1);
    const began = h.revision;
    apply(h, "two", 1); // made while the file was being written
    h.markSaved(began);
    expect(h.dirty).toBe(true);
    h.markSaved();
    expect(h.dirty).toBe(false);
  });

  it("is clean after a reset, and has nothing to undo", () => {
    const h = new EditHistory();
    const { apply } = counter();
    apply(h, "one", 1);
    h.reset();
    expect(h.dirty).toBe(false);
    expect(h.canUndo).toBe(false);
  });

  it("tells a listener about every change, until it stops listening", async () => {
    const h = new EditHistory();
    const { apply } = counter();
    const seen = vi.fn();
    const stop = h.onChange(seen);
    apply(h, "one", 1);
    await h.undo();
    await h.redo();
    h.markSaved();
    expect(seen).toHaveBeenCalledTimes(4);
    stop();
    apply(h, "two", 1);
    expect(seen).toHaveBeenCalledTimes(4);
  });
});

describe("EditHistory: the state a save wrote", () => {
  it("a save marks only the state it wrote, also when the oldest edits are forgotten meanwhile", () => {
    const h = new EditHistory(3);
    const { apply } = counter();
    apply(h, "one", 1);
    apply(h, "two", 1);
    apply(h, "three", 1);
    const began = h.revision;
    // An edit is made while the file is written; the history is full, so the oldest edit goes
    // and the number of edits in effect stays the same.
    apply(h, "four", 1);
    h.markSaved(began);
    expect(h.dirty).toBe(true);
    h.markSaved(h.revision);
    expect(h.dirty).toBe(false);
  });

  it("gives a state its number again when an undo returns to it", async () => {
    const h = new EditHistory();
    const { apply } = counter();
    apply(h, "one", 1);
    const saved = h.revision;
    apply(h, "two", 1);
    expect(h.revision).not.toBe(saved);
    await h.undo();
    expect(h.revision).toBe(saved);
  });
});

describe("EditHistory: dropRedo", () => {
  it("forgets what could be made again and keeps what can be taken back", async () => {
    const h = new EditHistory();
    const { state, apply } = counter();
    apply(h, "one", 1);
    apply(h, "two", 10);
    await h.undo();
    expect(h.canRedo).toBe(true);
    const listener = vi.fn();
    h.onChange(listener);
    h.dropRedo();
    expect(listener).toHaveBeenCalledTimes(1);
    expect(h.canRedo).toBe(false);
    expect(h.redoLabel).toBeNull();
    expect(await h.redo()).toBe(false);
    expect(state.value).toBe(1);
    expect(h.canUndo).toBe(true);
    expect(h.undoLabel).toBe("one");
    // The next edit goes on from where the document is.
    apply(h, "three", 100);
    expect(h.undoLabel).toBe("three");
    expect(h.canRedo).toBe(false);
  });

  it("does nothing, and says nothing, when there is nothing to make again", () => {
    const h = new EditHistory();
    const { apply } = counter();
    apply(h, "one", 1);
    const listener = vi.fn();
    h.onChange(listener);
    h.dropRedo();
    expect(listener).not.toHaveBeenCalled();
    expect(h.canUndo).toBe(true);
  });

  it("leaves the document counting as changed when the saved file was a state that could be made again", async () => {
    const h = new EditHistory();
    const { apply } = counter();
    apply(h, "one", 1);
    apply(h, "two", 10);
    h.markSaved();
    await h.undo();
    expect(h.dirty).toBe(true);
    h.dropRedo();
    expect(h.dirty).toBe(true);
    await h.undo();
    expect(h.dirty).toBe(true);
  });

  it("is refused while an undo is running", async () => {
    const h = new EditHistory();
    let release!: () => void;
    h.push({ label: "slow", undo: () => new Promise<void>((r) => (release = r)), redo: () => undefined });
    const running = h.undo();
    expect(() => h.dropRedo()).toThrow(/still running/);
    release();
    await running;
    expect(h.canRedo).toBe(true);
  });
});

// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PageToolsApi, PagesChanged } from "../lib/pagesapi";
import { EditHistory } from "./history";
import type { NoticeOptions, NoticeResult } from "./notice";
import { PageOps, type PageHost, type PageTarget } from "./pageops";

const letter = { width: 612, height: 792 };
const sizes = (n: number) => Array.from({ length: n }, () => letter);
const changed = (pages: number, snapshot = 0, count = 1, formFields = 0): PagesChanged => ({
  pages: sizes(pages),
  snapshot,
  count,
  formFields,
});

function fakeApi() {
  const api = {
    pagesDelete: vi.fn((_d: number, pages: number[]) => Promise.resolve(changed(10 - pages.length, 7, pages.length))),
    pagesReorder: vi.fn((_d: number, _o: number[]) => Promise.resolve(changed(10, 8, 1))),
    pagesRotate: vi.fn((_d: number, pages: number[], _t: number) => Promise.resolve(changed(10, 0, pages.length))),
    pagesCrop: vi.fn((_d: number, _p: number[], _m: [number, number, number, number]) =>
      Promise.resolve({
        pages: sizes(10),
        before: [[0, 0, 612, 792]] as [number, number, number, number][],
        after: [[10, 10, 600, 780]] as [number, number, number, number][],
      }),
    ),
    pagesSetBoxes: vi.fn((_d: number, _p: number[], _b: unknown[]) => Promise.resolve(changed(10, 0, 1))),
    pagesInsertBlank: vi.fn((_d: number, _at: number, _w: number, _h: number) => Promise.resolve(changed(11, 9, 1))),
    pagesRestore: vi.fn((_d: number, snapshot: number) => Promise.resolve(changed(snapshot === 7 ? 10 : 8, snapshot + 100, 0))),
    pagesInsertPdf: vi.fn(
      (_d: number, _at: number, _path: string, _o: object, onProgress: (d: number, t: number) => void) => {
        onProgress(50, 100);
        return Promise.resolve({ changed: changed(13, 11, 3), inserted: 3 });
      },
    ),
    pagesExtract: vi.fn(
      (_d: number, _p: number[], _o: object, _on: (d: number, t: number) => void) =>
        Promise.resolve({ path: "/x/part.pdf", name: "part.pdf" }) as Promise<{ path: string; name: string } | null>,
    ),
    pagesCancel: vi.fn((_d: number) => Promise.resolve("handover" as const)),
  };
  return api;
}

function setup(over: { target?: Partial<PageTarget> | null; picked?: string[]; busyDelayMs?: number } = {}) {
  const api = fakeApi();
  const layer = document.createElement("div");
  layer.hidden = true;
  document.body.append(layer);
  const history = new EditHistory();
  const target: PageTarget | null =
    over.target === null ? null : { doc: 5, name: "book.pdf", history, pages: sizes(10), ...over.target };
  const answers: NoticeResult[] = [];
  const notices: NoticeOptions[] = [];
  const applied: { doc: number; pages: number; show?: number }[] = [];
  const announced: string[] = [];
  const lost: { doc: number; code: string }[] = [];
  const host: PageHost = {
    target: () => target,
    applyPages: (doc, pages, show) => applied.push({ doc, pages: pages.length, show }),
    notice: (options) => {
      notices.push(options);
      return Promise.resolve(answers.shift() ?? { button: options.buttons.find((b) => b.primary)?.id ?? "ok" });
    },
    lost: (doc, error) => {
      lost.push({ doc, code: error.code });
      return Promise.resolve();
    },
    pickFiles: () => Promise.resolve(over.picked ?? ["/in/a.pdf"]),
    announce: (text) => announced.push(text),
    layer,
  };
  const busy: boolean[] = [];
  const ops = new PageOps(api as unknown as PageToolsApi, host, {
    busyChanged: (b) => busy.push(b),
    busyDelayMs: over.busyDelayMs ?? 400,
  });
  return { api, ops, history, layer, answers, notices, applied, announced, lost, busy };
}

beforeEach(() => {
  document.body.replaceChildren();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("deleting pages", () => {
  it("asks the engine once, with the pages in order, and tells the view", async () => {
    const t = setup();
    const result = await t.ops.remove([4, 1, 1]);
    expect(t.api.pagesDelete).toHaveBeenCalledOnce();
    expect(t.api.pagesDelete).toHaveBeenCalledWith(5, [1, 4]);
    expect(t.applied).toEqual([{ doc: 5, pages: 8, show: 1 }]);
    expect(result).toEqual({ select: [1], show: 1 });
    expect(t.history.dirty).toBe(true);
    expect(t.history.undoLabel).toBe("Delete 2 pages");
    expect(t.announced.join(" ")).toMatch(/Deleted 2, 5/);
  });

  it("takes the change back by going to the copy and forward again by the number it gave", async () => {
    const t = setup();
    await t.ops.remove([2]);
    await t.history.undo();
    expect(t.api.pagesRestore).toHaveBeenLastCalledWith(5, 7);
    expect(t.applied.at(-1)).toEqual({ doc: 5, pages: 10, show: undefined });
    await t.history.redo();
    // The engine answered 107 for the state it left: that is the number to redo with.
    expect(t.api.pagesRestore).toHaveBeenLastCalledWith(5, 107);
    await t.history.undo();
    expect(t.api.pagesRestore).toHaveBeenLastCalledWith(5, 207);
    expect(t.history.canRedo).toBe(true);
  });

  it("will not delete every page, and says so", async () => {
    const t = setup({ target: { pages: sizes(3) } });
    expect(await t.ops.remove([0, 1, 2])).toBeNull();
    expect(t.api.pagesDelete).not.toHaveBeenCalled();
    expect(t.notices[0].title).toMatch(/at least one page/);
    expect(t.history.dirty).toBe(false);
  });

  it("says when form fields were on the deleted pages", async () => {
    const t = setup();
    t.api.pagesDelete.mockResolvedValueOnce(changed(8, 7, 2, 3));
    await t.ops.remove([1, 2]);
    await vi.waitFor(() => expect(t.notices.some((n) => n.title.includes("Form fields"))).toBe(true));
    expect(t.notices.find((n) => n.title.includes("Form fields"))?.detail).toMatch(/names? .*stay|stay in the file/i);
  });

  it("says when a change cannot be taken back, and the undo then fails the way it says", async () => {
    const t = setup();
    t.api.pagesDelete.mockResolvedValueOnce(changed(9, 0, 1));
    await t.ops.remove([1]);
    await vi.waitFor(() => expect(t.notices.some((n) => n.title.includes("cannot be taken back"))).toBe(true));
    await expect(t.history.undo()).rejects.toMatchObject({ code: "undo_unavailable" });
    // The history stays where it was: the document is still changed.
    expect(t.history.dirty).toBe(true);
    expect(t.api.pagesRestore).not.toHaveBeenCalled();
  });
});

describe("turning, cropping, moving and adding pages", () => {
  it("a turn is taken back with the opposite turn on the same pages", async () => {
    const t = setup();
    await t.ops.rotate([3, 1], 1);
    expect(t.api.pagesRotate).toHaveBeenLastCalledWith(5, [1, 3], 1);
    await t.history.undo();
    expect(t.api.pagesRotate).toHaveBeenLastCalledWith(5, [1, 3], 3);
    await t.history.redo();
    expect(t.api.pagesRotate).toHaveBeenLastCalledWith(5, [1, 3], 1);
    await t.ops.rotate([0], 3);
    await t.history.undo();
    expect(t.api.pagesRotate).toHaveBeenLastCalledWith(5, [0], 1);
    // A turn needs no copy of the document.
    expect(t.api.pagesRestore).not.toHaveBeenCalled();
  });

  it("a crop is taken back with the boxes the engine gave and made again with the new ones", async () => {
    const t = setup();
    await t.ops.crop([2], [10, 20, 30, 40]);
    expect(t.api.pagesCrop).toHaveBeenCalledWith(5, [2], [10, 20, 30, 40]);
    await t.history.undo();
    expect(t.api.pagesSetBoxes).toHaveBeenLastCalledWith(5, [2], [[0, 0, 612, 792]]);
    await t.history.redo();
    expect(t.api.pagesSetBoxes).toHaveBeenLastCalledWith(5, [2], [[10, 10, 600, 780]]);
    expect(t.announced.join(" ")).toMatch(/hidden, not removed/);
  });

  it("a move records the pages that moved and picks them where they went", async () => {
    const t = setup();
    const result = await t.ops.reorder([3, 0, 1, 2, 4, 5, 6, 7, 8, 9], [3]);
    expect(t.api.pagesReorder).toHaveBeenCalledWith(5, [3, 0, 1, 2, 4, 5, 6, 7, 8, 9]);
    expect(result).toEqual({ select: [0], show: 0 });
    expect(t.history.undoLabel).toBe("Move 1 page");
    await t.history.undo();
    expect(t.api.pagesRestore).toHaveBeenLastCalledWith(5, 8);
  });

  it("a blank page goes where it is asked and is picked", async () => {
    const t = setup();
    const result = await t.ops.insertBlank(4, 300, 400);
    expect(t.api.pagesInsertBlank).toHaveBeenCalledWith(5, 4, 300, 400);
    expect(result).toEqual({ select: [4], show: 4 });
    expect(t.history.undoLabel).toBe("Add a blank page");
  });
});

describe("pages from other files", () => {
  it("inserts each file after the one before, and picks all the new pages", async () => {
    const t = setup({ picked: ["/in/a.pdf", "C:\\in\\b.pdf"] });
    const result = await t.ops.insertFiles(2);
    expect(t.api.pagesInsertPdf.mock.calls.map((c) => [c[1], c[2]])).toEqual([
      [2, "/in/a.pdf"],
      [5, "C:\\in\\b.pdf"],
    ]);
    expect(result).toEqual({ select: [2, 3, 4, 5, 6, 7], show: 2 });
    expect(t.history.undoLabel).toBe("Add the pages of b.pdf");
  });

  it("does nothing when the dialog is cancelled", async () => {
    const t = setup({ picked: [] });
    expect(await t.ops.insertFiles(0)).toBeNull();
    expect(t.api.pagesInsertPdf).not.toHaveBeenCalled();
    expect(t.busy).toEqual([]);
  });

  it("asks for the password, again when it is wrong, and tries with it", async () => {
    const t = setup();
    t.api.pagesInsertPdf
      .mockRejectedValueOnce({ code: "password_required", message: "" })
      .mockRejectedValueOnce({ code: "password_incorrect", message: "" });
    t.answers.push({ button: "open", password: "wrong" }, { button: "open", password: "right" });
    const result = await t.ops.insertFiles(0);
    expect(result?.select).toEqual([0, 1, 2]);
    expect(t.notices[0].password).toBe(true);
    expect(t.notices[1].title).toMatch(/not correct/);
    const passwords = t.api.pagesInsertPdf.mock.calls.map((c) => (c[3] as { password?: string }).password);
    expect(passwords).toEqual([undefined, "wrong", "right"]);
  });

  it("gives up when the password question is cancelled", async () => {
    const t = setup();
    t.api.pagesInsertPdf.mockRejectedValueOnce({ code: "password_required", message: "" });
    t.answers.push({ button: "cancel" });
    expect(await t.ops.insertFiles(0)).toBeNull();
    expect(t.api.pagesInsertPdf).toHaveBeenCalledOnce();
    expect(t.history.dirty).toBe(false);
  });

  it("says that a protected file's pages join without its protection, and does it only on a yes", async () => {
    const t = setup();
    t.api.pagesInsertPdf.mockRejectedValueOnce({ code: "insert_protected", message: "" });
    await t.ops.insertFiles(0);
    expect(t.notices[0].detail).toMatch(/"a.pdf" is protected by a password/);
    expect(t.notices[0].detail).toMatch(/without that protection/);
    expect((t.api.pagesInsertPdf.mock.calls[1][3] as { allowProtected?: boolean }).allowProtected).toBe(true);

    const n = setup();
    n.api.pagesInsertPdf.mockRejectedValueOnce({ code: "insert_protected", message: "" });
    n.answers.push({ button: "cancel" });
    expect(await n.ops.insertFiles(0)).toBeNull();
    expect(n.api.pagesInsertPdf).toHaveBeenCalledOnce();
    expect(n.history.dirty).toBe(false);
  });

  it("says that a signed file's pages come without the signature", async () => {
    const t = setup();
    t.api.pagesInsertPdf.mockRejectedValueOnce({ code: "insert_signed", message: "" });
    await t.ops.insertFiles(0);
    expect(t.notices[0].detail).toMatch(/is signed/);
    expect((t.api.pagesInsertPdf.mock.calls[1][3] as { allowSigned?: boolean }).allowSigned).toBe(true);
  });

  it("stops at the first file that fails and keeps what came before", async () => {
    const t = setup({ picked: ["/in/a.pdf", "/in/b.pdf", "/in/c.pdf"] });
    t.api.pagesInsertPdf
      .mockResolvedValueOnce({ changed: changed(12, 11, 2), inserted: 2 })
      .mockRejectedValueOnce({ code: "import_failed", message: "" });
    const result = await t.ops.insertFiles(0);
    expect(t.api.pagesInsertPdf).toHaveBeenCalledTimes(2);
    expect(result?.select).toEqual([0, 1]);
    expect(t.history.undoLabel).toBe("Add the pages of a.pdf");
    expect(t.notices.at(-1)?.title).toMatch(/could not be added/);
  });
});

describe("copying pages out", () => {
  it("copies the pages in order and says what the new file is", async () => {
    const t = setup();
    const saved = await t.ops.extract([5, 2]);
    expect(saved?.name).toBe("part.pdf");
    expect(t.api.pagesExtract.mock.calls[0][1]).toEqual([2, 5]);
    expect(t.history.dirty).toBe(false);
    await vi.waitFor(() => expect(t.notices.length).toBeGreaterThan(0));
    expect(t.notices[0].detail).toMatch(/no password/);
    expect(t.notices[0].detail).toMatch(/signatures/);
  });

  it("is quiet when the dialog is cancelled", async () => {
    const t = setup();
    t.api.pagesExtract.mockResolvedValueOnce(null);
    expect(await t.ops.extract([1])).toBeNull();
    expect(t.notices).toEqual([]);
  });

  it("asks before the new file loses the password or the signature", async () => {
    const t = setup();
    t.api.pagesExtract
      .mockRejectedValueOnce({ code: "save_unprotected", message: "" })
      .mockRejectedValueOnce({ code: "save_signed", message: "" });
    await t.ops.extract([1]);
    expect(t.notices[0].detail).toMatch(/will not be/);
    expect(t.notices[1].detail).toMatch(/will not have the signature/);
    const last = t.api.pagesExtract.mock.calls[2][2] as { allowSigned: boolean; allowUnprotected: boolean };
    expect(last).toEqual({ allowSigned: true, allowUnprotected: true });

    const n = setup();
    n.api.pagesExtract.mockRejectedValueOnce({ code: "save_unprotected", message: "" });
    n.answers.push({ button: "cancel" });
    expect(await n.ops.extract([1])).toBeNull();
    expect(n.api.pagesExtract).toHaveBeenCalledOnce();
  });

  it("explains a name that would replace the document", async () => {
    const t = setup();
    t.api.pagesExtract.mockRejectedValueOnce({ code: "extract_over_source", message: "" });
    await t.ops.extract([1]);
    expect(t.notices[0].title).toBe("Choose another name");
  });
});

describe("when something goes wrong", () => {
  it("a lost engine ends the tab through the host, and no other message is shown", async () => {
    const t = setup();
    t.api.pagesDelete.mockRejectedValueOnce({ code: "engine_stopped", message: "gone" });
    expect(await t.ops.remove([1])).toBeNull();
    expect(t.lost).toEqual([{ doc: 5, code: "engine_stopped" }]);
    expect(t.notices).toEqual([]);
    expect(t.history.dirty).toBe(false);
    expect(t.busy).toEqual([true, false]);
  });

  it("any other error is told in plain words and changes nothing", async () => {
    const t = setup();
    for (const [code, needle] of [
      ["too_many_pages", /10,000 pages/],
      ["page_change_failed", /refused the change/],
      ["snapshot_gone", /too old|let go of/],
      ["busy", /being saved/],
      ["page_out_of_range", /cannot be changed that way/],
    ] as const) {
      t.api.pagesRotate.mockRejectedValueOnce({ code, message: "" });
      await t.ops.rotate([0], 1);
      expect(t.notices.at(-1)?.detail ?? "").toMatch(needle);
    }
    expect(t.history.dirty).toBe(false);
  });

  it("a stop by the user is quiet", async () => {
    const t = setup();
    t.api.pagesInsertPdf.mockRejectedValueOnce({ code: "cancelled", message: "" });
    expect(await t.ops.insertFiles(0)).toBeNull();
    expect(t.notices).toEqual([]);
    expect(t.announced.join(" ")).toMatch(/as it was/);
  });

  it("a document with more pages than the tools take is refused before anything is asked", async () => {
    const t = setup({ target: { pages: sizes(10001) } });
    expect(await t.ops.rotate([0], 1)).toBeNull();
    expect(t.api.pagesRotate).not.toHaveBeenCalled();
    expect(t.notices[0].title).toMatch(/Too many pages/);
  });

  it("without a document nothing happens", async () => {
    const t = setup({ target: null });
    expect(await t.ops.remove([0])).toBeNull();
    expect(await t.ops.insertFiles(0)).toBeNull();
    expect(t.api.pagesDelete).not.toHaveBeenCalled();
  });
});

describe("working", () => {
  it("only one tool works at a time, and taking a change back waits its turn", async () => {
    const t = setup();
    let finish!: (c: PagesChanged) => void;
    t.api.pagesRotate.mockImplementationOnce(() => new Promise((r) => (finish = r)));
    const first = t.ops.rotate([0], 1);
    await Promise.resolve();
    expect(t.ops.busy).toBe(true);
    // A second tool is not started, and the history does not step.
    expect(await t.ops.rotate([1], 1)).toBeNull();
    expect(t.api.pagesRotate).toHaveBeenCalledOnce();
    let idle = false;
    void t.ops.whenIdle().then(() => (idle = true));
    await Promise.resolve();
    expect(idle).toBe(false);
    finish(changed(10, 0, 1));
    await first;
    await t.ops.whenIdle();
    expect(idle).toBe(true);
    expect(t.ops.busy).toBe(false);
    expect(t.busy).toEqual([true, false]);
    // Going back while a tool works is refused.
    let release!: (c: PagesChanged) => void;
    t.api.pagesRotate.mockImplementationOnce(() => new Promise((r) => (release = r)));
    const second = t.ops.rotate([2], 1);
    await Promise.resolve();
    await expect(t.history.undo()).rejects.toMatchObject({ code: "busy" });
    release(changed(10, 0, 1));
    await second;
  });

  it("shows the box with the bar after a moment, and Stop asks the app to stop", async () => {
    vi.useFakeTimers();
    const t = setup({ busyDelayMs: 300 });
    let finish!: (v: { changed: PagesChanged; inserted: number }) => void;
    let report!: (d: number, t: number) => void;
    t.api.pagesInsertPdf.mockImplementationOnce((_d, _a, _p, _o, onProgress) => {
      report = onProgress;
      return new Promise((r) => (finish = r));
    });
    const work = t.ops.insertFiles(0);
    await vi.advanceTimersByTimeAsync(100);
    expect(t.layer.querySelector(".busy-box")).toBeNull();
    await vi.advanceTimersByTimeAsync(250);
    const box = t.layer.querySelector<HTMLElement>(".busy-box");
    expect(box).not.toBeNull();
    expect(box?.querySelector("h2")?.textContent).toBe("Adding pages");
    report(25, 100);
    expect(box?.querySelector<HTMLProgressElement>("progress")?.value).toBe(25);
    expect(box?.textContent).toMatch(/25 %/);
    box?.querySelector<HTMLButtonElement>("button")?.click();
    await vi.advanceTimersByTimeAsync(0);
    expect(t.api.pagesCancel).toHaveBeenCalledWith(5);
    finish({ changed: changed(13, 11, 3), inserted: 3 });
    await work;
    expect(t.layer.querySelector(".busy-box")).toBeNull();
    expect(t.layer.hidden).toBe(true);
  });

  it("a quick tool never shows the box", async () => {
    vi.useFakeTimers();
    const t = setup();
    const work = t.ops.rotate([0], 1);
    await vi.advanceTimersByTimeAsync(10);
    await work;
    await vi.advanceTimersByTimeAsync(1000);
    expect(t.layer.querySelector(".busy-box")).toBeNull();
  });

  it("the box is out of the way while a question is open", async () => {
    vi.useFakeTimers();
    const t = setup({ busyDelayMs: 50 });
    let ask!: () => void;
    t.api.pagesInsertPdf.mockImplementationOnce(
      () =>
        new Promise((_resolve, reject) => {
          ask = () => reject({ code: "insert_protected", message: "" });
        }),
    );
    let seen = "";
    const original = t.notices.push.bind(t.notices);
    t.notices.push = (n: NoticeOptions) => {
      seen = t.layer.querySelector(".busy-box") === null ? "hidden" : "shown";
      return original(n);
    };
    const work = t.ops.insertFiles(0);
    await vi.advanceTimersByTimeAsync(100);
    expect(t.layer.querySelector(".busy-box")).not.toBeNull();
    ask();
    await vi.advanceTimersByTimeAsync(0);
    await work;
    expect(seen).toBe("hidden");
  });
});

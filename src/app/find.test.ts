// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DocumentView } from "../viewer/viewer";
import { FindBar, findStatus, type FindTarget } from "./find";

beforeEach(() => {
  const html = readFileSync("index.html", "utf8");
  const body = /<body>([\s\S]*)<\/body>/.exec(html)?.[1] ?? "";
  document.body.innerHTML = body.replace(/<script[\s\S]*?<\/script>/g, "");
});

afterEach(() => {
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

const wait = (ms = 0) => new Promise((r) => setTimeout(r, ms));
const q = <T extends HTMLElement>(selector: string) => document.querySelector(selector) as T;

/** A stand-in for a document view: it only records what the find bar asks of it. */
function fakeView(pageCount = 10, current = 0) {
  const view = {
    pageCount,
    currentPageIndex: current,
    element: document.createElement("div"),
    setFind: vi.fn(),
    clearFind: vi.fn(),
    revealMatch: vi.fn(() => Promise.resolve()),
  };
  view.element.tabIndex = 0;
  return view;
}

function make(matches: Record<number, [number, number][]> = { 2: [[0, 3]], 5: [[4, 3], [9, 3]] }) {
  const searchPage = vi.fn((_doc: number, page: number, _query: string) => Promise.resolve(matches[page] ?? []));
  const onEngineLost = vi.fn();
  const bar = new FindBar(document, { searchPage, onEngineLost, debounceMs: 5 });
  const view = fakeView();
  const target: FindTarget = { doc: 3, view: view as unknown as DocumentView };
  bar.setTarget(target);
  return { bar, view, target, searchPage, onEngineLost };
}

async function type(text: string) {
  const input = q<HTMLInputElement>("#find-input");
  input.value = text;
  input.dispatchEvent(new Event("input"));
  await wait(40);
  await wait(80);
}

describe("findStatus", () => {
  const base = { query: "x", running: false, failed: false, total: 0, position: 0, searched: 0, pageCount: 10 };
  it("tells where the search is", () => {
    expect(findStatus({ ...base, query: null })).toBe("");
    expect(findStatus({ ...base, failed: true })).toBe("The search stopped");
    expect(findStatus({ ...base, running: true })).toBe("Searching…");
    expect(findStatus({ ...base, running: true, total: 3, position: 1 })).toBe("1 of 3+");
    expect(findStatus({ ...base, running: true, pageCount: 500, searched: 120 })).toBe("Searching… (page 120 of 500)");
    expect(findStatus({ ...base, total: 0 })).toBe("No matches");
    expect(findStatus({ ...base, total: 7, position: 2 })).toBe("2 of 7");
    expect(findStatus({ ...base, total: 7, position: 0 })).toBe("1 of 7");
  });
});

describe("FindBar", () => {
  it("is closed at start, and the button is off while no document is open", () => {
    new FindBar(document, { searchPage: vi.fn(), onEngineLost: vi.fn() });
    expect(q("#findbar").hidden).toBe(true);
    expect(q<HTMLButtonElement>("#find-button").disabled).toBe(true);
  });

  it("opens with the focus in the box, and closes with Escape and gives the focus back", () => {
    const { bar, view } = make();
    document.body.append(view.element);
    bar.open();
    expect(q("#findbar").hidden).toBe(false);
    expect(document.activeElement).toBe(q("#find-input"));
    expect(q("#find-button").getAttribute("aria-pressed")).toBe("true");
    q("#find-input").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(q("#findbar").hidden).toBe(true);
    expect(document.activeElement).toBe(view.element);
    expect(view.clearFind).toHaveBeenCalled();
  });

  it("searches from the page in view when typing rests, and shows the first match", async () => {
    const { bar, view, searchPage } = make();
    view.currentPageIndex = 4;
    bar.open();
    await type("fox");
    expect(searchPage.mock.calls.map((c) => c[1])).toEqual([4, 5, 6, 7, 8, 9, 0, 1, 2, 3]);
    expect(searchPage.mock.calls.every((c) => c[0] === 3 && c[2] === "fox")).toBe(true);
    // The first match from page 5 on is shown and scrolled to.
    expect(view.revealMatch).toHaveBeenCalledWith({ page: 5, start: 4, len: 3 });
    const [matches, current] = view.setFind.mock.calls.at(-1) as [Map<number, unknown>, unknown];
    expect([...matches.keys()].sort()).toEqual([2, 5]);
    expect(current).toEqual({ page: 5, start: 4, len: 3 });
    // The matches are page 2, then two on page 5: the second one is the first from page 4 on.
    expect(q("#find-status").textContent).toBe("2 of 3");
  });

  it("does not start a search for every key press", async () => {
    const { bar, searchPage } = make();
    bar.open();
    const input = q<HTMLInputElement>("#find-input");
    for (const text of ["f", "fo", "fox"]) {
      input.value = text;
      input.dispatchEvent(new Event("input"));
      await wait(1);
    }
    await wait(120);
    expect(new Set(searchPage.mock.calls.map((c) => c[2]))).toEqual(new Set(["fox"]));
  });

  it("moves to the next and previous match with Enter, Shift+Enter and the buttons, and wraps round", async () => {
    const { bar, view } = make();
    bar.open();
    await type("fox");
    const status = () => q("#find-status").textContent;
    expect(status()).toBe("1 of 3");
    const input = q("#find-input");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    expect(status()).toBe("2 of 3");
    expect(view.revealMatch).toHaveBeenLastCalledWith({ page: 5, start: 4, len: 3 });
    q("#find-next").click();
    expect(status()).toBe("3 of 3");
    q("#find-next").click();
    expect(status()).toBe("1 of 3");
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", shiftKey: true, bubbles: true }));
    expect(status()).toBe("3 of 3");
    q("#find-prev").click();
    expect(status()).toBe("2 of 3");
  });

  it("says when there are no matches", async () => {
    const { bar, view } = make({});
    bar.open();
    await type("zebra");
    expect(q("#find-status").textContent).toBe("No matches");
    expect(view.revealMatch).not.toHaveBeenCalled();
    q("#find-next").click();
    expect(q("#find-status").textContent).toBe("No matches");
  });

  it("clears the highlights when the box is emptied", async () => {
    const { bar, view } = make();
    bar.open();
    await type("fox");
    view.clearFind.mockClear();
    await type("   ");
    expect(view.clearFind).toHaveBeenCalled();
    expect(q("#find-status").textContent).toBe("");
  });

  it("stops the old search when a new one starts", async () => {
    let release: () => void = () => undefined;
    const gate = new Promise<void>((r) => (release = r));
    const searchPage = vi.fn(async (_d: number, page: number, query: string) => {
      if (query === "slow" && page === 1) await gate;
      return query === "fast" && page === 0 ? ([[0, 4]] as [number, number][]) : [];
    });
    const bar = new FindBar(document, { searchPage, onEngineLost: vi.fn(), debounceMs: 5 });
    const view = fakeView(100);
    bar.setTarget({ doc: 1, view: view as unknown as DocumentView });
    bar.open();
    await type("slow");
    await type("fast");
    release();
    await wait(100);
    const slowPages = searchPage.mock.calls.filter((c) => c[2] === "slow").map((c) => c[1]);
    expect(slowPages.length).toBeLessThanOrEqual(2);
    expect(q("#find-status").textContent).not.toBe("");
  });

  it("keeps a separate search for each document", async () => {
    const { bar, view, target } = make();
    bar.open();
    await type("fox");
    expect(q("#find-status").textContent).toBe("1 of 3");
    const other = fakeView();
    bar.setTarget({ doc: 4, view: other as unknown as DocumentView });
    expect(q<HTMLInputElement>("#find-input").value).toBe("");
    expect(q("#find-status").textContent).toBe("");
    bar.setTarget(target);
    expect(q<HTMLInputElement>("#find-input").value).toBe("fox");
    expect(q("#find-status").textContent).toBe("1 of 3");
    bar.forget(view as unknown as DocumentView);
  });

  it("takes the highlights off a document that is shown while the bar is closed", () => {
    const { bar } = make();
    const other = fakeView();
    bar.setTarget({ doc: 4, view: other as unknown as DocumentView });
    expect(other.clearFind).toHaveBeenCalled();
    expect(q("#findbar").hidden).toBe(true);
  });

  it("reports an engine that is gone, once, and stops", async () => {
    const searchPage = vi.fn(async (_d: number, page: number) => {
      if (page === 2) throw { code: "engine_stopped", message: "gone" };
      return [] as [number, number][];
    });
    const onEngineLost = vi.fn();
    const bar = new FindBar(document, { searchPage, onEngineLost, debounceMs: 5 });
    const view = fakeView();
    bar.setTarget({ doc: 1, view: view as unknown as DocumentView });
    bar.open();
    await type("x");
    expect(onEngineLost).toHaveBeenCalledTimes(1);
    expect(onEngineLost.mock.calls[0][1]).toMatchObject({ code: "engine_stopped" });
    expect(q("#find-status").textContent).toBe("The search stopped");
    expect(searchPage).toHaveBeenCalledTimes(3);
  });

  it("goes with no document: the button is off and the bar is shut", () => {
    const { bar } = make();
    bar.open();
    bar.setTarget(null);
    expect(q("#findbar").hidden).toBe(true);
    expect(q<HTMLButtonElement>("#find-button").disabled).toBe(true);
    bar.open();
    expect(q("#findbar").hidden).toBe(true);
  });

  it("opens from F3 when there is no search yet", () => {
    const { bar } = make();
    bar.step(1);
    expect(q("#findbar").hidden).toBe(false);
  });
});

describe("findStatus: limits", () => {
  const base = {
    query: "x",
    running: false,
    failed: false,
    total: 20000,
    position: 3,
    searched: 10,
    pageCount: 5000,
  };
  it("says when the search stopped at the total, and when a page has more matches than are shown", () => {
    expect(findStatus({ ...base, limit: "total" })).toBe("3 of 20,000+ (stopped at the limit)");
    expect(findStatus({ ...base, total: 7, limit: "page" })).toBe("3 of 7 (some pages have more matches than are shown)");
    expect(findStatus({ ...base, total: 7, limit: null })).toBe("3 of 7");
  });
});

describe("FindBar: the limit and shared timers", () => {
  it("tells the user that the search stopped at the limit", async () => {
    const searchPage = vi.fn((_doc: number, _page: number, _query: string) =>
      Promise.resolve(Array.from({ length: 2000 }, (_, i) => [i * 2, 1] as [number, number])),
    );
    const bar = new FindBar(document, { searchPage, onEngineLost: vi.fn(), debounceMs: 5 });
    const view = fakeView(5000);
    bar.setTarget({ doc: 3, view: view as unknown as DocumentView });
    bar.open();
    await type("e");
    await wait(400);
    expect(q("#find-status").textContent).toContain("20,000+");
    expect(q("#find-status").textContent).toContain("stopped at the limit");
    expect(searchPage.mock.calls.length).toBe(10);
  });

  it("does not lose the last update of a search because another document had one waiting", async () => {
    let releaseA: () => void = () => undefined;
    const gateA = new Promise<void>((r) => (releaseA = r));
    const searchPage = vi.fn(async (doc: number, page: number, _query: string) => {
      // Document 1 finds a match on its first page and then waits.
      if (doc === 1 && page === 1) await gateA;
      return page === 0 ? ([[0, 3]] as [number, number][]) : [];
    });
    const bar = new FindBar(document, { searchPage, onEngineLost: vi.fn(), debounceMs: 5 });
    const a = fakeView(3);
    const b = fakeView(1);
    bar.setTarget({ doc: 1, view: a as unknown as DocumentView });
    bar.open();
    const input = q<HTMLInputElement>("#find-input");
    input.value = "fox";
    input.dispatchEvent(new Event("input"));
    await wait(15);
    // The search of document 1 has an update waiting. Now the other tab comes to the front.
    bar.setTarget({ doc: 2, view: b as unknown as DocumentView });
    input.value = "fox";
    input.dispatchEvent(new Event("input"));
    await wait(15);
    await wait(150);
    expect(q("#find-status").textContent).toBe("1 of 1");
    releaseA();
  });
});

describe("FindBar: a new search replaces the update that the old one was waiting to show", () => {
  it("shows the end of a search that finishes within the wait of the one before", async () => {
    const searchPage = vi.fn(async (_doc: number, page: number, query: string) =>
      query === "first" ? (page === 0 ? ([[0, 3]] as [number, number][]) : []) : page === 0 ? ([[1, 2]] as [number, number][]) : [],
    );
    const bar = new FindBar(document, { searchPage, onEngineLost: vi.fn(), debounceMs: 5 });
    const view = fakeView(1);
    bar.setTarget({ doc: 1, view: view as unknown as DocumentView });
    bar.open();
    const input = q<HTMLInputElement>("#find-input");
    input.value = "first";
    input.dispatchEvent(new Event("input"));
    await wait(12);
    // The first search has an update waiting. The reader changes the text at once.
    input.value = "second";
    input.dispatchEvent(new Event("input"));
    await wait(12);
    await wait(150);
    expect(q("#find-status").textContent).toBe("1 of 1");
    const [, current] = view.setFind.mock.calls.at(-1) as [Map<number, number[][]>, unknown];
    expect(current).toEqual({ page: 0, start: 1, len: 2 });
  });
});

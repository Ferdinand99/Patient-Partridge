import { describe, expect, it, vi } from "vitest";
import { MAX_QUERY_CHARS, MatchCursor, SearchSession, cleanQuery, type PageSearch } from "./search";

const wait = () => new Promise((r) => setTimeout(r, 0));

/** A fake engine: the matches on each page, by page number. */
function fake(matches: Record<number, [number, number][]>) {
  const search = vi.fn<PageSearch>((page) => Promise.resolve(matches[page] ?? []));
  return search;
}

function session(search: PageSearch, pageCount: number, startPage = 0, query = "fox") {
  const onUpdate = vi.fn();
  const onError = vi.fn();
  const s = new SearchSession({ query, pageCount, startPage, search, onUpdate, onError });
  return { s, onUpdate, onError };
}

describe("cleanQuery", () => {
  it("trims, squeezes white space, and cuts to the allowed length", () => {
    expect(cleanQuery("  quick   brown\n fox ")).toBe("quick brown fox");
    expect(cleanQuery("   ")).toBeNull();
    expect(cleanQuery("")).toBeNull();
    const long = cleanQuery("x".repeat(500)) as string;
    expect(Array.from(long)).toHaveLength(MAX_QUERY_CHARS);
    // Counted in characters, so the cut never splits an emoji.
    expect(Array.from(cleanQuery("📕".repeat(100)) as string)).toHaveLength(MAX_QUERY_CHARS);
  });
});

describe("SearchSession", () => {
  it("searches every page once and keeps the matches in reading order", async () => {
    const search = fake({ 0: [[4, 3]], 2: [[1, 3], [20, 3]], 4: [[0, 3]] });
    const { s, onUpdate } = session(search, 5);
    await s.run();
    expect(search.mock.calls.map((c) => c[0])).toEqual([0, 1, 2, 3, 4]);
    expect(search.mock.calls.every((c) => c[1] === "fox")).toBe(true);
    expect(s.matches).toEqual([
      { page: 0, start: 4, len: 3 },
      { page: 2, start: 1, len: 3 },
      { page: 2, start: 20, len: 3 },
      { page: 4, start: 0, len: 3 },
    ]);
    expect(s.done).toBe(true);
    expect(s.running).toBe(false);
    expect(s.pagesSearched).toBe(5);
    expect(onUpdate).toHaveBeenCalled();
    expect(s.onPage(2)).toEqual([[1, 3], [20, 3]]);
    expect(s.onPage(1)).toEqual([]);
  });

  it("starts at the page the reader is on, then wraps round to the start", async () => {
    const search = fake({ 1: [[0, 3]], 4: [[0, 3]] });
    const { s } = session(search, 6, 3);
    await s.run();
    expect(search.mock.calls.map((c) => c[0])).toEqual([3, 4, 5, 0, 1, 2]);
    // Still in reading order, whichever page it started on.
    expect(s.matches.map((m) => m.page)).toEqual([1, 4]);
  });

  it("shows what it has found while it is still running", async () => {
    let release: () => void = () => undefined;
    const gate = new Promise<void>((r) => (release = r));
    const search = vi.fn<PageSearch>(async (page) => {
      if (page === 1) await gate;
      return page === 0 ? [[2, 3]] : [];
    });
    const { s } = session(search, 3);
    const run = s.run();
    await wait();
    expect(s.running).toBe(true);
    expect(s.matches).toHaveLength(1);
    expect(s.pagesSearched).toBe(1);
    release();
    await run;
    expect(s.done).toBe(true);
  });

  it("stops when cancelled, keeps what it found, and asks for no more pages", async () => {
    const calls: number[] = [];
    const ref: { s: SearchSession | null } = { s: null };
    const search = vi.fn<PageSearch>(async (page) => {
      calls.push(page);
      if (page === 2) ref.s?.cancel();
      return page === 1 ? [[0, 3]] : [];
    });
    const { s } = session(search, 100);
    ref.s = s;
    await s.run();
    expect(calls).toEqual([0, 1, 2]);
    expect(s.matches).toHaveLength(1);
    expect(s.done).toBe(false);
    expect(s.running).toBe(false);
  });

  it("reports an error once and stops", async () => {
    const search = vi.fn<PageSearch>(async (page) => {
      if (page === 2) throw { code: "engine_stopped", message: "gone" };
      return [];
    });
    const { s, onError } = session(search, 10);
    await s.run();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][0]).toMatchObject({ code: "engine_stopped" });
    expect(s.hasFailed).toBe(true);
    expect(s.running).toBe(false);
    expect(search).toHaveBeenCalledTimes(3);
  });

  it("does not report an error that comes after it was cancelled", async () => {
    const ref: { s: SearchSession | null } = { s: null };
    const search = vi.fn<PageSearch>(async () => {
      ref.s?.cancel();
      throw new Error("late");
    });
    const made = session(search, 5);
    ref.s = made.s;
    await made.s.run();
    expect(made.onError).not.toHaveBeenCalled();
  });

  it("copes with a document without pages and a start page out of range", async () => {
    const none = session(fake({}), 0);
    await none.s.run();
    expect(none.s.done).toBe(true);
    const search = fake({ 0: [[0, 1]] });
    const odd = session(search, 3, 99);
    await odd.s.run();
    expect(search.mock.calls.map((c) => c[0])).toEqual([2, 0, 1]);
  });
});

describe("MatchCursor", () => {
  async function ready() {
    const search = fake({ 1: [[0, 3], [9, 3]], 3: [[5, 3]] });
    const { s } = session(search, 5);
    await s.run();
    return new MatchCursor(() => s);
  }

  it("starts at the first match from a page on, or the first of all", async () => {
    const cursor = await ready();
    expect(cursor.start(0)).toEqual({ page: 1, start: 0, len: 3 });
    expect(cursor.start(2)).toEqual({ page: 3, start: 5, len: 3 });
    expect(cursor.start(4)).toEqual({ page: 1, start: 0, len: 3 });
    expect(cursor.position).toBe(1);
    expect(cursor.total).toBe(3);
  });

  it("moves forward and backward and wraps round at both ends", async () => {
    const cursor = await ready();
    cursor.start(0);
    expect(cursor.next()?.start).toBe(9);
    expect(cursor.next()?.page).toBe(3);
    expect(cursor.position).toBe(3);
    expect(cursor.next()).toEqual({ page: 1, start: 0, len: 3 });
    expect(cursor.previous()).toEqual({ page: 3, start: 5, len: 3 });
    expect(cursor.previous()?.start).toBe(9);
    expect(cursor.previous()?.start).toBe(0);
  });

  it("starts from the first or last match when nothing is current", async () => {
    const cursor = await ready();
    expect(cursor.next()?.page).toBe(1);
    cursor.clear();
    expect(cursor.previous()?.page).toBe(3);
    expect(cursor.position).toBe(3);
  });

  it("gives nothing when there are no matches or no search", async () => {
    const { s } = session(fake({}), 3);
    await s.run();
    const none = new MatchCursor(() => s);
    expect(none.start(0)).toBeNull();
    expect(none.next()).toBeNull();
    expect(none.previous()).toBeNull();
    expect(none.position).toBe(0);
    const absent = new MatchCursor(() => null);
    expect(absent.next()).toBeNull();
    expect(absent.total).toBe(0);
  });

  it("keeps its place when matches before it turn up later", async () => {
    // The search started on page 3, so page 1 is searched last.
    let release: () => void = () => undefined;
    const gate = new Promise<void>((r) => (release = r));
    const search = vi.fn<PageSearch>(async (page) => {
      if (page === 1) await gate;
      return page === 1 ? [[0, 3]] : page === 3 ? [[5, 3]] : [];
    });
    const { s } = session(search, 5, 3);
    const run = s.run();
    await wait();
    await wait();
    const cursor = new MatchCursor(() => s);
    cursor.start(3);
    expect(cursor.position).toBe(1);
    release();
    await run;
    // Now there are two matches and the current one is the second.
    expect(cursor.current).toEqual({ page: 3, start: 5, len: 3 });
    expect(cursor.position).toBe(2);
    expect(cursor.total).toBe(2);
  });
});

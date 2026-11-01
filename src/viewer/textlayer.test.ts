import { describe, expect, it } from "vitest";
import {
  TextStore,
  hasBox,
  isEmptySelection,
  lineRange,
  lineRects,
  offsetAt,
  orderSelection,
  parsePageText,
  rangeOnPage,
  selectionText,
  textInRange,
  wordRange,
  type PageText,
} from "./textlayer";

const W = 6;
const H = 10;
const LINE = 14;

/** Lays text out like the engine reports it: `\n` has no box, other characters are 6 x 10 points. */
function layout(text: string, left = 50, top = 100): PageText {
  const chars = Array.from(text);
  const boxes: number[] = [];
  let x = left;
  let y = top;
  for (const ch of chars) {
    if (ch === "\n") {
      boxes.push(0, 0, 0, 0);
      x = left;
      y += LINE;
      continue;
    }
    boxes.push(x, y, x + W, y + H);
    x += W;
  }
  return { chars, boxes };
}

describe("parsePageText", () => {
  it("pairs the text with its boxes by character, not by UTF-16 unit", () => {
    const page = parsePageText("a📕b", Array(12).fill(1));
    expect(page?.chars).toEqual(["a", "📕", "b"]);
  });

  it("refuses text and boxes that do not agree, and numbers that are not numbers", () => {
    expect(parsePageText("abc", Array(8).fill(0))).toBeNull();
    expect(parsePageText("a", [0, 0, 1, Number.NaN])).toBeNull();
    expect(parsePageText("a", [0, 0, 1, Number.POSITIVE_INFINITY])).toBeNull();
    expect(parsePageText("", [])).toEqual({ chars: [], boxes: [] });
  });
});

describe("offsetAt", () => {
  const page = layout("Hello world\nsecond line");

  it("gives the place between characters, on the nearer side of the one hit", () => {
    // 'H' spans x 50..56: the left half is before it, the right half after it.
    expect(offsetAt(page, 51, 105)).toBe(0);
    expect(offsetAt(page, 55, 105)).toBe(1);
    expect(offsetAt(page, 50 + W * 5 + 1, 105)).toBe(5);
  });

  it("uses the nearest line for a point between lines or in a margin", () => {
    // Between the two lines (first ends at y 110, second starts at 114).
    expect(offsetAt(page, 51, 112.5)).toBeGreaterThanOrEqual(12);
    expect(offsetAt(page, 51, 108)).toBe(0);
    // Far to the left of the first line: before its first character.
    expect(offsetAt(page, 0, 105)).toBe(0);
    // Far to the right of the first line: after its last character (index 11 is the line break).
    expect(offsetAt(page, 500, 105)).toBe(11);
    // Far below everything: after the last character.
    expect(offsetAt(page, 500, 900)).toBe(page.chars.length);
    // Far above everything: before the first.
    expect(offsetAt(page, 0, -50)).toBe(0);
  });

  it("copes with a page without text", () => {
    expect(offsetAt({ chars: [], boxes: [] }, 10, 10)).toBe(0);
    expect(offsetAt(layout("\n\n"), 10, 10)).toBe(0);
  });
});

describe("lineRects", () => {
  it("makes one box for each line of a selection", () => {
    const page = layout("Hello world\nsecond line");
    const rects = lineRects(page, 0, page.chars.length);
    expect(rects).toHaveLength(2);
    expect(rects[0]).toEqual({ left: 50, top: 100, right: 50 + W * 11, bottom: 110 });
    expect(rects[1].top).toBe(114);
    expect(rects[1].right).toBe(50 + W * 11);
  });

  it("covers only the part that is selected", () => {
    const page = layout("Hello world");
    expect(lineRects(page, 6, 11)).toEqual([{ left: 50 + W * 6, top: 100, right: 50 + W * 11, bottom: 110 }]);
    expect(lineRects(page, 3, 3)).toEqual([]);
    expect(lineRects(page, 9, 4)).toEqual([]);
    expect(lineRects(page, -5, 2)).toHaveLength(1);
    expect(lineRects(page, 5, 500)).toHaveLength(1);
  });

  it("starts a new box when the text jumps back, as in a second column", () => {
    const left = layout("aaaa", 50, 100);
    const right = layout("bbbb", 300, 100);
    // Reading order: the whole left column, then the right one; the boxes sit on one line.
    const page: PageText = { chars: [...left.chars, ...right.chars], boxes: [...left.boxes, ...right.boxes] };
    expect(lineRects(page, 0, 8)).toHaveLength(1);
    const back: PageText = { chars: [...right.chars, ...left.chars], boxes: [...right.boxes, ...left.boxes] };
    expect(lineRects(back, 0, 8)).toHaveLength(2);
  });

  it("skips characters without a place", () => {
    const page = layout("a\n\nb");
    expect(lineRects(page, 0, 4)).toHaveLength(2);
    expect(hasBox(page, 1)).toBe(false);
  });
});

describe("words and lines", () => {
  const page = layout("Hello, wide world\nnext line");

  it("selects a word, a run of spaces, or a single mark", () => {
    expect(wordRange(page, 2)).toEqual([0, 5]);
    expect(wordRange(page, 5)).toEqual([5, 6]);
    expect(wordRange(page, 6)).toEqual([6, 7]);
    expect(wordRange(page, 8)).toEqual([7, 11]);
    // The last place of the text counts as the last character.
    expect(wordRange(page, 500)).toEqual([23, 27]);
    expect(wordRange({ chars: [], boxes: [] }, 0)).toEqual([0, 0]);
  });

  it("does not run across a line break", () => {
    const joined = layout("one\ntwo");
    expect(wordRange(joined, 2)).toEqual([0, 3]);
    expect(wordRange(joined, 4)).toEqual([4, 7]);
  });

  it("selects a whole line", () => {
    expect(lineRange(page, 3)).toEqual([0, 17]);
    expect(lineRange(page, 20)).toEqual([18, 27]);
    expect(lineRange({ chars: [], boxes: [] }, 0)).toEqual([0, 0]);
  });

  it("knows letters of other scripts and digits", () => {
    const text = layout("blåbær 2026 日本語");
    expect(wordRange(text, 2)).toEqual([0, 6]);
    expect(wordRange(text, 8)).toEqual([7, 11]);
    expect(wordRange(text, 13)).toEqual([12, 15]);
  });
});

describe("selection across pages", () => {
  const sel = (a: [number, number], b: [number, number]) => ({
    anchor: { page: a[0], offset: a[1] },
    focus: { page: b[0], offset: b[1] },
  });

  it("puts the two ends in reading order whichever way it was dragged", () => {
    const forward = orderSelection(sel([1, 5], [3, 2]));
    const backward = orderSelection(sel([3, 2], [1, 5]));
    expect(forward).toEqual(backward);
    expect(forward.start).toEqual({ page: 1, offset: 5 });
    expect(orderSelection(sel([2, 9], [2, 4])).start.offset).toBe(4);
  });

  it("says which part of each page is selected", () => {
    const s = sel([1, 5], [3, 2]);
    expect(rangeOnPage(s, 0)).toBeNull();
    expect(rangeOnPage(s, 1)).toEqual([5, Infinity]);
    expect(rangeOnPage(s, 2)).toEqual([0, Infinity]);
    expect(rangeOnPage(s, 3)).toEqual([0, 2]);
    expect(rangeOnPage(s, 4)).toBeNull();
    expect(rangeOnPage(sel([2, 3], [2, 8]), 2)).toEqual([3, 8]);
    expect(rangeOnPage(sel([2, 3], [2, 3]), 2)).toBeNull();
  });

  it("knows an empty selection", () => {
    expect(isEmptySelection(null)).toBe(true);
    expect(isEmptySelection(sel([1, 2], [1, 2]))).toBe(true);
    expect(isEmptySelection(sel([1, 2], [1, 3]))).toBe(false);
    expect(isEmptySelection(sel([1, 2], [2, 2]))).toBe(false);
  });

  it("builds the text from the pages and lists the pages it does not have", () => {
    const pages = new Map<number, PageText>([
      [0, layout("zero page")],
      [1, layout("one\nsecond line")],
      [3, layout("three")],
    ]);
    const get = (p: number) => pages.get(p);
    const one = selectionText(sel([0, 5], [1, 3]), get);
    expect(one).toEqual({ text: "page\none", missing: [] });
    const across = selectionText(sel([1, 4], [3, 3]), get);
    expect(across.missing).toEqual([2]);
    expect(across.text).toBe("second line\nthr");
    expect(selectionText(sel([1, 0], [1, 0]), get).text).toBe("");
    // Dragged backwards gives the same text.
    expect(selectionText(sel([1, 3], [0, 5]), get).text).toBe("page\none");
  });

  it("cuts the text of a range to the characters of the page", () => {
    const page = layout("abcdef");
    expect(textInRange(page, 2, 100)).toBe("cdef");
    expect(textInRange(page, -3, 2)).toBe("ab");
  });
});

describe("TextStore", () => {
  const page = (n: number): PageText => ({ chars: Array(n).fill("a"), boxes: Array(n * 4).fill(1) });

  it("asks the engine once for a page, however often it is wanted", async () => {
    const fetch = (p: number) => Promise.resolve(page(10 + p));
    let calls = 0;
    const store = new TextStore((p) => {
      calls++;
      return fetch(p);
    });
    const [a, b] = await Promise.all([store.load(1), store.load(1)]);
    expect(a).toBe(b);
    await store.load(1);
    expect(calls).toBe(1);
    expect(store.get(1)?.chars).toHaveLength(11);
    expect(store.get(2)).toBeUndefined();
  });

  it("gives null for a page that cannot be read, and asks again next time", async () => {
    let fail = true;
    const store = new TextStore(() => (fail ? Promise.reject(new Error("no")) : Promise.resolve(page(3))));
    expect(await store.load(0)).toBeNull();
    fail = false;
    expect(await store.load(0)).not.toBeNull();
  });

  it("drops the oldest pages when too much is kept, except pages that are needed", async () => {
    const keep = new Set([0]);
    const store = new TextStore(() => Promise.resolve(page(100)), (p) => keep.has(p), 250);
    for (let p = 0; p < 6; p++) await store.load(p);
    // Limit 250 characters: at most two full pages and the one that is protected.
    expect(store.get(0)).toBeDefined();
    expect(store.get(5)).toBeDefined();
    expect(store.get(1)).toBeUndefined();
    expect(store.size).toBeLessThanOrEqual(3);
  });
});

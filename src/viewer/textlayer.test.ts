import { describe, expect, it } from "vitest";
import {
  MAX_KEPT_CHARS,
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

// ---- right-to-left text -------------------------------------------------------------

/**
 * The page of `rtl_pdf()` (crates/testkit) as the real engine reports it, in points: two lines of
 * Hebrew in reading order. PDFium puts the first letter at the right end of the line, so the boxes
 * of each line run from right to left. The numbers were copied from the output of the engine.
 */
const REAL_RTL: PageText = {
  chars: Array.from("אבגדהוזחטי\nוזחטי"),
  boxes: [
    [405, 75, 414, 96.0625],
    [400, 75, 405, 96.0625],
    [387, 75, 400, 96.0625],
    [373, 75, 387, 96.0625],
    [362, 75, 373, 96.0625],
    [350, 75, 362, 96.0625],
    [337, 75, 350, 96.0625],
    [324, 75, 337, 96.0625],
    [312, 75, 324, 96.0625],
    [300, 75, 312, 96.0625],
    [0, 0, 0, 0],
    [350, 115, 362, 136.0625],
    [337, 115, 350, 136.0625],
    [324, 115, 337, 136.0625],
    [312, 115, 324, 136.0625],
    [300, 115, 312, 136.0625],
  ].flat(),
};

/** A line of right-to-left text drawn with equal boxes: the first letter is the rightmost. */
function mirrored(count: number, right = 200): PageText {
  const boxes: number[] = [];
  for (let i = 0; i < count; i++) boxes.push(right - (i + 1) * W, 100, right - i * W, 100 + H);
  return { chars: Array.from({ length: count }, (_, i) => String.fromCodePoint(0x5d0 + i)), boxes };
}

describe("offsetAt in right-to-left text", () => {
  it("puts the start of the line at its right end, as a reader expects", () => {
    const page = mirrored(10);
    // c0 spans x 194..200. Its right half is before it, its left half after it.
    expect(offsetAt(page, 199, 105)).toBe(0);
    expect(offsetAt(page, 195, 105)).toBe(1);
    expect(offsetAt(page, 193, 105)).toBe(1);
    expect(offsetAt(page, 189, 105)).toBe(2);
    // Beyond the right end: before the first letter. Beyond the left end: after the last.
    expect(offsetAt(page, 400, 105)).toBe(0);
    expect(offsetAt(page, 0, 105)).toBe(10);
  });

  it("keeps the whole line when it is dragged from beyond one end to beyond the other", () => {
    const page = mirrored(10);
    const from = offsetAt(page, 400, 105);
    const to = offsetAt(page, 0, 105);
    expect(textInRange(page, from, to)).toBe(page.chars.join(""));
    // And the other way round, as orderSelection will sort it out.
    const back = orderSelection({
      anchor: { page: 0, offset: offsetAt(page, 0, 105) },
      focus: { page: 0, offset: offsetAt(page, 400, 105) },
    });
    expect(textInRange(page, back.start.offset, back.end.offset)).toBe(page.chars.join(""));
  });

  it("works on the line as the real engine reports it, for the first and the last letter", () => {
    // Press right of the middle of the first letter (x 405..414), release left of the last one.
    expect(offsetAt(REAL_RTL, 412, 85)).toBe(0);
    expect(offsetAt(REAL_RTL, 407, 85)).toBe(1);
    expect(offsetAt(REAL_RTL, 302, 85)).toBe(10);
    expect(offsetAt(REAL_RTL, 310, 85)).toBe(9);
    expect(textInRange(REAL_RTL, offsetAt(REAL_RTL, 600, 85), offsetAt(REAL_RTL, 100, 85))).toBe("אבגדהוזחטי");
    // The second line starts at its right end too (x 350..362).
    expect(offsetAt(REAL_RTL, 360, 125)).toBe(11);
    expect(offsetAt(REAL_RTL, 302, 125)).toBe(16);
    expect(textInRange(REAL_RTL, offsetAt(REAL_RTL, 600, 125), offsetAt(REAL_RTL, 100, 125))).toBe("וזחטי");
  });

  it("still reads a left-to-right line left to right", () => {
    const page = layout("abcdef");
    expect(offsetAt(page, 51, 105)).toBe(0);
    expect(offsetAt(page, 55, 105)).toBe(1);
    expect(offsetAt(page, 500, 105)).toBe(6);
  });

  it("copes with a single letter, and with a left-to-right run inside a right-to-left line", () => {
    const one = mirrored(1);
    // With no neighbour there is nothing to tell the direction from: it reads left to right.
    expect(offsetAt(one, 199, 105)).toBe(1);
    expect(offsetAt(one, 195, 105)).toBe(0);
    // "ab" then "12" drawn left to right inside a right-to-left line: digits keep their own order.
    const mixed: PageText = {
      chars: ["א", "ב", "1", "2", "ג"],
      // Reading order is right to left, except the two digits, which read left to right.
      boxes: [
        [194, 100, 200, 110],
        [188, 100, 194, 110],
        [176, 100, 182, 110],
        [182, 100, 188, 110],
        [170, 100, 176, 110],
      ].flat(),
    };
    expect(offsetAt(mixed, 199, 105)).toBe(0);
    expect(offsetAt(mixed, 169, 105)).toBe(5);
    // The digits: '1' (x 176..182) is left of '2' (182..188). Its left half is before it.
    expect(offsetAt(mixed, 177, 105)).toBe(2);
    expect(offsetAt(mixed, 187, 105)).toBe(4);
  });
});

describe("lineRects in right-to-left text", () => {
  it("makes one box for the whole line, and one for each line", () => {
    expect(lineRects(REAL_RTL, 0, 10)).toEqual([{ left: 300, top: 75, right: 414, bottom: 96.0625 }]);
    expect(lineRects(REAL_RTL, 0, 16)).toHaveLength(2);
    // A part of the line: the middle letters are a box in the middle.
    expect(lineRects(REAL_RTL, 2, 4)).toEqual([{ left: 373, top: 75, right: 400, bottom: 96.0625 }]);
  });
});

describe("TextStore: the limit is hard", () => {
  const page = (n: number): PageText => ({ chars: Array(n).fill("a"), boxes: Array(n * 4).fill(1) });

  it("refuses a page that does not fit, even when everything kept is on screen", async () => {
    const onScreen = new Set([0, 1]);
    const store = new TextStore(() => Promise.resolve(page(100)), (p) => onScreen.has(p), 250);
    await store.load(0);
    await store.load(1);
    onScreen.add(2);
    const text = await store.load(2);
    // The caller gets the text for the moment, but the store does not keep it.
    expect(text?.chars).toHaveLength(100);
    expect(store.get(2)).toBeUndefined();
    expect(store.refused(2)).toBe(true);
    expect(store.keptChars).toBe(200);
    expect(store.get(0)).toBeDefined();
    expect(store.get(1)).toBeDefined();
    // It can try again once it is let in again (it came on screen again).
    store.allow(2);
    expect(store.refused(2)).toBe(false);
  });

  it("drops pages that are not on screen before it refuses anything", async () => {
    const onScreen = new Set([0, 1, 2]);
    const store = new TextStore(() => Promise.resolve(page(100)), (p) => onScreen.has(p), 250);
    await store.load(7);
    await store.load(8);
    await store.load(0);
    expect(store.refused(0)).toBe(false);
    expect(store.get(0)).toBeDefined();
    expect(store.keptChars).toBeLessThanOrEqual(250);
  });

  it("never holds more than the limit, whatever the shape of the document", async () => {
    // 5 000 pages that are all on screen at once, each with the most text a page can have.
    const store = new TextStore(() => Promise.resolve(page(100_000)), () => true, MAX_KEPT_CHARS);
    let refused = 0;
    for (let p = 0; p < 200; p++) {
      await store.load(p);
      if (store.refused(p)) refused++;
      expect(store.keptChars).toBeLessThanOrEqual(MAX_KEPT_CHARS);
    }
    expect(refused).toBeGreaterThan(150);
  });
});

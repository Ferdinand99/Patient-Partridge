import { describe, expect, it } from "vitest";
import {
  MARGIN,
  PAGE_GAP,
  anchorAt,
  bitmapSize,
  computeLayout,
  currentPage,
  pagesInBand,
  scrollTopForAnchor,
  scrollTopForPage,
  visibleRange,
} from "./layout";

const letter = { width: 612, height: 792 };

describe("computeLayout", () => {
  it("scales points to css pixels (100 % is 96 dpi)", () => {
    const l = computeLayout([letter], 1);
    expect(l.widths[0]).toBe(816);
    expect(l.heights[0]).toBe(1056);
  });

  it("stacks pages with a gap and margins", () => {
    const l = computeLayout([letter, letter, letter], 1);
    expect(l.tops).toEqual([MARGIN, MARGIN + 1056 + PAGE_GAP, MARGIN + 2 * (1056 + PAGE_GAP)]);
    expect(l.totalHeight).toBe(MARGIN + 3 * 1056 + 2 * PAGE_GAP + MARGIN);
  });

  it("is as wide as the widest page plus margins", () => {
    const l = computeLayout([letter, { width: 842, height: 595 }], 1);
    expect(l.contentWidth).toBe(Math.round(842 * (96 / 72)) + 2 * MARGIN);
  });

  it("handles a document without pages", () => {
    const l = computeLayout([], 1);
    expect(l.tops).toEqual([]);
    expect(l.totalHeight).toBeGreaterThan(0);
    expect(visibleRange(l, 0, 500)).toBeNull();
    expect(currentPage(l, 0, 500)).toBe(0);
    expect(scrollTopForPage(l, 3)).toBe(0);
  });

  it("never gives a page a size of zero", () => {
    const l = computeLayout([{ width: 0.01, height: 0.01 }], 0.1);
    expect(l.widths[0]).toBe(1);
    expect(l.heights[0]).toBe(1);
  });
});

describe("visibleRange", () => {
  const pages = Array.from({ length: 500 }, () => letter);
  const l = computeLayout(pages, 1);

  it("finds the pages in the viewport", () => {
    expect(visibleRange(l, 0, 800)).toEqual({ first: 0, last: 0 });
    // A viewport that starts in the middle of page 0 and reaches into page 1.
    expect(visibleRange(l, 600, 800)).toEqual({ first: 0, last: 1 });
  });

  it("skips the gap between two pages", () => {
    const top = l.tops[0] + l.heights[0] + 2;
    expect(pagesInBand(l, top, top + 5)).toBeNull();
  });

  it("works deep into a long document", () => {
    const y = l.tops[300] + 10;
    expect(visibleRange(l, y, 100)).toEqual({ first: 300, last: 300 });
  });

  it("widens by the overscan", () => {
    const y = l.tops[300] + 10;
    const wide = visibleRange(l, y, 100, 600);
    expect(wide).toEqual({ first: 299, last: 300 });
  });

  it("returns null past the end", () => {
    expect(visibleRange(l, l.totalHeight + 100, 500)).toBeNull();
  });

  it("returns null for an empty band", () => {
    expect(pagesInBand(l, 100, 100)).toBeNull();
  });
});

describe("currentPage", () => {
  const l = computeLayout(
    Array.from({ length: 10 }, () => letter),
    1,
  );

  it("is the page with most of the viewport", () => {
    expect(currentPage(l, 0, 800)).toBe(0);
    // Mostly on page 1.
    expect(currentPage(l, l.tops[1] - 100, 800)).toBe(1);
    expect(currentPage(l, 0, 200)).toBe(0);
  });
});

describe("scrollTopForPage", () => {
  const l = computeLayout(
    Array.from({ length: 5 }, () => letter),
    1,
  );

  it("puts the page near the top", () => {
    expect(scrollTopForPage(l, 0)).toBe(Math.max(0, MARGIN - PAGE_GAP));
    expect(scrollTopForPage(l, 2)).toBe(l.tops[2] - PAGE_GAP);
  });

  it("clamps the index", () => {
    expect(scrollTopForPage(l, -4)).toBe(scrollTopForPage(l, 0));
    expect(scrollTopForPage(l, 99)).toBe(scrollTopForPage(l, 4));
  });
});

describe("anchors", () => {
  const pages = Array.from({ length: 20 }, () => letter);

  it("keeps the reading position when the zoom changes", () => {
    const before = computeLayout(pages, 1);
    const scrollTop = before.tops[7] + before.heights[7] * 0.25;
    const anchor = anchorAt(before, scrollTop);
    expect(anchor.page).toBe(7);
    expect(anchor.fraction).toBeCloseTo(0.25, 3);

    const after = computeLayout(pages, 2);
    const moved = scrollTopForAnchor(after, anchor);
    const again = anchorAt(after, moved);
    expect(again.page).toBe(7);
    expect(again.fraction).toBeCloseTo(0.25, 2);
  });

  it("copes with no pages and with the top of the document", () => {
    expect(anchorAt(computeLayout([], 1), 50)).toEqual({ page: 0, fraction: 0 });
    expect(scrollTopForAnchor(computeLayout([], 1), { page: 3, fraction: 0.5 })).toBe(0);
    const l = computeLayout(pages, 1);
    expect(scrollTopForAnchor(l, anchorAt(l, 0))).toBe(0);
  });
});

describe("bitmapSize", () => {
  it("matches the screen pixels", () => {
    expect(bitmapSize(letter, 1, 1)).toEqual({ width: 816, height: 1056 });
    expect(bitmapSize(letter, 1, 2)).toEqual({ width: 1632, height: 2112 });
  });

  it("ignores pixel ratios below 1", () => {
    expect(bitmapSize(letter, 1, 0.5)).toEqual({ width: 816, height: 1056 });
  });

  it("is capped so memory stays bounded and the shape is kept", () => {
    const size = bitmapSize(letter, 8, 2);
    expect(size.width * size.height).toBeLessThanOrEqual(16_000_000);
    expect(size.width / size.height).toBeCloseTo(612 / 792, 2);
  });
});

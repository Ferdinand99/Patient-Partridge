import { describe, expect, it } from "vitest";
import {
  MIN_CROP_SIDE,
  PageSelection,
  TOOL_NOTES,
  boxOfMargins,
  clampMargins,
  describePages,
  dragBox,
  insertionForStep,
  isIdentity,
  marginsOfBox,
  orderAfterMove,
  pagesWord,
  placesAfterMove,
  type Box,
  type Handle,
} from "./pages";

describe("PageSelection", () => {
  const none = { shift: false, ctrl: false };

  it("a plain click picks one page and nothing else", () => {
    const s = new PageSelection(10);
    s.click(3, none);
    s.click(5, none);
    expect(s.toArray()).toEqual([5]);
    expect(s.cursor).toBe(5);
  });

  it("Ctrl adds and removes a page, Shift picks the run from the anchor", () => {
    const s = new PageSelection(10);
    s.click(2, none);
    s.click(5, { shift: false, ctrl: true });
    expect(s.toArray()).toEqual([2, 5]);
    s.click(2, { shift: false, ctrl: true });
    expect(s.toArray()).toEqual([5]);
    // The last page clicked is where a run starts, as in a file manager.
    s.click(8, { shift: true, ctrl: false });
    expect(s.toArray()).toEqual([2, 3, 4, 5, 6, 7, 8]);
    // A run the other way round, from the same anchor.
    s.click(0, { shift: true, ctrl: false });
    expect(s.toArray()).toEqual([0, 1, 2]);
    // Ctrl and Shift together keep what was picked.
    s.click(9, { shift: true, ctrl: true });
    expect(s.toArray()).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  });

  it("the arrow keys move one page, Shift extends the run, and both stop at the ends", () => {
    const s = new PageSelection(4);
    s.step(1, false);
    expect(s.toArray()).toEqual([0]);
    s.step(1, true);
    s.step(1, true);
    expect(s.toArray()).toEqual([0, 1, 2]);
    s.step(-1, true);
    expect(s.toArray()).toEqual([0, 1]);
    s.step(10, false);
    expect(s.toArray()).toEqual([3]);
    s.step(10, false);
    expect(s.toArray()).toEqual([3]);
    s.step(-100, false);
    expect(s.toArray()).toEqual([0]);
    // Up from nothing goes to the last page.
    const t = new PageSelection(4);
    t.step(-1, false);
    expect(t.toArray()).toEqual([3]);
  });

  it("Ctrl and the arrows move the cursor without picking, and the end keys go to the ends", () => {
    const s = new PageSelection(6);
    s.only(2);
    s.moveCursor(2);
    expect(s.cursor).toBe(4);
    expect(s.toArray()).toEqual([2]);
    s.toggle(s.cursor);
    expect(s.toArray()).toEqual([2, 4]);
    s.edge(true, false);
    expect(s.toArray()).toEqual([5]);
    s.edge(false, true);
    expect(s.toArray()).toEqual([0, 1, 2, 3, 4, 5]);
  });

  it("Select all picks every page; an empty document picks nothing", () => {
    const s = new PageSelection(3);
    s.all();
    expect(s.toArray()).toEqual([0, 1, 2]);
    const e = new PageSelection(0);
    e.all();
    e.step(1, false);
    e.edge(true, false);
    e.only(0);
    expect(e.toArray()).toEqual([]);
  });

  it("pages past the end are let go of when the count falls", () => {
    const s = new PageSelection(10);
    s.click(8, { shift: false, ctrl: false });
    s.click(2, { shift: false, ctrl: true });
    s.setCount(5);
    expect(s.toArray()).toEqual([2]);
    expect(s.cursor).toBe(2);
    s.setCount(1);
    expect(s.toArray()).toEqual([]);
  });

  it("clicks outside the document do nothing", () => {
    const s = new PageSelection(3);
    s.only(-1);
    s.only(3);
    s.toggle(7);
    s.rangeTo(9);
    expect(s.toArray()).toEqual([]);
  });
});

describe("orderAfterMove", () => {
  it("moves one page to the front, to the end and into the middle", () => {
    expect(orderAfterMove(5, [3], 0)).toEqual([3, 0, 1, 2, 4]);
    expect(orderAfterMove(5, [1], 5)).toEqual([0, 2, 3, 4, 1]);
    expect(orderAfterMove(5, [0], 3)).toEqual([1, 2, 0, 3, 4]);
    expect(orderAfterMove(5, [4], 2)).toEqual([0, 1, 4, 2, 3]);
  });

  it("moves a group, keeping its order, to one place", () => {
    expect(orderAfterMove(8, [1, 2, 6], 4)).toEqual([0, 3, 1, 2, 6, 4, 5, 7]);
    expect(orderAfterMove(8, [6, 1, 2], 4)).toEqual([0, 3, 1, 2, 6, 4, 5, 7]);
    expect(orderAfterMove(8, [0, 7], 3)).toEqual([1, 2, 0, 7, 3, 4, 5, 6]);
  });

  it("dropping where the pages already are changes nothing", () => {
    for (const [picked, at] of [
      [[2], 2],
      [[2], 3],
      [[1, 2], 1],
      [[1, 2], 3],
      [[1, 2], 2],
    ] as const) {
      expect(isIdentity(orderAfterMove(5, [...picked], at))).toBe(true);
    }
    expect(isIdentity(orderAfterMove(5, [2], 4))).toBe(false);
  });

  it("is always a permutation, with the moved pages in a row", () => {
    // Every pick of up to three pages of six, and every place.
    for (let mask = 1; mask < 1 << 6; mask++) {
      const picked = [0, 1, 2, 3, 4, 5].filter((p) => mask & (1 << p));
      if (picked.length > 3) continue;
      for (let at = 0; at <= 6; at++) {
        const order = orderAfterMove(6, picked, at);
        expect([...order].sort((a, b) => a - b)).toEqual([0, 1, 2, 3, 4, 5]);
        const places = placesAfterMove(order, picked);
        expect(places).toHaveLength(picked.length);
        expect(places[places.length - 1] - places[0]).toBe(picked.length - 1);
        // The pages that did not move keep their order.
        const rest = order.filter((p) => !picked.includes(p));
        expect(rest).toEqual([...rest].sort((a, b) => a - b));
      }
    }
  });

  it("ignores pages that are not there", () => {
    expect(orderAfterMove(3, [-1, 7, 1], 0)).toEqual([1, 0, 2]);
    expect(orderAfterMove(3, [], 1)).toEqual([0, 1, 2]);
    expect(orderAfterMove(3, [1], 99)).toEqual([0, 2, 1]);
    expect(orderAfterMove(3, [1], -5)).toEqual([1, 0, 2]);
  });

  it("tells where the moved pages are afterwards", () => {
    expect(placesAfterMove([3, 0, 1, 2, 4], [3])).toEqual([0]);
    expect(placesAfterMove([0, 3, 1, 2, 6, 4, 5, 7], [1, 2, 6])).toEqual([2, 3, 4]);
  });
});

describe("insertionForStep", () => {
  it("moves a group one page up or down, past the page in the way", () => {
    expect(orderAfterMove(5, [2], insertionForStep(5, [2], -1))).toEqual([0, 2, 1, 3, 4]);
    expect(orderAfterMove(5, [2], insertionForStep(5, [2], 1))).toEqual([0, 1, 3, 2, 4]);
    expect(orderAfterMove(6, [2, 3], insertionForStep(6, [2, 3], 1))).toEqual([0, 1, 4, 2, 3, 5]);
    expect(orderAfterMove(6, [2, 3], insertionForStep(6, [2, 3], -1))).toEqual([0, 2, 3, 1, 4, 5]);
  });

  it("does nothing at the ends", () => {
    expect(isIdentity(orderAfterMove(5, [0], insertionForStep(5, [0], -1)))).toBe(true);
    expect(isIdentity(orderAfterMove(5, [4], insertionForStep(5, [4], 1)))).toBe(true);
    expect(isIdentity(orderAfterMove(5, [3, 4], insertionForStep(5, [3, 4], 1)))).toBe(true);
  });
});

describe("words", () => {
  it("writes page numbers as people do", () => {
    expect(describePages([0])).toBe("1");
    expect(describePages([2, 3, 4, 8])).toBe("3-5, 9");
    expect(describePages([4, 2, 3, 3, 8])).toBe("3-5, 9");
    expect(describePages([0, 1])).toBe("1, 2");
    expect(describePages([])).toBe("");
    expect(pagesWord(1)).toBe("1 page");
    expect(pagesWord(2)).toBe("2 pages");
    expect(pagesWord(1234)).toBe("1,234 pages");
  });

  it("says plainly what a crop does not do, and what the other tools lose", () => {
    expect(TOOL_NOTES.crop).toMatch(/does not remove anything/);
    expect(TOOL_NOTES.crop).toMatch(/still in the file/);
    expect(TOOL_NOTES.delete).toMatch(/removes it from the file/);
    expect(TOOL_NOTES.deleteForms).toMatch(/form fields/i);
    expect(TOOL_NOTES.move).toMatch(/stop working/);
    expect(TOOL_NOTES.insert).toMatch(/Bookmarks/);
    expect(TOOL_NOTES.insert).toMatch(/signatures/);
    expect(TOOL_NOTES.extract).toMatch(/no password/);
    expect(TOOL_NOTES.unprotectedCopy).toMatch(/will not be/);
    expect(TOOL_NOTES.signedCopy).toMatch(/signature/);
  });
});

describe("crop geometry", () => {
  it("margins and boxes are the same thing seen from two sides", () => {
    const m = { left: 10, top: 20, right: 30, bottom: 40 };
    const box = boxOfMargins(m, 200, 300);
    expect(box).toEqual({ left: 10, top: 20, right: 170, bottom: 260 });
    expect(marginsOfBox(box, 200, 300)).toEqual(m);
  });

  it("margins are never negative and always leave the smallest side", () => {
    expect(clampMargins({ left: -5, top: -1, right: 0, bottom: 0 }, 100, 100)).toEqual({
      left: 0,
      top: 0,
      right: 0,
      bottom: 0,
    });
    const m = clampMargins({ left: 90, top: 90, right: 90, bottom: 90 }, 100, 100);
    expect(100 - m.left - m.right).toBeGreaterThanOrEqual(MIN_CROP_SIDE);
    expect(100 - m.top - m.bottom).toBeGreaterThanOrEqual(MIN_CROP_SIDE);
    // A page smaller than the smallest side cannot be cropped at all.
    expect(clampMargins({ left: 5, top: 5, right: 5, bottom: 5 }, 6, 6)).toEqual({
      left: 0,
      top: 0,
      right: 0,
      bottom: 0,
    });
    // Numbers that are not numbers do not get through as numbers.
    const bad = clampMargins({ left: Number.NaN, top: 1, right: 1, bottom: 1 }, 100, 100);
    expect(Number.isNaN(bad.left)).toBe(false);
  });

  it("dragging a handle moves its edges only, and keeps the box inside the page", () => {
    const box: Box = { left: 20, top: 20, right: 80, bottom: 80 };
    expect(dragBox(box, "e", 10, 5, 100, 100, 8)).toEqual({ left: 20, top: 20, right: 90, bottom: 80 });
    expect(dragBox(box, "nw", -30, -30, 100, 100, 8)).toEqual({ left: 0, top: 0, right: 80, bottom: 80 });
    expect(dragBox(box, "se", 50, 50, 100, 100, 8)).toEqual({ left: 20, top: 20, right: 100, bottom: 100 });
    // Past the opposite edge: stops at the smallest side.
    expect(dragBox(box, "e", -100, 0, 100, 100, 8)).toEqual({ left: 20, top: 20, right: 28, bottom: 80 });
    expect(dragBox(box, "n", 0, 100, 100, 100, 8)).toEqual({ left: 20, top: 72, right: 80, bottom: 80 });
    // The whole box moves without changing size, and stops at the page.
    expect(dragBox(box, "move", 100, -100, 100, 100, 8)).toEqual({ left: 40, top: 0, right: 100, bottom: 60 });
  });

  it("every handle, dragged anywhere, leaves a box that fits", () => {
    const handles: Handle[] = ["n", "s", "e", "w", "ne", "nw", "se", "sw", "move"];
    for (const h of handles) {
      for (const dx of [-500, -37, 0, 41, 500]) {
        for (const dy of [-500, -13, 0, 77, 500]) {
          const b = dragBox({ left: 30, top: 40, right: 70, bottom: 90 }, h, dx, dy, 100, 120, 8);
          expect(b.left).toBeGreaterThanOrEqual(0);
          expect(b.top).toBeGreaterThanOrEqual(0);
          expect(b.right).toBeLessThanOrEqual(100);
          expect(b.bottom).toBeLessThanOrEqual(120);
          expect(b.right - b.left).toBeGreaterThanOrEqual(8 - 1e-9);
          expect(b.bottom - b.top).toBeGreaterThanOrEqual(8 - 1e-9);
        }
      }
    }
  });
});

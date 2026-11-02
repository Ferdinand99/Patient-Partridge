import { describe, expect, it } from "vitest";
import { MAX_TABS, pickSessionTabs, wantedPdfPaths } from "./paths";

describe("wantedPdfPaths", () => {
  it("takes every file that looks like a PDF and no repeats", () => {
    expect(wantedPdfPaths(["/a/x.pdf", "/a/notes.txt", "/a/y.PDF", "/a/x.pdf"])).toEqual(["/a/x.pdf", "/a/y.PDF"]);
  });

  it("tries the first path when none looks like a PDF", () => {
    expect(wantedPdfPaths(["/a/scan", "/a/other"])).toEqual(["/a/scan"]);
  });

  it("gives nothing for nothing", () => {
    expect(wantedPdfPaths([])).toEqual([]);
    expect(wantedPdfPaths([""])).toEqual([]);
  });
});

describe("pickSessionTabs", () => {
  it("keeps the order and finds the tab that was in view", () => {
    expect(pickSessionTabs({ paths: ["/a", "/b", "/c"], active: 2 }, 16)).toEqual({
      paths: ["/a", "/b", "/c"],
      active: 2,
      left: 0,
    });
  });

  it("drops empty paths and repeats, and follows the tab in view to its new place", () => {
    expect(pickSessionTabs({ paths: ["", "/a", "/a", "/b"], active: 3 }, 16)).toEqual({
      paths: ["/a", "/b"],
      active: 1,
      left: 0,
    });
  });

  it("shows the first tab when none was in view or the index points nowhere", () => {
    expect(pickSessionTabs({ paths: ["/a", "/b"], active: null }, 16).active).toBe(0);
    expect(pickSessionTabs({ paths: ["/a", "/b"], active: 9 }, 16).active).toBe(0);
  });

  it("gives nothing for nothing", () => {
    expect(pickSessionTabs(null, 16)).toEqual({ paths: [], active: 0, left: 0 });
    expect(pickSessionTabs({ paths: [], active: null }, 16).paths).toEqual([]);
  });

  it("cuts to the room that is left, keeping the tab in view and the first of the others", () => {
    const paths = ["/a", "/b", "/c", "/d", "/e"];
    expect(pickSessionTabs({ paths, active: 4 }, 3)).toEqual({ paths: ["/a", "/b", "/e"], active: 2, left: 2 });
    expect(pickSessionTabs({ paths, active: 0 }, 2)).toEqual({ paths: ["/a", "/b"], active: 0, left: 3 });
    expect(pickSessionTabs({ paths, active: 1 }, 0)).toEqual({ paths: [], active: 0, left: 5 });
  });

  it("never gives more than the document limit, whatever the room", () => {
    const many = Array.from({ length: MAX_TABS + 9 }, (_, i) => "/" + i);
    const pick = pickSessionTabs({ paths: many, active: null }, 1000);
    expect(pick.paths).toHaveLength(MAX_TABS);
    expect(pick.left).toBe(9);
  });
});

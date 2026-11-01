import { describe, expect, it } from "vitest";
import { MAX_OPEN_AT_ONCE, pickPdfPath, pickPdfPaths } from "./paths";

describe("pickPdfPath", () => {
  it("prefers a file that looks like a PDF", () => {
    expect(pickPdfPath(["/a/notes.txt", "/a/book.PDF", "/a/b.pdf"])).toBe("/a/book.PDF");
  });

  it("takes the first path when none looks like a PDF", () => {
    expect(pickPdfPath(["/a/scan", "/a/other"])).toBe("/a/scan");
  });

  it("returns null when there is nothing", () => {
    expect(pickPdfPath([])).toBeNull();
    expect(pickPdfPath([""])).toBeNull();
  });
});

describe("pickPdfPaths", () => {
  it("takes every file that looks like a PDF and no repeats", () => {
    expect(pickPdfPaths(["/a/x.pdf", "/a/notes.txt", "/a/y.PDF", "/a/x.pdf"])).toEqual(["/a/x.pdf", "/a/y.PDF"]);
  });

  it("tries the first path when none looks like a PDF", () => {
    expect(pickPdfPaths(["/a/scan", "/a/other"])).toEqual(["/a/scan"]);
  });

  it("gives nothing for nothing", () => {
    expect(pickPdfPaths([])).toEqual([]);
    expect(pickPdfPaths([""])).toEqual([]);
  });

  it("opens at most a few files at once", () => {
    const many = Array.from({ length: 30 }, (_, i) => `/a/${i}.pdf`);
    expect(pickPdfPaths(many)).toHaveLength(MAX_OPEN_AT_ONCE);
    expect(pickPdfPaths(many)[0]).toBe("/a/0.pdf");
  });
});

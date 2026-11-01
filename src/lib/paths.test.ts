import { describe, expect, it } from "vitest";
import { pickPdfPath } from "./paths";

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

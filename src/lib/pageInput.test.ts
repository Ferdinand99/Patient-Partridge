import { describe, expect, it } from "vitest";
import { parsePageInput } from "./pageInput";

describe("parsePageInput", () => {
  it("turns a page number into a zero-based index", () => {
    expect(parsePageInput("1", 10)).toBe(0);
    expect(parsePageInput("7", 10)).toBe(6);
    expect(parsePageInput(" 10 ", 10)).toBe(9);
  });

  it("keeps the page inside the document", () => {
    expect(parsePageInput("999", 10)).toBe(9);
  });

  it("refuses text that is not a page number", () => {
    for (const text of ["", " ", "0", "-3", "abc", "1.5", "2a", "1e3"]) {
      expect(parsePageInput(text, 10), JSON.stringify(text)).toBeNull();
    }
  });

  it("refuses everything when there are no pages", () => {
    expect(parsePageInput("1", 0)).toBeNull();
  });

  it("does not overflow on very long numbers", () => {
    expect(parsePageInput("99999999999999999999", 10)).toBeNull();
  });
});

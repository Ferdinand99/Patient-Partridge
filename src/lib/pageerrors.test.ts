import { describe, expect, it } from "vitest";
import { describePageError } from "./pageerrors";

describe("the words for an error of a page tool", () => {
  it("names the other file when pages were being added from one, and says the document was not changed", () => {
    for (const code of ["insert_source_failed", "import_failed", "corrupt", "io", "unsupported", "too_many_pages"]) {
      const text = describePageError({ code, message: "reason" }, "report.pdf", "scan.pdf");
      expect(text.title).toBe("The pages could not be added");
      expect(text.detail).toContain('"scan.pdf" could not be used (reason)');
      expect(text.detail).toContain('"report.pdf" was not changed');
      expect(text.detail).not.toMatch(/damaged/);
    }
  });

  it("does not take a fault of the document itself for a fault of the other file", () => {
    const text = describePageError({ code: "page_change_failed", message: "" }, "report.pdf", "scan.pdf");
    expect(text.detail).not.toContain("scan.pdf");
  });

  it("says what a document that forbids a change forbids", () => {
    const text = describePageError(
      { code: "pages_not_allowed", message: "the document does not allow pages to be inserted, deleted, moved or turned" },
      "locked.pdf",
    );
    expect(text.title).toBe("This document does not allow that");
    expect(text.detail).toMatch(/^The document does not allow pages to be inserted/);
    expect(text.detail).toContain('"locked.pdf" was not changed');
  });

  it("explains a file that changed on the disk instead of saying that something went wrong", () => {
    for (const code of ["source_changed", "source_unreadable"]) {
      const text = describePageError({ code, message: "" }, "report.pdf");
      expect(text.title).toBe("The file changed while it was open");
      expect(text.detail).toContain("open it again");
    }
  });

  it("keeps the words it had for the rest", () => {
    expect(describePageError({ code: "too_many_pages", message: "" }, "x.pdf").title).toBe("Too many pages for this tool");
    expect(describePageError({ code: "snapshot_gone", message: "" }, "x.pdf").title).toBe(
      "That change cannot be taken back any more",
    );
  });
});

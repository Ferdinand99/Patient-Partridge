import { describe, expect, it } from "vitest";
import { describeError, fileNameOf, isEngineLost, toAppError } from "./errors";

describe("toAppError", () => {
  it("passes the shape Tauri rejects with", () => {
    expect(toAppError({ code: "corrupt", message: "bad" })).toEqual({
      code: "corrupt",
      message: "bad",
    });
  });

  it("wraps other things", () => {
    expect(toAppError(new Error("x"))).toEqual({ code: "internal", message: "x" });
    expect(toAppError("text")).toEqual({ code: "internal", message: "text" });
    expect(toAppError(null).code).toBe("internal");
    expect(toAppError({ code: 4 }).code).toBe("internal");
  });
});

describe("describeError", () => {
  it("asks for a password when the file needs one", () => {
    const d = describeError({ code: "password_required", message: "" }, "tax.pdf");
    expect(d.action).toBe("password");
    expect(d.detail).toContain("tax.pdf");
    expect(d.title.toLowerCase()).toContain("password");
  });

  it("says the password was wrong and asks again", () => {
    const d = describeError({ code: "password_incorrect", message: "" }, "tax.pdf");
    expect(d.action).toBe("password");
    expect(d.title.toLowerCase()).toContain("not correct");
  });

  it("explains a damaged file", () => {
    const d = describeError({ code: "corrupt", message: "" }, "x.pdf");
    expect(d.action).toBe("none");
    expect(d.detail).toContain("damaged");
  });

  it("explains an engine crash and says the app is still running", () => {
    const d = describeError({ code: "engine_stopped", message: "it exited" }, "x.pdf");
    expect(d.action).toBe("retry");
    expect(d.detail).toContain("still running");
    expect(d.detail).toContain('"x.pdf"');
  });

  it("works without a file name", () => {
    expect(describeError({ code: "engine_stopped", message: "" }).detail).toContain("this file");
  });

  it("has a readable fallback for unknown codes", () => {
    const d = describeError({ code: "something_new", message: "details here" });
    expect(d.detail).toBe("details here");
    expect(describeError({ code: "x", message: "" }).detail).not.toBe("");
  });

  it("covers every code the engine can send", () => {
    const codes = [
      "io",
      "corrupt",
      "password_required",
      "password_incorrect",
      "unsupported",
      "unknown_document",
      "page_out_of_range",
      "invalid_request",
      "render_failed",
      "engine_unavailable",
      "protocol_mismatch",
      "internal",
      "engine_stopped",
    ];
    for (const code of codes) {
      const d = describeError({ code, message: "m" }, "f.pdf");
      expect(d.title.length, code).toBeGreaterThan(0);
      expect(d.detail.length, code).toBeGreaterThan(0);
    }
  });
});

describe("isEngineLost", () => {
  it("is true only when the document went away with the engine", () => {
    expect(isEngineLost({ code: "engine_stopped", message: "" })).toBe(true);
    expect(isEngineLost({ code: "unknown_document", message: "" })).toBe(true);
    expect(isEngineLost({ code: "render_failed", message: "" })).toBe(false);
  });
});

describe("fileNameOf", () => {
  it("handles both separators", () => {
    expect(fileNameOf("C:\\Users\\me\\a b.pdf")).toBe("a b.pdf");
    expect(fileNameOf("/home/me/a.pdf")).toBe("a.pdf");
    expect(fileNameOf("plain.pdf")).toBe("plain.pdf");
  });
});

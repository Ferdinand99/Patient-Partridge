import { describe, expect, it } from "vitest";
import { describeFontScripts, scriptsIn } from "./scripts";

describe("scriptsIn", () => {
  it("finds the scripts that need a font", () => {
    expect([...scriptsIn("Hello 日本語 かな")].sort()).toEqual(["han", "japanese"]);
    expect([...scriptsIn("한국어")]).toEqual(["korean"]);
    expect([...scriptsIn("שלום")]).toEqual(["hebrew"]);
    expect([...scriptsIn("مرحبا")]).toEqual(["arabic"]);
    expect([...scriptsIn("สวัสดี")]).toEqual(["thai"]);
  });

  it("finds nothing in Latin, Greek and Cyrillic text", () => {
    expect(scriptsIn("Hello, wörld! Ελληνικά Русский 123").size).toBe(0);
    expect(scriptsIn("").size).toBe(0);
  });
});

describe("describeFontScripts", () => {
  it("makes a short phrase from the names the engine uses", () => {
    expect(describeFontScripts(["thai"])).toBe("Thai");
    expect(describeFontScripts(["thai", "arabic"])).toBe("Thai and Arabic");
    expect(describeFontScripts(["chinese-simplified", "korean", "hebrew"])).toBe(
      "Simplified Chinese, Korean and Hebrew",
    );
    expect(describeFontScripts([])).toBe("");
  });
});

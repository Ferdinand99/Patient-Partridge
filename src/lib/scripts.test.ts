import { describe, expect, it } from "vitest";
import { describeScripts, scriptsIn, scriptsWithoutFont } from "./scripts";

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

describe("scriptsWithoutFont", () => {
  it("lists what the computer has no font for", () => {
    const seen = new Set(["han", "thai", "arabic"] as const);
    expect(scriptsWithoutFont(seen, ["japanese", "arabic"])).toEqual(["thai"]);
    expect(scriptsWithoutFont(seen, [])).toEqual(["han", "thai", "arabic"]);
  });

  it("lets any CJK font cover the shared Chinese characters, but not kana or hangul", () => {
    expect(scriptsWithoutFont(["han"], ["korean"])).toEqual([]);
    expect(scriptsWithoutFont(["japanese"], ["korean"])).toEqual(["japanese"]);
    expect(scriptsWithoutFont(["korean"], ["chinese-simplified"])).toEqual(["korean"]);
  });
});

describe("describeScripts", () => {
  it("makes a short phrase", () => {
    expect(describeScripts(["thai"])).toBe("Thai");
    expect(describeScripts(["thai", "arabic"])).toBe("Thai and Arabic");
    expect(describeScripts(["han", "thai", "arabic"])).toBe("Chinese characters, Thai and Arabic");
  });
});

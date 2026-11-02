import { describe, expect, it } from "vitest";
import {
  MAX_NOTE_BYTES,
  MAX_STROKE_POINTS,
  PALETTE,
  STAMPS,
  TOOLS,
  clampToBytes,
  clientToPage,
  describeAnnotation,
  isMarkupTool,
  keepOnPage,
  simplifyStroke,
  snippet,
  stampSize,
  swatchOf,
  toolForKey,
  utf8Length,
  type Annotation,
} from "./annotations";

const note = (over: Partial<Annotation> = {}): Annotation => ({
  id: 1,
  kind: "note",
  can: { editText: true, recolour: false, move: true, remove: true, restore: true },
  rect: [10, 20, 30, 40],
  color: [255, 235, 59, 255],
  text: "",
  author: "",
  ...over,
});

describe("tools and colours", () => {
  it("has one letter for each tool, and no letter twice", () => {
    const keys = TOOLS.map((t) => t.key);
    expect(new Set(keys).size).toBe(TOOLS.length);
    for (const tool of TOOLS) expect(toolForKey(tool.key)).toBe(tool.id);
    expect(toolForKey("H")).toBe("highlight");
    expect(toolForKey("x")).toBeNull();
    expect(toolForKey("Enter")).toBeNull();
  });

  it("knows which tools mark text", () => {
    expect(["highlight", "underline", "strikeout"].every((t) => isMarkupTool(t as never))).toBe(true);
    expect(["select", "note", "pen", "stamp"].some((t) => isMarkupTool(t as never))).toBe(false);
  });

  it("has distinct, named colours and stamps", () => {
    expect(new Set(PALETTE.map((s) => s.id)).size).toBe(PALETTE.length);
    expect(PALETTE.every((s) => s.label.length > 0 && s.rgba[3] === 255)).toBe(true);
    expect(swatchOf([126, 217, 87, 255])?.id).toBe("green");
    expect(swatchOf([1, 2, 3, 255])).toBeNull();
    expect(STAMPS.map((s) => s.id)).toEqual(["approved", "rejected", "draft", "final", "confidential", "reviewed"]);
  });
});

describe("text limits", () => {
  it("counts bytes the way the engine does", () => {
    expect(utf8Length("abc")).toBe(3);
    expect(utf8Length("å")).toBe(2);
    expect(utf8Length("✓")).toBe(3);
    expect(utf8Length("\u{1F600}")).toBe(4);
  });

  it("cuts on a character, never in the middle of one", () => {
    expect(clampToBytes("abc", 10)).toBe("abc");
    expect(clampToBytes("ååå", 5)).toBe("åå");
    expect(clampToBytes("\u{1F600}\u{1F600}", 7)).toBe("\u{1F600}");
    const long = "x".repeat(MAX_NOTE_BYTES + 50);
    expect(utf8Length(clampToBytes(long))).toBe(MAX_NOTE_BYTES);
    expect(clampToBytes("✓".repeat(2000))).toHaveLength(Math.floor(MAX_NOTE_BYTES / 3));
  });
});

describe("what a screen reader says", () => {
  it("names the kind, the page, the author and a cut of the text", () => {
    expect(describeAnnotation(note(), 0)).toBe("Note, page 1");
    expect(describeAnnotation(note({ kind: "ink" }), 4)).toBe("Drawing, page 5");
    expect(describeAnnotation(note({ text: "Hello", author: "Ann" }), 1)).toBe("Note by Ann, page 2: Hello");
    const long = describeAnnotation(note({ text: "w ".repeat(200) }), 0);
    expect(Array.from(long).length).toBeLessThan(120);
    expect(long.endsWith("…")).toBe(true);
  });

  it("keeps text from a file as text: markup stays characters", () => {
    const hostile = "<img src=x onerror=alert(1)>\n\n  tabs\t and   spaces";
    expect(snippet(hostile)).toBe("<img src=x onerror=alert(1)> tabs and spaces");
    expect(describeAnnotation(note({ text: hostile }), 0)).toContain("<img src=x onerror=alert(1)>");
  });
});

describe("places on a page", () => {
  const page = { width: 612, height: 792 };

  it("turns a point on the screen into points on the page at any zoom", () => {
    for (const scale of [0.5, 1, 2, 3.7]) {
      const box = { left: 100, top: 50, width: 612 * scale, height: 792 * scale };
      const [x, y] = clientToPage(box, page, 100 + 72 * scale, 50 + 300 * scale);
      expect(x).toBeCloseTo(72, 6);
      expect(y).toBeCloseTo(300, 6);
    }
    // A page of landscape shape.
    const [x, y] = clientToPage({ left: 0, top: 0, width: 396, height: 306 }, { width: 792, height: 612 }, 198, 153);
    expect([x, y]).toEqual([396, 306]);
  });

  it("gives nothing for a page without a size", () => {
    expect(clientToPage({ left: 0, top: 0, width: 0, height: 10 }, page, 5, 5)).toEqual([0, 0]);
  });

  it("keeps a box on the page", () => {
    expect(keepOnPage(-5, -5, { width: 100, height: 40 }, page)).toEqual([0, 0]);
    expect(keepOnPage(600, 780, { width: 100, height: 40 }, page)).toEqual([512, 752]);
    expect(keepOnPage(50, 60, { width: 100, height: 40 }, page)).toEqual([50, 60]);
    expect(keepOnPage(10, 10, { width: 1000, height: 40 }, page)).toEqual([0, 10]);
  });

  it("sizes a stamp by its wording, as the engine does", () => {
    const approved = stampSize("approved");
    expect(approved.width).toBeCloseTo(8 * 26 * 0.72 + 18, 6);
    expect(approved.height).toBe(44);
    expect(stampSize("confidential").width).toBeGreaterThan(approved.width);
  });
});

describe("strokes", () => {
  it("drops points that crowd the last one and keeps both ends", () => {
    const points: [number, number][] = [
      [0, 0],
      [0.1, 0],
      [0.2, 0],
      [5, 0],
      [5.1, 0],
      [10, 0],
    ];
    expect(simplifyStroke(points)).toEqual([
      [0, 0],
      [5, 0],
      [10, 0],
    ]);
  });

  it("leaves a dot and a straight pair alone", () => {
    expect(simplifyStroke([[3, 4]])).toEqual([[3, 4]]);
    expect(simplifyStroke([[3, 4], [3, 4]])).toEqual([[3, 4], [3, 4]]);
  });

  it("never makes a stroke longer than the engine takes", () => {
    const many: [number, number][] = Array.from({ length: MAX_STROKE_POINTS * 3 }, (_, i) => [i, i % 7]);
    const out = simplifyStroke(many, 0);
    expect(out.length).toBeLessThanOrEqual(MAX_STROKE_POINTS);
    expect(out[0]).toEqual(many[0]);
    expect(out.at(-1)).toEqual(many.at(-1));
  });
});

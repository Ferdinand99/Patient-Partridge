import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  DEFAULT_WIDTH,
  MAX_IMAGE_PIXELS,
  MAX_IMAGE_SIDE,
  MAX_NAME_CHARS,
  MAX_POINTS,
  MAX_STROKES,
  MAX_TEXT_CHARS,
  MIN_SIDE,
  PICTURE_WORDS,
  SIGNATURE_FONTS,
  aspectOf,
  base64Bytes,
  bodyOf,
  canBeginStroke,
  chaikin,
  cleanName,
  cleanTypedText,
  drawnPicture,
  fitImageSize,
  fromBase64,
  hasInk,
  pictureOfSaved,
  pictureWords,
  sameRect,
  scaledRect,
  startingRect,
  strokeTotals,
  thin,
  toBase64,
  typedOk,
  whiteToTransparent,
  widenedRect,
  type Point,
  type Rect,
  type SavedPicture,
} from "./signpicture";

const PAGE = { width: 612, height: 792 };

describe("a typed name", () => {
  it("is one line without control or direction characters, cut to the cap in whole characters", () => {
    expect(cleanTypedText("  Anna \t  Alvorlig\n")).toBe("Anna Alvorlig");
    expect(cleanTypedText("A\u0000b‮c​d e⁦f﻿g")).toBe("A b c d e f g");
    expect(cleanTypedText("a".repeat(100))).toBe("a".repeat(MAX_TEXT_CHARS));
    // A character outside the plane of the first 65 536 counts as one, not two.
    const cut = cleanTypedText("\u{1F600}".repeat(60));
    expect(Array.from(cut)).toHaveLength(MAX_TEXT_CHARS);
    expect(cut.includes("�")).toBe(false);
  });

  it("can be set when it has something that is not a space", () => {
    expect(typedOk("Anna")).toBe(true);
    expect(typedOk("")).toBe(false);
    expect(typedOk("   \t ")).toBe(false);
    expect(typedOk("​‮")).toBe(false);
  });

  it("has a name for a kept signature that is one line within its cap", () => {
    expect(cleanName("  My   name ")).toBe("My   name");
    expect(cleanName("two\nlines")).toBe("two lines");
    expect(cleanName("x".repeat(MAX_NAME_CHARS + 20))).toHaveLength(MAX_NAME_CHARS);
    expect(cleanName(" \n ")).toBe("");
  });
});

describe("drawing", () => {
  const line = (n: number): Point[] => Array.from({ length: n }, (_, i) => [10 + i * 3, 40 + Math.sin(i / 4) * 20] as Point);

  it("makes a box that is cut to the lines with half the pen around them", () => {
    const p = drawnPicture([[[100, 50], [200, 150]]], 4);
    expect(p).not.toBeNull();
    // 100 wide and 100 high, and the pen: half the line and one unit on each side.
    expect(p?.width).toBeCloseTo(100 + 2 * (2 + 1), 1);
    expect(p?.height).toBeCloseTo(100 + 2 * (2 + 1), 1);
    expect(p?.line).toBe(4);
    const all = p?.strokes.flat() ?? [];
    expect(Math.min(...all.map((q) => q[0]))).toBeCloseTo(3, 1);
    expect(Math.min(...all.map((q) => q[1]))).toBeCloseTo(3, 1);
    expect(all.every((q) => q[0] >= 0 && q[1] >= 0 && q[0] <= (p?.width ?? 0) && q[1] <= (p?.height ?? 0))).toBe(true);
  });

  it("keeps the ends of a line and draws a dot as a stroke of one point", () => {
    const p = drawnPicture([line(30), [[50, 50]]], 3);
    expect(p?.strokes).toHaveLength(2);
    expect(p?.strokes[1]).toHaveLength(1);
    const first = p?.strokes[0] ?? [];
    expect(first.length).toBeGreaterThan(30);
  });

  it("is nothing when nothing was drawn", () => {
    expect(drawnPicture([], 3)).toBeNull();
    expect(drawnPicture([[]], 3)).toBeNull();
  });

  it("holds the picture to the cap of strokes", () => {
    const many = Array.from({ length: MAX_STROKES + 30 }, (_, i) => [[i * 5, 0]] as Point[]);
    const p = drawnPicture(many, 3);
    expect(p?.strokes).toHaveLength(MAX_STROKES);
  });

  it("holds the picture to the cap of points, however long the lines are", () => {
    // Zigzags, so that no point is dropped for being close to the last: 120 000 points to cut down.
    const zigzag = (n: number): Point[] => Array.from({ length: n }, (_, i) => [i * 0.03, (i % 2) * 3] as Point);
    const long = [zigzag(40_000), zigzag(40_000), zigzag(40_000)];
    const p = drawnPicture(long, 3);
    expect(p).not.toBeNull();
    expect(strokeTotals(p?.strokes ?? []).points).toBeLessThanOrEqual(MAX_POINTS);
    expect(strokeTotals(p?.strokes ?? []).strokes).toBeLessThanOrEqual(MAX_STROKES);
    // Every stroke still starts and ends where it did.
    for (const s of p?.strokes ?? []) expect(s.length).toBeGreaterThan(0);
  });

  it("refuses a point that is not a number and a box over the largest", () => {
    expect(drawnPicture([[[Number.NaN, 1]]], 3)).toBeNull();
    expect(drawnPicture([[[0, 0], [5_000, 10]]], 3)).toBeNull();
  });

  it("keeps the thickness of the pen within its range", () => {
    expect(drawnPicture([[[0, 0], [10, 10]]], 0)?.line).toBeCloseTo(0.1, 5);
    expect(drawnPicture([[[0, 0], [10, 10]]], 9_999)?.line).toBe(256);
    expect(drawnPicture([[[0, 0], [10, 10]]], 256)?.line).toBe(256);
  });

  it("allows another stroke only below both caps", () => {
    expect(canBeginStroke([])).toBe(true);
    expect(canBeginStroke(Array.from({ length: MAX_STROKES - 1 }, () => [[0, 0]] as Point[]))).toBe(true);
    expect(canBeginStroke(Array.from({ length: MAX_STROKES }, () => [[0, 0]] as Point[]))).toBe(false);
    expect(canBeginStroke([line(MAX_POINTS)])).toBe(false);
    expect(canBeginStroke([line(MAX_POINTS - 1)])).toBe(true);
  });

  it("rounds corners by cutting them, and thins points that are close", () => {
    const cut = chaikin([
      [0, 0],
      [10, 0],
      [10, 10],
    ]);
    expect(cut).toHaveLength(6);
    expect(cut[0]).toEqual([0, 0]);
    expect(cut.at(-1)).toEqual([10, 10]);
    expect(thin([[0, 0], [0.1, 0], [0.2, 0], [5, 0], [5.1, 0]], 1)).toEqual([[0, 0], [5, 0], [5.1, 0]]);
    expect(chaikin([[1, 1]])).toEqual([[1, 1]]);
  });
});

describe("an imported image", () => {
  it("is scaled to fit the side and the pixel caps, with its proportions", () => {
    expect(fitImageSize(200, 100)).toEqual({ width: 200, height: 100 });
    for (const [w, h] of [
      [4_000, 3_000],
      [10_000, 100],
      [100, 10_000],
      [1_024, 1_024],
      [5_000, 5_000],
      [1_025, 513],
    ]) {
      const s = fitImageSize(w, h);
      expect(s, `${w}x${h}`).not.toBeNull();
      expect(s!.width).toBeLessThanOrEqual(MAX_IMAGE_SIDE);
      expect(s!.height).toBeLessThanOrEqual(MAX_IMAGE_SIDE);
      expect(s!.width * s!.height).toBeLessThanOrEqual(MAX_IMAGE_PIXELS);
      expect(s!.width).toBeGreaterThanOrEqual(1);
      expect(s!.height).toBeGreaterThanOrEqual(1);
    }
    const s = fitImageSize(4_000, 3_000)!;
    expect(s.width / s.height).toBeCloseTo(4 / 3, 1);
  });

  it("is refused when it has no size", () => {
    expect(fitImageSize(0, 10)).toBeNull();
    expect(fitImageSize(10, -1)).toBeNull();
    expect(fitImageSize(Number.NaN, 10)).toBeNull();
    expect(fitImageSize(Infinity, 10)).toBeNull();
  });

  it("can have its white made see-through without changing the picture it was made from", () => {
    const px = new Uint8ClampedArray([255, 255, 255, 255, 20, 20, 20, 255, 240, 250, 255, 255, 200, 255, 255, 255]);
    const copy = new Uint8ClampedArray(px);
    const out = whiteToTransparent(px, 235);
    expect(Array.from(out)).toEqual([255, 255, 255, 0, 20, 20, 20, 255, 240, 250, 255, 0, 200, 255, 255, 255]);
    expect(Array.from(px)).toEqual(Array.from(copy));
  });

  it("is seen only when some pixel can be seen", () => {
    expect(hasInk(new Uint8ClampedArray([0, 0, 0, 0, 1, 2, 3, 4]))).toBe(false);
    expect(hasInk(new Uint8ClampedArray([0, 0, 0, 0, 1, 2, 3, 255]))).toBe(true);
  });
});

describe("the box on the page", () => {
  it("starts at the width of a signature, around the point, kept on the page", () => {
    const r = startingRect(4, [300, 400], PAGE);
    expect(r[2] - r[0]).toBe(DEFAULT_WIDTH);
    expect(r[3] - r[1]).toBeCloseTo(DEFAULT_WIDTH / 4, 5);
    expect((r[0] + r[2]) / 2).toBeCloseTo(300, 5);
    const corner = startingRect(4, [610, 790], PAGE);
    expect(corner[2]).toBeLessThanOrEqual(612);
    expect(corner[3]).toBeLessThanOrEqual(792);
    const top = startingRect(4, [0, 0], PAGE);
    expect(top[0]).toBe(0);
    expect(top[1]).toBe(0);
  });

  it("is never more than half the page, also for a tall picture", () => {
    const small = startingRect(2, [50, 50], { width: 100, height: 100 });
    expect(small[2] - small[0]).toBeLessThanOrEqual(50);
    const tall = startingRect(0.1, [300, 400], PAGE);
    expect(tall[3] - tall[1]).toBeLessThanOrEqual(396);
    expect((tall[2] - tall[0]) / (tall[3] - tall[1])).toBeCloseTo(0.1, 5);
  });

  it("is made wider with its corner and proportions kept", () => {
    const r: Rect = [100, 200, 260, 240];
    const w = widenedRect(r, 80, PAGE);
    expect(w[0]).toBe(100);
    expect(w[1]).toBe(200);
    expect(w[2] - w[0]).toBe(240);
    expect((w[2] - w[0]) / (w[3] - w[1])).toBeCloseTo(4, 5);
    const n = widenedRect(r, -150, PAGE);
    expect(n[3] - n[1]).toBeGreaterThanOrEqual(MIN_SIDE - 0.001);
    expect(n[2] - n[0]).toBeGreaterThanOrEqual(MIN_SIDE - 0.001);
  });

  it("is kept inside the page when made wider", () => {
    const r: Rect = [500, 700, 580, 720];
    const w = widenedRect(r, 5_000, PAGE);
    expect(w[2]).toBeLessThanOrEqual(612);
    expect(w[3]).toBeLessThanOrEqual(792);
    expect((w[2] - w[0]) / (w[3] - w[1])).toBeCloseTo(4, 5);
  });

  it("is scaled around its corner", () => {
    const r: Rect = [100, 200, 260, 240];
    const s = scaledRect(r, 1.5, PAGE);
    expect(s[2] - s[0]).toBeCloseTo(240, 5);
    expect(sameRect(scaledRect(r, 1, PAGE), r)).toBe(true);
    expect(sameRect(r, [100, 200, 260, 240.5])).toBe(false);
  });

  it("has proportions of its own, and a typed name has the measured ones", () => {
    expect(aspectOf({ kind: "draw", width: 300, height: 100, line: 3, strokes: [] })).toBe(3);
    expect(aspectOf({ kind: "image", width: 200, height: 50, rgba: new Uint8ClampedArray(0) })).toBe(4);
    expect(aspectOf({ kind: "type", font: "allura", text: "Anna" }, 5.5)).toBe(5.5);
    expect(aspectOf({ kind: "type", font: "allura", text: "Anna" })).toBeGreaterThan(0);
    expect(aspectOf({ kind: "draw", width: 0, height: 0, line: 3, strokes: [] })).toBeNull();
  });
});

describe("keeping and sending", () => {
  it("sends the lines of a drawing and the name of a font, nothing else", () => {
    const draw = { kind: "draw", width: 10, height: 5, line: 2, strokes: [[[1, 1]]] } as const;
    expect(bodyOf({ ...draw, strokes: [[[1, 1]]] })).toEqual({ kind: "draw", width: 10, height: 5, line: 2, strokes: [[[1, 1]]] });
    const typed = bodyOf({ kind: "type", font: "kristi", text: "Anna" });
    expect(typed).toEqual({ kind: "type", font: "kristi", text: "Anna" });
  });

  it("knows a kept drawing and a kept name, and not a font that is not bundled", () => {
    const base = { id: "a", name: "n", created: 1 };
    const d: SavedPicture = { ...base, kind: "draw", width: 10, height: 5, line: 2, strokes: [[[1, 1]]] };
    expect(pictureOfSaved(d)?.kind).toBe("draw");
    expect(pictureOfSaved({ ...base, kind: "type", font: "allura", text: "A" })?.kind).toBe("type");
    expect(pictureOfSaved({ ...base, kind: "type", font: "papyrus", text: "A" })).toBeNull();
    expect(pictureOfSaved({ ...base, kind: "image", width: 1, height: 1, png: "iVBORw0KGg" })).toBeNull();
  });

  it("turns bytes into text and back, also for a long array", () => {
    const bytes = new Uint8Array(100_000).map((_, i) => (i * 31) % 256);
    const text = toBase64(bytes);
    expect(Array.from(fromBase64(text))).toEqual(Array.from(bytes));
    expect(base64Bytes(text)).toBe(bytes.length);
    expect(() => fromBase64("not base64!")).toThrow();
  });

  it("offers three fonts, each with a name of its own", () => {
    expect(SIGNATURE_FONTS.map((f) => f.id)).toEqual(["great-vibes", "allura", "kristi"]);
    expect(new Set(SIGNATURE_FONTS.map((f) => f.family)).size).toBe(3);
  });
});

describe("what the window says", () => {
  const FORBIDDEN =
    new RegExp("\\b(legal|legally|court|guarantee|guaranteed|valid|validity|binding|qualified|advanced|eidas|compliant|certified|certificate|" + "ad" + "obe|acro" + "bat|proof|prove|proves|authentic|authenticity|notarised|notarized)\\b", "i");

  it("says that a picture is not a digital signature and does not show who placed it", () => {
    expect(PICTURE_WORDS.notice).toContain("A picture is not a digital signature");
    expect(PICTURE_WORDS.notice).toContain("does not show who placed it");
    expect(PICTURE_WORDS.notice).toContain("anyone can copy it");
    expect(PICTURE_WORDS.saved).toContain("this computer only");
    expect(PICTURE_WORDS.saved).toContain("Anyone who can read that folder can use them");
  });

  it("makes no promise of validity, in any text", () => {
    for (const text of pictureWords()) expect(text, text).not.toMatch(FORBIDDEN);
  });

  it("makes no promise of validity in the code that shows the box either", () => {
    for (const file of ["src/lib/signpicture.ts", "src/app/signpicture.ts", "src/app/annotator.ts"]) {
      const source = readFileSync(file, "utf8");
      // Strings and comments alike: the words that must not be said are not said anywhere.
      expect(source, file).not.toMatch(new RegExp("\\b(legal|legally|court|guarantee|guaranteed|eidas|compliant|certified|" + "ad" + "obe|acro" + "bat)\\b", "i"));
    }
  });
});

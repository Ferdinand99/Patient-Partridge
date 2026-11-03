import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  DEFAULT_WIDTH,
  HEADER_BYTES,
  MAX_DECODE_SIDE,
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
  checkImageHeader,
  chaikin,
  cleanName,
  cleanTypedText,
  drawnPicture,
  fitImageSize,
  fromBase64,
  hasInk,
  imageSizeOf,
  judgeImageSize,
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

describe("the size an image file declares", () => {
  const ascii = (text: string) => Array.from(text, (c) => c.charCodeAt(0));
  const be32 = (n: number) => [(n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255];
  const le16 = (n: number) => [n & 255, (n >> 8) & 255];
  const le32 = (n: number) => [n & 255, (n >>> 8) & 255, (n >>> 16) & 255, (n >>> 24) & 255];
  const le24 = (n: number) => [n & 255, (n >> 8) & 255, (n >> 16) & 255];

  const png = (w: number, h: number) =>
    new Uint8Array([0x89, ...ascii("PNG"), 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, ...ascii("IHDR"), ...be32(w), ...be32(h), 8, 6, 0, 0, 0]);
  // A GIF with a screen, a global colour table of two colours, and frames (each given by its size
  // and, if it has them, its place). Each frame has an extension before it, a colour table of its
  // own when asked, and its data in two sub-blocks.
  const gifFrame = (w: number, h: number, left = 0, top = 0, ownColours = false) => [
    0x21, 0xf9, 4, 0, 0, 0, 0, 0,
    0x2c, ...le16(left), ...le16(top), ...le16(w), ...le16(h), ownColours ? 0x80 : 0,
    ...(ownColours ? [0, 0, 0, 255, 255, 255] : []),
    2, 3, 1, 2, 3, 2, 4, 5, 0,
  ];
  const gifBytes = (sw: number, sh: number, frames: number[][], trailer = true) =>
    new Uint8Array([...ascii("GIF89a"), ...le16(sw), ...le16(sh), 0x80, 0, 0, 0, 0, 0, 255, 255, 255, ...frames.flat(), ...(trailer ? [0x3b] : [])]);
  const gif = (w: number, h: number) => gifBytes(w, h, [gifFrame(w, h)]);
  const bmp = (w: number, h: number) => new Uint8Array([...ascii("BM"), ...new Array(12).fill(0), ...le32(40), ...le32(w), ...le32(h >>> 0), 1, 0, 24, 0]);
  const bmpCore = (w: number, h: number) => new Uint8Array([...ascii("BM"), ...new Array(12).fill(0), ...le32(12), ...le16(w), ...le16(h), 1, 0, 24, 0]);
  const webpLossy = (w: number, h: number) =>
    new Uint8Array([...ascii("RIFF"), 0, 0, 0, 0, ...ascii("WEBPVP8 "), 0, 0, 0, 0, 0, 0, 0, 0x9d, 0x01, 0x2a, ...le16(w), ...le16(h)]);
  const webpLossless = (w: number, h: number) => {
    const bits = (w - 1) | ((h - 1) << 14);
    return new Uint8Array([...ascii("RIFF"), 0, 0, 0, 0, ...ascii("WEBPVP8L"), 0, 0, 0, 0, 0x2f, ...le32(bits >>> 0)]);
  };
  const webpExtended = (w: number, h: number) =>
    new Uint8Array([...ascii("RIFF"), 0, 0, 0, 0, ...ascii("WEBPVP8X"), 10, 0, 0, 0, 0, 0, 0, 0, ...le24(w - 1), ...le24(h - 1)]);
  const jpeg = (w: number, h: number, marker = 0xc0) =>
    new Uint8Array([
      0xff, 0xd8,
      // A segment with some other data first (an EXIF block would be one), then fill bytes, then the frame.
      0xff, 0xe1, 0, 6, 1, 2, 3, 4,
      0xff, 0xff,
      0xff, marker, 0, 17, 8, (h >> 8) & 255, h & 255, (w >> 8) & 255, w & 255, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1,
    ]);

  it("is read from the header of a PNG, a GIF, a BMP, a WebP and a JPEG without decoding anything", () => {
    expect(imageSizeOf(png(640, 480))).toEqual({ width: 640, height: 480 });
    expect(imageSizeOf(png(30_000, 20_000))).toEqual({ width: 30_000, height: 20_000 });
    expect(imageSizeOf(gif(320, 200))).toEqual({ width: 320, height: 200 });
    expect(imageSizeOf(bmp(300, 100))).toEqual({ width: 300, height: 100 });
    // Rows stored from the top say so with a negative height.
    expect(imageSizeOf(bmp(300, -100))).toEqual({ width: 300, height: 100 });
    expect(imageSizeOf(bmpCore(64, 32))).toEqual({ width: 64, height: 32 });
    expect(imageSizeOf(webpLossy(800, 600))).toEqual({ width: 800, height: 600 });
    expect(imageSizeOf(webpLossless(1234, 567))).toEqual({ width: 1234, height: 567 });
    expect(imageSizeOf(webpExtended(4000, 3000))).toEqual({ width: 4000, height: 3000 });
    expect(imageSizeOf(jpeg(1920, 1080))).toEqual({ width: 1920, height: 1080 });
    // A progressive frame is a frame too; the tables (0xC4) are not.
    expect(imageSizeOf(jpeg(50, 60, 0xc2))).toEqual({ width: 50, height: 60 });
    expect(imageSizeOf(jpeg(50, 60, 0xc4))).toBeNull();
  });

  it("is nothing for a header that is cut short, wrong or from another kind of file", () => {
    for (const bytes of [
      new Uint8Array(0),
      new Uint8Array(40),
      png(10, 10).subarray(0, 20),
      gif(10, 10).subarray(0, 8),
      bmp(10, 10).subarray(0, 20),
      // A GIF cut short: in the screen, in the colour table, in a frame, in its data, or before the trailer.
      gif(10, 10).subarray(0, 14),
      gif(10, 10).subarray(0, 25),
      gif(10, 10).subarray(0, 36),
      gif(10, 10).subarray(0, gif(10, 10).length - 3),
      gif(10, 10).subarray(0, gif(10, 10).length - 1),
      // A GIF with no frame, or with a block that is none.
      gifBytes(10, 10, []),
      gifBytes(10, 10, [[0x55, 0, 0]]),
      // A screen of nothing and no frame that has a size.
      gifBytes(0, 0, [gifFrame(0, 0)]),
      // A second frame that is cut short is not passed over for the first.
      gifBytes(10, 10, [gifFrame(10, 10), gifFrame(10, 10).slice(0, 12)], false),
      webpLossy(10, 10).subarray(0, 28),
      jpeg(10, 10).subarray(0, 14),
      new Uint8Array(ascii("<svg xmlns='http://www.w3.org/2000/svg' width='10' height='10'/>")),
      png(0, 10),
      gif(10, 0),
    ]) {
      expect(imageSizeOf(bytes)).toBeNull();
    }
    // A PNG whose first chunk is not the header chunk.
    const odd = png(10, 10);
    odd.set(ascii("IDAT"), 12);
    expect(imageSizeOf(odd)).toBeNull();
    // A JPEG that reaches the data before any frame.
    expect(imageSizeOf(new Uint8Array([0xff, 0xd8, 0xff, 0xda, 0, 2, 0, 0]))).toBeNull();
  });

  it("is the largest of the screen and every frame of a GIF", () => {
    // A screen of 1 by 1 and a frame of 16000 by 16000: what a decoder makes is the frame.
    const bomb = gifBytes(1, 1, [gifFrame(16_000, 16_000)]);
    expect(imageSizeOf(bomb)).toEqual({ width: 16_000, height: 16_000 });
    expect(judgeImageSize(imageSizeOf(bomb))).toBe("too-big");
    // A later frame counts as much as the first, and a frame is counted with its place.
    expect(imageSizeOf(gifBytes(10, 10, [gifFrame(10, 10), gifFrame(9_000, 5, 0, 0, true)]))).toEqual({ width: 9_000, height: 10 });
    expect(imageSizeOf(gifBytes(10, 10, [gifFrame(10, 10, 8_000, 8_000)]))).toEqual({ width: 8_010, height: 8_010 });
    expect(judgeImageSize(imageSizeOf(gifBytes(10, 10, [gifFrame(10, 10), gifFrame(10, 10), gifFrame(8_193, 1)])))).toBe("too-big");
    // A frame that is too big is not hidden behind frames that are small: many of them, and extensions.
    const many = Array.from({ length: 200 }, () => gifFrame(20, 20, 0, 0, true));
    expect(imageSizeOf(gifBytes(20, 20, many))).toEqual({ width: 20, height: 20 });
    expect(judgeImageSize(imageSizeOf(gifBytes(20, 20, [...many, gifFrame(1, 20_000)])))).toBe("too-big");
    // A normal GIF is read.
    expect(judgeImageSize(imageSizeOf(gif(640, 480)))).toBe("ok");
    expect(imageSizeOf(gifBytes(300, 200, [gifFrame(300, 200), gifFrame(100, 100, 10, 10)]))).toEqual({ width: 300, height: 200 });
  });

  it("is judged against the side and the pixel limit of the decoder", () => {
    expect(judgeImageSize({ width: 1, height: 1 })).toBe("ok");
    expect(judgeImageSize({ width: MAX_DECODE_SIDE, height: 4_000 })).toBe("ok");
    expect(judgeImageSize({ width: MAX_DECODE_SIDE + 1, height: 1 })).toBe("too-big");
    expect(judgeImageSize({ width: 1, height: MAX_DECODE_SIDE + 1 })).toBe("too-big");
    expect(judgeImageSize({ width: 7_000, height: 7_000 })).toBe("too-big");
    expect(judgeImageSize({ width: 30_000, height: 30_000 })).toBe("too-big");
    expect(judgeImageSize(null)).toBe("unreadable");
  });

  it("is checked on a file before the decoder is asked, and the words name the formats the box takes", async () => {
    expect(await checkImageHeader(new Blob([png(100, 50)]))).toBe("ok");
    expect(await checkImageHeader(new Blob([png(100_000, 100_000)]))).toBe("too-big");
    expect(await checkImageHeader(new Blob(["not a picture"]))).toBe("unreadable");
    // A GIF is judged by every frame, also past the first bytes of the file.
    expect(await checkImageHeader(new Blob([gif(100, 50)]))).toBe("ok");
    expect(await checkImageHeader(new Blob([gifBytes(1, 1, [gifFrame(16_000, 16_000)])]))).toBe("too-big");
    expect(await checkImageHeader(new Blob([gif(100, 50).subarray(0, 30)]))).toBe("unreadable");
    // The frame that is too big comes after more than the first bytes that are read for a header.
    const filler = [0x2c, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, ...Array.from({ length: 9_000 }, () => [255, ...new Array(255).fill(0)]).flat(), 0];
    const late = gifBytes(1, 1, [filler, gifFrame(16_000, 16_000)]);
    expect(late.length).toBeGreaterThan(HEADER_BYTES);
    expect(await checkImageHeader(new Blob([late]))).toBe("too-big");
    expect(await checkImageHeader(new Blob([gifBytes(1, 1, [filler, gifFrame(1, 1)])]))).toBe("ok");
    for (const format of ["PNG", "JPEG", "WebP", "GIF", "BMP"]) expect(PICTURE_WORDS.imageFailed).toContain(format);
  });
});

import { describe, expect, it } from "vitest";
import { MARGIN } from "./layout";
import { MAX_ZOOM, MIN_ZOOM, clampZoom, fitWidthZoom, stepZoom, zoomLabel } from "./zoom";

describe("stepZoom", () => {
  it("moves to the next step", () => {
    expect(stepZoom(1, 1)).toBe(1.25);
    expect(stepZoom(1, -1)).toBe(0.75);
  });

  it("steps from a value between two steps", () => {
    expect(stepZoom(0.9, 1)).toBe(1);
    expect(stepZoom(0.9, -1)).toBe(0.75);
  });

  it("stops at the ends", () => {
    expect(stepZoom(MAX_ZOOM, 1)).toBe(MAX_ZOOM);
    expect(stepZoom(MIN_ZOOM, -1)).toBe(MIN_ZOOM);
  });
});

describe("clampZoom", () => {
  it("keeps zoom in range and survives nonsense", () => {
    expect(clampZoom(100)).toBe(MAX_ZOOM);
    expect(clampZoom(0)).toBe(MIN_ZOOM);
    expect(clampZoom(NaN)).toBe(1);
    expect(clampZoom(1.5)).toBe(1.5);
  });
});

describe("fitWidthZoom", () => {
  it("makes the widest page fill the viewport less the margins", () => {
    // A Letter page is 816 css pixels wide at 100 %.
    const viewport = 816 + 2 * MARGIN;
    expect(fitWidthZoom(612, viewport)).toBeCloseTo(1, 5);
    expect(fitWidthZoom(612, 408 + 2 * MARGIN)).toBeCloseTo(0.5, 5);
  });

  it("falls back to 100 % when nothing is known yet", () => {
    expect(fitWidthZoom(0, 800)).toBe(1);
    expect(fitWidthZoom(612, 0)).toBe(1);
  });

  it("stays within the zoom limits", () => {
    expect(fitWidthZoom(10, 5000)).toBe(MAX_ZOOM);
    expect(fitWidthZoom(5000, 100)).toBe(MIN_ZOOM);
  });
});

describe("zoomLabel", () => {
  it("shows a percentage", () => {
    expect(zoomLabel(1.25)).toBe("125%");
    expect(zoomLabel(0.333)).toBe("33%");
  });
});

// Page layout for the continuous view. Pure functions: no DOM, so they are easy to test.
//
// Units: page sizes come in PDF points (1/72 inch). The scale turns points into CSS pixels:
// at 100 % zoom one point is 96/72 CSS pixels.

export interface PageSizePt {
  width: number;
  height: number;
}

/** Space between two pages, in CSS pixels. */
export const PAGE_GAP = 12;
/** Space around the whole stack of pages, in CSS pixels. */
export const MARGIN = 16;

export interface Layout {
  /** Top edge of each page, in CSS pixels from the top of the scrolled content. */
  tops: number[];
  widths: number[];
  heights: number[];
  /** Width of the widest page plus margins. */
  contentWidth: number;
  totalHeight: number;
}

/** CSS pixels per PDF point at the given zoom (1 = 100 %). */
export function cssPerPoint(zoom: number): number {
  return (zoom * 96) / 72;
}

export function computeLayout(pages: PageSizePt[], zoom: number): Layout {
  const k = cssPerPoint(zoom);
  const tops: number[] = [];
  const widths: number[] = [];
  const heights: number[] = [];
  let y = MARGIN;
  let widest = 0;
  for (const page of pages) {
    const w = Math.max(1, Math.round(page.width * k));
    const h = Math.max(1, Math.round(page.height * k));
    tops.push(y);
    widths.push(w);
    heights.push(h);
    widest = Math.max(widest, w);
    y += h + PAGE_GAP;
  }
  const totalHeight = pages.length === 0 ? 2 * MARGIN : y - PAGE_GAP + MARGIN;
  return { tops, widths, heights, contentWidth: widest + 2 * MARGIN, totalHeight };
}

/** Index of the last page whose top edge is at or above `y`, or -1 when `y` is above all pages. */
function lastTopAtOrBefore(layout: Layout, y: number): number {
  let lo = 0;
  let hi = layout.tops.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (layout.tops[mid] <= y) {
      found = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return found;
}

export interface Range {
  first: number;
  last: number;
}

/**
 * The pages that overlap the band from `top` to `bottom` (CSS pixels of content), or null when
 * there are none.
 */
export function pagesInBand(layout: Layout, top: number, bottom: number): Range | null {
  const count = layout.tops.length;
  if (count === 0 || bottom <= top) return null;
  let first = lastTopAtOrBefore(layout, top);
  if (first < 0) first = 0;
  // The page found may end above the band: then the next one is the first that overlaps.
  if (layout.tops[first] + layout.heights[first] <= top) first += 1;
  const last = lastTopAtOrBefore(layout, bottom - 1);
  if (first >= count || last < first) return null;
  return { first, last };
}

/** Pages visible in the viewport, widened by `overscan` CSS pixels above and below. */
export function visibleRange(
  layout: Layout,
  scrollTop: number,
  viewportHeight: number,
  overscan = 0,
): Range | null {
  return pagesInBand(layout, scrollTop - overscan, scrollTop + viewportHeight + overscan);
}

/** The page that fills most of the viewport; ties go to the earlier page. */
export function currentPage(layout: Layout, scrollTop: number, viewportHeight: number): number {
  const range = visibleRange(layout, scrollTop, viewportHeight);
  if (!range) return 0;
  let best = range.first;
  let bestArea = -1;
  for (let i = range.first; i <= range.last; i++) {
    const top = Math.max(layout.tops[i], scrollTop);
    const bottom = Math.min(layout.tops[i] + layout.heights[i], scrollTop + viewportHeight);
    const area = bottom - top;
    if (area > bestArea) {
      best = i;
      bestArea = area;
    }
  }
  return best;
}

/** Scroll position that puts the top of page `index` just below the top edge. */
export function scrollTopForPage(layout: Layout, index: number): number {
  if (layout.tops.length === 0) return 0;
  const i = Math.min(Math.max(index, 0), layout.tops.length - 1);
  return Math.max(0, layout.tops[i] - PAGE_GAP);
}

/** A reading position that survives a change of zoom: a page and how far down it we are. */
export interface Anchor {
  page: number;
  /** 0 at the top of the page, 1 at the bottom. May be negative in the gap above the page. */
  fraction: number;
}

export function anchorAt(layout: Layout, scrollTop: number): Anchor {
  const count = layout.tops.length;
  if (count === 0) return { page: 0, fraction: 0 };
  const page = Math.max(0, lastTopAtOrBefore(layout, scrollTop));
  return { page, fraction: (scrollTop - layout.tops[page]) / layout.heights[page] };
}

export function scrollTopForAnchor(layout: Layout, anchor: Anchor): number {
  if (layout.tops.length === 0) return 0;
  const page = Math.min(anchor.page, layout.tops.length - 1);
  return Math.max(0, Math.round(layout.tops[page] + anchor.fraction * layout.heights[page]));
}

/** Limit on the size of one rendered bitmap, in pixels. Keeps memory in check at high zoom. */
export const MAX_BITMAP_PIXELS = 16_000_000;
/** Limit on one side of a rendered bitmap. The engine refuses anything longer (16384). */
export const MAX_BITMAP_SIDE = 16_384;

export interface BitmapSize {
  width: number;
  height: number;
}

/**
 * Size in device pixels of the bitmap to ask the engine for. The bitmap is sharp on screens with
 * a pixel ratio above 1, and never bigger than `MAX_BITMAP_PIXELS` in area or `MAX_BITMAP_SIDE`
 * on a side (a narrow, tall page can pass the side limit long before the area limit).
 */
export function bitmapSize(
  page: PageSizePt,
  zoom: number,
  devicePixelRatio: number,
  maxPixels = MAX_BITMAP_PIXELS,
): BitmapSize {
  const k = cssPerPoint(zoom) * Math.max(1, devicePixelRatio);
  let width = Math.max(1, Math.round(page.width * k));
  let height = Math.max(1, Math.round(page.height * k));
  if (width * height > maxPixels) {
    const shrink = Math.sqrt(maxPixels / (width * height));
    width = Math.max(1, Math.floor(width * shrink));
    height = Math.max(1, Math.floor(height * shrink));
  }
  const longest = Math.max(width, height);
  if (longest > MAX_BITMAP_SIDE) {
    const shrink = MAX_BITMAP_SIDE / longest;
    width = Math.max(1, Math.floor(width * shrink));
    height = Math.max(1, Math.floor(height * shrink));
  }
  return { width, height };
}

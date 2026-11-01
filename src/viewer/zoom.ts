// Zoom levels and the "fit width" mode.

import { MARGIN, cssPerPoint } from "./layout";

export const MIN_ZOOM = 0.1;
export const MAX_ZOOM = 8;

/** Steps for the zoom in and zoom out commands. */
export const ZOOM_STEPS = [0.1, 0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8];

export type ZoomMode = { kind: "fit-width" } | { kind: "custom"; zoom: number };

export function clampZoom(zoom: number): number {
  if (!Number.isFinite(zoom)) return 1;
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, zoom));
}

/** The next step above (direction 1) or below (direction -1) the current zoom. */
export function stepZoom(current: number, direction: 1 | -1): number {
  const eps = 1e-6;
  if (direction === 1) {
    const next = ZOOM_STEPS.find((s) => s > current + eps);
    return next ?? MAX_ZOOM;
  }
  for (let i = ZOOM_STEPS.length - 1; i >= 0; i--) {
    if (ZOOM_STEPS[i] < current - eps) return ZOOM_STEPS[i];
  }
  return MIN_ZOOM;
}

/**
 * The zoom at which the widest page exactly fills the viewport width, less the margins.
 * `viewportWidth` is the width available for pages, in CSS pixels.
 */
export function fitWidthZoom(widestPagePt: number, viewportWidth: number): number {
  if (!(widestPagePt > 0) || !(viewportWidth > 0)) return 1;
  const available = Math.max(1, viewportWidth - 2 * MARGIN);
  return clampZoom(available / (widestPagePt * cssPerPoint(1)));
}

/** "125 %" style label. */
export function zoomLabel(zoom: number): string {
  return `${Math.round(zoom * 100)}%`;
}

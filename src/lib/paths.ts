/** Most files opened at once from one drop or one launch. The rest is ignored. */
export const MAX_OPEN_AT_ONCE = 8;

/**
 * Every path to open from a drop or a launch, without repeats. Files that look like PDFs come
 * first and are all taken; when none looks like a PDF, the first path is tried (a PDF does not
 * have to have the right extension). This is the whole list; `pickPdfPaths` cuts it.
 */
export function wantedPdfPaths(paths: string[]): string[] {
  const real = [...new Set(paths.filter((p) => p.length > 0))];
  const pdfs = real.filter((p) => /\.pdf$/i.test(p));
  return pdfs.length > 0 ? pdfs : real.slice(0, 1);
}

/** The paths to open: `wantedPdfPaths`, cut to `MAX_OPEN_AT_ONCE`. */
export function pickPdfPaths(paths: string[]): string[] {
  return wantedPdfPaths(paths).slice(0, MAX_OPEN_AT_ONCE);
}

/** Most documents the app has open at once (the same limit as the app process). */
export const MAX_TABS = 16;

/** The tabs of the last run, as the app process remembered them. */
export interface SessionTabs {
  paths: string[];
  /** Index into `paths` of the tab that was in view, or null. */
  active: number | null;
}

/**
 * The tabs to reopen at the start: the saved paths without empty ones and repeats, cut to `room`
 * (and never more than `MAX_TABS`). When some must go, the tab in view stays and the last of the
 * others are left out. `active` is the index of the tab in view in `paths` (the first one when the
 * saved index points nowhere), and `left` counts the paths that were cut.
 */
export function pickSessionTabs(
  saved: SessionTabs | null | undefined,
  room: number,
): { paths: string[]; active: number; left: number } {
  const wanted = saved && saved.active !== null ? saved.paths[saved.active] : undefined;
  const all = [...new Set((saved?.paths ?? []).filter((p) => typeof p === "string" && p.length > 0))];
  const limit = Math.max(0, Math.min(room, MAX_TABS));
  const activeAt = wanted === undefined ? 0 : Math.max(0, all.indexOf(wanted));
  let paths = all;
  if (all.length > limit) {
    let others = limit - (limit > 0 ? 1 : 0);
    paths = all.filter((_, i) => (i === activeAt && limit > 0) || (i !== activeAt && others-- > 0));
  }
  return { paths, active: Math.max(0, paths.indexOf(all[activeAt])), left: all.length - paths.length };
}

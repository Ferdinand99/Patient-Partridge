/**
 * Chooses which of the dropped or passed paths to open: the first one that looks like a PDF,
 * else the first one (a PDF does not have to have the right extension).
 */
export function pickPdfPath(paths: string[]): string | null {
  const real = paths.filter((p) => p.length > 0);
  if (real.length === 0) return null;
  return real.find((p) => /\.pdf$/i.test(p)) ?? real[0];
}

/** Most files opened at once from one drop or one launch. The rest is ignored. */
export const MAX_OPEN_AT_ONCE = 8;

/**
 * Every path to open from a drop or a launch, without repeats. Files that look like PDFs come
 * first and are all taken; when none looks like a PDF, the first path is tried (a PDF does not
 * have to have the right extension). The list is cut to `MAX_OPEN_AT_ONCE`.
 */
export function pickPdfPaths(paths: string[]): string[] {
  const real = [...new Set(paths.filter((p) => p.length > 0))];
  const pdfs = real.filter((p) => /\.pdf$/i.test(p));
  const chosen = pdfs.length > 0 ? pdfs : real.slice(0, 1);
  return chosen.slice(0, MAX_OPEN_AT_ONCE);
}

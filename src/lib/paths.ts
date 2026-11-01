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

/**
 * Chooses which of the dropped or passed paths to open: the first one that looks like a PDF,
 * else the first one (a PDF does not have to have the right extension).
 */
export function pickPdfPath(paths: string[]): string | null {
  const real = paths.filter((p) => p.length > 0);
  if (real.length === 0) return null;
  return real.find((p) => /\.pdf$/i.test(p)) ?? real[0];
}

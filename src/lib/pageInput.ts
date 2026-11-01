/**
 * Reads what the user typed in the page number box. Returns the zero-based page index, kept
 * inside the document, or null when the text is not a page number.
 */
export function parsePageInput(text: string, pageCount: number): number | null {
  if (pageCount <= 0) return null;
  const trimmed = text.trim();
  if (!/^\d{1,9}$/.test(trimmed)) return null;
  const number = parseInt(trimmed, 10);
  if (number < 1) return null;
  return Math.min(number, pageCount) - 1;
}

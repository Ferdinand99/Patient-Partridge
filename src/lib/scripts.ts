// Which writing systems a text uses, so the window knows when to ask the engine whether it had to
// refuse a font for them (`missed_fonts` on the Rust side), and the words for the message. Pure
// functions: no DOM.

/** A script as the window tells it apart. "han" is the Chinese characters that Chinese, Japanese
 * and Korean share: any CJK font draws them. */
export type TextScript = "han" | "japanese" | "korean" | "arabic" | "hebrew" | "thai";

function scriptOf(code: number): TextScript | null {
  if ((code >= 0x3040 && code <= 0x30ff) || (code >= 0x31f0 && code <= 0x31ff)) return "japanese";
  if (code >= 0xff66 && code <= 0xff9f) return "japanese";
  if (
    (code >= 0xac00 && code <= 0xd7af) ||
    (code >= 0x1100 && code <= 0x11ff) ||
    (code >= 0x3130 && code <= 0x318f)
  ) {
    return "korean";
  }
  if (
    (code >= 0x4e00 && code <= 0x9fff) ||
    (code >= 0x3400 && code <= 0x4dbf) ||
    (code >= 0xf900 && code <= 0xfaff)
  ) {
    return "han";
  }
  if (
    (code >= 0x0600 && code <= 0x06ff) ||
    (code >= 0x0750 && code <= 0x077f) ||
    (code >= 0xfb50 && code <= 0xfdff) ||
    (code >= 0xfe70 && code <= 0xfeff)
  ) {
    return "arabic";
  }
  if ((code >= 0x0590 && code <= 0x05ff) || (code >= 0xfb1d && code <= 0xfb4f)) return "hebrew";
  if (code >= 0x0e00 && code <= 0x0e7f) return "thai";
  return null;
}

/** The scripts that need a font among the characters of a text. */
export function scriptsIn(text: string): Set<TextScript> {
  const found = new Set<TextScript>();
  for (const ch of text) {
    const script = scriptOf(ch.codePointAt(0) ?? 0);
    if (script) found.add(script);
    if (found.size === 6) break;
  }
  return found;
}

const FONT_LABELS: Record<string, string> = {
  japanese: "Japanese",
  korean: "Korean",
  "chinese-simplified": "Simplified Chinese",
  "chinese-traditional": "Traditional Chinese",
  arabic: "Arabic",
  hebrew: "Hebrew",
  thai: "Thai",
};

/** The scripts the engine reports (its names for them) as words for a message: "Japanese and Thai". */
export function describeFontScripts(names: readonly string[]): string {
  const labels = names.map((n) => FONT_LABELS[n] ?? n);
  if (labels.length <= 1) return labels[0] ?? "";
  return `${labels.slice(0, -1).join(", ")} and ${labels[labels.length - 1]}`;
}

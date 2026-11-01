// Which writing systems a text uses, so the window can say when a document needs a font that this
// computer does not have. Pure functions: no DOM. The engine draws text in a script that the PDF
// does not carry a font for only if the app found a font for that script on the system (see
// `system_fonts` in the Rust side); this module finds out which scripts a page's text needs.

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

/**
 * The scripts among `seen` that this computer has no font for. `fonts` are the script names the app
 * found a system font for ("japanese", "korean", "chinese-simplified", "chinese-traditional",
 * "arabic", "hebrew", "thai"). The shared Chinese characters are covered by any of the four CJK
 * fonts.
 */
export function scriptsWithoutFont(seen: Iterable<TextScript>, fonts: readonly string[]): TextScript[] {
  const has = (name: string): boolean => fonts.includes(name);
  const anyCjk = ["japanese", "korean", "chinese-simplified", "chinese-traditional"].some(has);
  const missing: TextScript[] = [];
  for (const script of seen) {
    const covered = script === "han" ? anyCjk : has(script);
    if (!covered) missing.push(script);
  }
  return missing;
}

const LABELS: Record<TextScript, string> = {
  han: "Chinese characters",
  japanese: "Japanese",
  korean: "Korean",
  arabic: "Arabic",
  hebrew: "Hebrew",
  thai: "Thai",
};

/** The scripts as words for a message: "Japanese and Thai". */
export function describeScripts(scripts: readonly TextScript[]): string {
  const names = scripts.map((s) => LABELS[s]);
  if (names.length <= 1) return names[0] ?? "";
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

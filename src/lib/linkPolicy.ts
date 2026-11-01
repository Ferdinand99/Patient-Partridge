// What may be done with a link from a PDF. A link is never opened by itself: the reader asks the
// user first, and only web addresses (http and https) are ever opened. Everything else is refused
// with a reason. The same rules are checked again on the Rust side (src-tauri/src/links.rs).

/** Longest address accepted, in characters (the engine reports at most 2 048 bytes). */
export const MAX_URI_LENGTH = 2048;

export type LinkVerdict =
  | {
      kind: "web";
      /** The address to ask about and to open: cleaned up by the URL parser. */
      url: string;
      /** The name of the site, with international names in their plain ASCII form. */
      host: string;
    }
  | { kind: "blocked"; reason: string; scheme: string | null };

/** Control characters, spaces and the C1 range: nothing that belongs in an address. */
function hasBadCharacter(text: string): boolean {
  for (const ch of text) {
    const n = ch.codePointAt(0) ?? 0;
    if (n <= 0x20 || (n >= 0x7f && n <= 0x9f)) return true;
  }
  return false;
}

export function classifyUri(uri: string): LinkVerdict {
  if (uri.length > MAX_URI_LENGTH) {
    return { kind: "blocked", reason: "The address is too long.", scheme: null };
  }
  if (hasBadCharacter(uri)) {
    return { kind: "blocked", reason: "The address contains characters that do not belong in one.", scheme: null };
  }
  let url: URL;
  try {
    url = new URL(uri);
  } catch {
    return { kind: "blocked", reason: "This is not a web address.", scheme: null };
  }
  const scheme = url.protocol.replace(/:$/, "").toLowerCase();
  if (scheme !== "http" && scheme !== "https") {
    return {
      kind: "blocked",
      reason: `Addresses that start with “${scheme}:” are never opened. Only web addresses (http and https) are.`,
      scheme,
    };
  }
  if (url.hostname === "") {
    return { kind: "blocked", reason: "The address has no site name.", scheme };
  }
  if (url.username !== "" || url.password !== "") {
    return {
      kind: "blocked",
      reason: "The address contains a user name or password, which can make it look like another site.",
      scheme,
    };
  }
  return { kind: "web", url: url.href, host: url.hostname };
}

/**
 * An address from a document as it is shown to the reader before it is known to be safe: control
 * characters and the characters that turn text around (bidirectional marks) are written out as
 * \u{...}, so they cannot make the address read as something else.
 */
export function visibleAddress(text: string): string {
  let out = "";
  for (const ch of text) {
    const n = ch.codePointAt(0) ?? 0;
    const hidden =
      n < 0x20 ||
      (n >= 0x7f && n <= 0x9f) ||
      n === 0x061c ||
      n === 0x200e ||
      n === 0x200f ||
      (n >= 0x202a && n <= 0x202e) ||
      (n >= 0x2066 && n <= 0x2069);
    out += hidden ? `\\u{${n.toString(16).toUpperCase()}}` : ch;
  }
  return out;
}

/**
 * What to call a link on a button or in a tooltip: the cleaned up address when it is a web address
 * (so what is shown is what would be opened, and a trick such as a backslash before an @ shows its
 * real site), and a plain description when it is not one.
 */
export function describeLink(uri: string, max = 80): string {
  const verdict = classifyUri(uri);
  return verdict.kind === "web" ? shortAddress(verdict.url, max) : "not a web address, not opened";
}

/** A short form of an address for a button or a tooltip. The full address is always shown in the question. */
export function shortAddress(url: string, max = 60): string {
  if (url.length <= max) return url;
  const half = Math.floor((max - 1) / 2);
  return `${url.slice(0, half)}…${url.slice(url.length - half)}`;
}

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

/** A short form of an address for a button or a tooltip. The full address is always shown in the question. */
export function shortAddress(url: string, max = 60): string {
  if (url.length <= max) return url;
  const half = Math.floor((max - 1) / 2);
  return `${url.slice(0, half)}…${url.slice(url.length - half)}`;
}

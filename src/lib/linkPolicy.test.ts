import { describe, expect, it } from "vitest";
import { MAX_URI_LENGTH, classifyUri, shortAddress } from "./linkPolicy";

describe("classifyUri", () => {
  it("accepts plain http and https addresses", () => {
    for (const ok of [
      "http://example.com",
      "https://example.com/path?q=1&r=2#frag",
      "HTTPS://EXAMPLE.COM/",
      "https://example.com:8443/a",
      "https://[2001:db8::1]/",
    ]) {
      expect(classifyUri(ok).kind, ok).toBe("web");
    }
  });

  it("gives the cleaned up address and the site name, international names in ASCII", () => {
    expect(classifyUri("HTTPS://EXAMPLE.COM")).toEqual({ kind: "web", url: "https://example.com/", host: "example.com" });
    const idn = classifyUri("https://bücher.example/");
    expect(idn).toMatchObject({ kind: "web", host: "xn--bcher-kva.example" });
  });

  it("never accepts anything but http and https", () => {
    for (const uri of [
      "javascript:alert(1)",
      "JavaScript:alert(1)",
      "file:///etc/passwd",
      "file://server/share/a.exe",
      "ftp://example.com/",
      "mailto:a@example.com",
      "tel:+4712345678",
      "data:text/html,<script>1</script>",
      "vbscript:run",
      "ms-msdt:/id",
      "ssh://host",
      "smb://host/share",
      "tauri://localhost",
      "blob:https://example.com/x",
    ]) {
      const verdict = classifyUri(uri);
      expect(verdict.kind, uri).toBe("blocked");
      if (verdict.kind === "blocked") expect(verdict.scheme, uri).not.toBeNull();
    }
  });

  it("refuses text that is not an address", () => {
    for (const uri of ["", "example.com", "//example.com", "http:", "https://", "not a url", "http//example.com"]) {
      expect(classifyUri(uri).kind, uri).toBe("blocked");
    }
  });

  it("refuses an address with a user name or password, which can pose as another site", () => {
    for (const uri of ["https://user:pw@example.com/", "https://google.com@evil.example/", "https://a@example.com"]) {
      const verdict = classifyUri(uri);
      expect(verdict.kind, uri).toBe("blocked");
      if (verdict.kind === "blocked") expect(verdict.reason).toContain("user name");
    }
  });

  it("refuses control characters, spaces and a very long address", () => {
    for (const uri of ["https://example.com/\u0000", "https://example.com/\n", " https://example.com/", "https://exa mple.com/", "https://example.com/\u0085"]) {
      expect(classifyUri(uri).kind, JSON.stringify(uri)).toBe("blocked");
    }
    expect(classifyUri(`https://example.com/${"a".repeat(MAX_URI_LENGTH)}`).kind).toBe("blocked");
  });
});

describe("shortAddress", () => {
  it("leaves a short address and cuts a long one in the middle", () => {
    expect(shortAddress("https://example.com/")).toBe("https://example.com/");
    const long = `https://example.com/${"a".repeat(200)}end`;
    const short = shortAddress(long, 41);
    expect(short).toHaveLength(41);
    expect(short.startsWith("https://example.com/")).toBe(true);
    expect(short.endsWith("end")).toBe(true);
    expect(short).toContain("…");
  });
});

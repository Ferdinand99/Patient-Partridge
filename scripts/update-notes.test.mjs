import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { withNotes } from "./update-notes.mjs";

const latest = { version: "0.2.0", notes: "", pub_date: "2026-01-01T00:00:00Z", platforms: { "windows-x86_64": { signature: "s", url: "u" } } };

describe("withNotes", () => {
  it("copies the release body into notes and keeps everything else", () => {
    const out = withNotes(latest, "## 0.2.0\r\n\r\n* Faster start\r\n");
    expect(out.notes).toBe("## 0.2.0\n\n* Faster start");
    expect(out.platforms).toEqual(latest.platforms);
    expect(out.version).toBe("0.2.0");
    expect(latest.notes).toBe("");
  });
  it("keeps existing notes when the body is empty", () => {
    expect(withNotes({ ...latest, notes: "old" }, "  \n").notes).toBe("old");
    expect(withNotes(latest, undefined).notes).toBe("");
  });
  it("refuses something that is not a manifest", () => {
    expect(() => withNotes({}, "x")).toThrow();
    expect(() => withNotes(null, "x")).toThrow();
  });
});

// The same code ran for Nimble Newt v0.3.0. Its real latest.json (as tauri-action wrote it, with
// empty notes before the publish step) and the real body of its release are the data.
const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/nimble-newt-v0.3.0/${name}`, import.meta.url), "utf8"));

describe("withNotes on the real files of Nimble Newt v0.3.0", () => {
  const real = fixture("latest.json");
  const release = fixture("release.json");
  it("gives the notes the body of the release, and keeps every system and signature as it was", () => {
    const written = { ...real, notes: "" };
    const out = withNotes(written, release.body);
    expect(out.notes).toBe(release.body.replace(/\r\n/g, "\n").trim());
    expect(out.notes).toBe(real.notes);
    expect(out.platforms).toEqual(real.platforms);
    expect(out.pub_date).toBe(real.pub_date);
  });
  it("finds the notes already in place, so that a second run changes nothing", () => {
    expect(withNotes(real, release.body).notes).toBe(real.notes);
  });
});

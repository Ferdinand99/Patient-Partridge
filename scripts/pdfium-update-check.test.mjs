import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { buildOfTag, compare, featureOf, issueBody, issueTitle } from "./pdfium-update-check.mjs";

const lock = JSON.parse(readFileSync(new URL("../pdfium.lock.json", import.meta.url), "utf8"));
const cargo = readFileSync(new URL("../crates/engine/Cargo.toml", import.meta.url), "utf8");

describe("the repository", () => {
  it("asks pdfium-render for the feature of the pinned build", () => {
    expect(featureOf(cargo)).toBe(lock.build);
  });
  it("names the release after the build", () => {
    expect(lock.release).toBe(`chromium/${lock.build}`);
  });
});

describe("buildOfTag", () => {
  it("reads the number", () => expect(buildOfTag("chromium/8125")).toBe(8125));
  it.each(["8125", "chromium/", "chromium/81x", "v1.2", "", undefined])("refuses %s", (tag) => {
    expect(() => buildOfTag(tag)).toThrow(/Unexpected release tag/);
  });
});

describe("featureOf", () => {
  it("finds one feature", () => expect(featureOf('features = ["pdfium_8125"]')).toBe(8125));
  it("gives null for none or several", () => {
    expect(featureOf("features = []")).toBeNull();
    expect(featureOf('features = ["pdfium_8125", "pdfium_7881"]')).toBeNull();
  });
});

describe("compare", () => {
  it("is not newer for the same build", () => {
    expect(compare(lock, `chromium/${lock.build}`, cargo).newer).toBe(false);
  });
  it("is not newer for an older release", () => {
    expect(compare(lock, `chromium/${lock.build - 1}`, cargo).newer).toBe(false);
  });
  it("is newer for a higher build, compared as a number", () => {
    expect(compare({ ...lock, build: 999 }, "chromium/1000", cargo).newer).toBe(true);
  });
  it("sees a feature that does not match the lock", () => {
    expect(compare({ ...lock, build: 7000 }, "chromium/7000", cargo).featureMatchesLock).toBe(false);
  });
});

describe("the issue", () => {
  const result = compare(lock, `chromium/${lock.build + 50}`, cargo);
  it("has the tag in the title, so a later run can find it", () => {
    expect(issueTitle("chromium/8200")).toBe("PDFium update available: chromium/8200");
  });
  it("links the release and the steps", () => {
    const body = issueBody(result, lock, null);
    expect(body).toContain(`releases/tag/chromium/${lock.build + 50}`);
    expect(body).toContain("docs/pdfium-updates.md");
  });
  it("tells when pdfium-render has no feature for the new build yet", () => {
    const body = issueBody(result, lock, { version: "0.9.5", newestFeature: lock.build });
    expect(body).toContain("no feature for build");
  });
});

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { checkLegal } from "./check-installer-legal.mjs";

const expected = { notices: "the notices\n", license: "GNU AFFERO GENERAL PUBLIC LICENSE\n" };
const dirs = [];
afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

function installer(files) {
  const dir = mkdtempSync(join(tmpdir(), "legal-"));
  dirs.push(dir);
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), content);
  }
  return dir;
}

const pdfium = (prefix) => ({
  [`${prefix}/pdfium-LICENSE`]: "BSD",
  ...Object.fromEntries(["pdfium", "abseil", "icu", "zlib", "freetype"].map((n) => [`${prefix}/pdfium-licenses/${n}.txt`, n])),
});
const full = (prefix) => ({
  [`${prefix}/LICENSE`]: expected.license,
  [`${prefix}/THIRD-PARTY-NOTICES`]: expected.notices,
  ...pdfium(prefix),
});

describe("checkLegal", () => {
  it("accepts an installer with everything, also with Windows line ends", () => {
    const dir = installer({ ...full("App"), "App/LICENSE": expected.license.replace(/\n/g, "\r\n") });
    expect(checkLegal("windows", dir, expected)).toEqual([]);
  });
  it("accepts a deb with the copyright file", () => {
    const dir = installer({
      ...full("usr/lib/Patient Partridge"),
      "usr/share/doc/patient-partridge/copyright": expected.license,
      "usr/share/doc/patient-partridge/THIRD-PARTY-NOTICES": expected.notices,
    });
    expect(checkLegal("linux", dir, expected)).toEqual([]);
  });
  it("wants the copyright file in a deb", () => {
    const dir = installer(full("usr/lib/Patient Partridge"));
    expect(checkLegal("linux", dir, expected).join("\n")).toContain("copyright");
  });
  it("finds a missing notices file, a stale one and a wrong licence", () => {
    const none = installer({ ...pdfium("A"), "A/LICENSE": expected.license });
    expect(checkLegal("macos", none, expected)).toEqual(["THIRD-PARTY-NOTICES is not in the installer"]);
    const stale = installer({ ...full("A"), "A/THIRD-PARTY-NOTICES": "old" });
    expect(checkLegal("macos", stale, expected)[0]).toContain("not the THIRD-PARTY-NOTICES");
    const wrong = installer({ ...full("A"), "A/LICENSE": "MIT" });
    expect(checkLegal("macos", wrong, expected)[0]).toContain("not the LICENSE");
  });
  it("finds missing PDFium licence texts", () => {
    const dir = installer({ "A/LICENSE": expected.license, "A/THIRD-PARTY-NOTICES": expected.notices });
    const problems = checkLegal("windows", dir, expected);
    expect(problems.join("\n")).toContain("pdfium-LICENSE");
    expect(problems.join("\n")).toContain("pdfium-licenses");
  });
});

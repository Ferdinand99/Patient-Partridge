import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { checkLegal, checkSystem, pdfiumLicenses } from "./check-installer-legal.mjs";

const expected = { notices: "the notices\n", license: "GNU AFFERO GENERAL PUBLIC LICENSE\n", pdfium: null };
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

const pdfiumNames = ["pdfium", "abseil", "icu", "zlib", "freetype"];
const pdfium = (prefix) => ({
  [`${prefix}/pdfium-LICENSE`]: "BSD",
  ...Object.fromEntries(pdfiumNames.map((n) => [`${prefix}/pdfium-licenses/${n}.txt`, n])),
});
const full = (prefix) => ({
  [`${prefix}/LICENSE`]: expected.license,
  [`${prefix}/THIRD-PARTY-NOTICES`]: expected.notices,
  ...pdfium(prefix),
});
const withDownload = { ...expected, pdfium: new Map([["LICENSE", "BSD"], ...pdfiumNames.map((n) => [`${n}.txt`, n])]) };

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
  it("compares every PDFium licence file with the download, also an empty or changed one", () => {
    expect(checkLegal("windows", installer(full("A")), withDownload)).toEqual([]);
    const lost = installer({ ...full("A") });
    rmSync(join(lost, "A", "pdfium-licenses", "icu.txt"));
    expect(checkLegal("windows", lost, withDownload)).toEqual(["pdfium-licenses/icu.txt is not in the installer"]);
    const changed = installer({ ...full("A"), "A/pdfium-licenses/zlib.txt": "other" });
    expect(checkLegal("windows", changed, withDownload)).toEqual(["pdfium-licenses/zlib.txt is not the file of the PDFium download"]);
    const empty = installer({ ...full("A"), "A/pdfium-LICENSE": "" });
    expect(checkLegal("windows", empty, withDownload).join("\n")).toContain("pdfium-LICENSE is missing or empty");
  });
});

describe("checkSystem (each installer format on its own)", () => {
  const windows = (msi, nsis) => installer({ ...msi, ...nsis });
  it("passes when msi and nsis both carry everything", () => {
    expect(checkSystem("windows", windows(full("msi/PFiles/App"), full("nsis")), expected)).toEqual([]);
  });
  it("fails when only the msi lacks the notices, although the nsis has them", () => {
    const msi = full("msi/PFiles/App");
    delete msi["msi/PFiles/App/THIRD-PARTY-NOTICES"];
    const problems = checkSystem("windows", windows(msi, full("nsis")), expected);
    expect(problems).toEqual(["msi: THIRD-PARTY-NOTICES is not in the installer"]);
  });
  it("fails when only the nsis lacks the AGPL text, although the msi has it", () => {
    const nsis = full("nsis");
    delete nsis["nsis/LICENSE"];
    const problems = checkSystem("windows", windows(full("msi/PFiles/App"), nsis), expected);
    expect(problems).toEqual(["nsis: LICENSE (the AGPL text) is not in the installer"]);
  });
  it("fails when a format was not unpacked at all", () => {
    const problems = checkSystem("windows", windows(full("msi/PFiles/App"), {}), expected);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("nsis: the nsis installer was not unpacked");
  });
  it("checks the deb and the dmg as their own formats", () => {
    expect(checkSystem("macos", installer(full("dmg/App.app/Contents/Resources")), expected)).toEqual([]);
    expect(checkSystem("macos", installer(full("msi/x")), expected)[0]).toContain("dmg: the dmg installer was not unpacked");
    expect(checkSystem("linux", installer(full("deb/usr/lib/Patient Partridge")), expected).join("\n")).toContain("deb: usr/share/doc/patient-partridge/copyright");
  });
});

describe("the AppImage", () => {
  const app = "appimage/usr/lib/Patient Partridge";
  const libraries = { "appimage/usr/share/doc/libgtk/LICENSE": "LGPL of GTK", "appimage/usr/lib/webkit/THIRD-PARTY-NOTICES": "WebKit's own" };
  it("is checked as its own format, without the Debian copyright file", () => {
    expect(checkSystem("linux", installer({ ...full("deb/usr/lib/Patient Partridge"), "deb/usr/share/doc/patient-partridge/copyright": expected.license, "deb/usr/share/doc/patient-partridge/THIRD-PARTY-NOTICES": expected.notices, ...full(app) }), expected)).toEqual([]);
  });
  it("looks only at the app's own folder, not at the files of the libraries it carries", () => {
    expect(checkLegal("linux", installer({ ...full(app), ...libraries }), expected, "appimage")).toEqual([]);
    // The same files would be taken for the app's own texts in a deb.
    expect(checkLegal("linux", installer({ ...full(app), ...libraries }), expected, "deb").join(" ")).toContain("not the LICENSE");
  });
  it("fails when the app folder lacks the notices, whatever the libraries have", () => {
    const files = { ...full(app), ...libraries };
    delete files[`${app}/THIRD-PARTY-NOTICES`];
    expect(checkLegal("linux", installer(files), expected, "appimage")).toEqual(["THIRD-PARTY-NOTICES is not in the installer"]);
  });
  it("fails when it was not unpacked", () => {
    expect(checkSystem("linux", installer({ ...full("deb/usr/lib/Patient Partridge"), "deb/usr/share/doc/patient-partridge/copyright": expected.license, "deb/usr/share/doc/patient-partridge/THIRD-PARTY-NOTICES": expected.notices }), expected)[0]).toContain("appimage: the appimage installer was not unpacked");
  });
});

describe("pdfiumLicenses", () => {
  it("reads the licence files of a download", () => {
    const dir = installer({ LICENSE: "BSD", ...Object.fromEntries(pdfiumNames.map((n) => [`licenses/${n}.txt`, n])) });
    const map = pdfiumLicenses(dir);
    expect(map.get("LICENSE")).toBe("BSD");
    expect(map.size).toBe(6);
  });
  it("refuses a folder without them", () => {
    expect(() => pdfiumLicenses(installer({ other: "x" }))).toThrow(/fetch PDFium first/);
  });
});

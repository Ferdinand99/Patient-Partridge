import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { checkDesktopFile, checkMacPlist, checkMsiRows } from "./association.mjs";

const conf = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
const desktopTemplate = readFileSync(new URL("../src-tauri/partridge.desktop", import.meta.url), "utf8");

describe("the file association in the app configuration", () => {
  const [pdf, ...others] = conf.bundle.fileAssociations;

  it("declares PDF files, and nothing else", () => {
    expect(others).toEqual([]);
    expect(pdf.ext).toEqual(["pdf"]);
    expect(pdf.mimeType).toBe("application/pdf");
    expect(pdf.contentTypes).toEqual(["com.adobe.pdf"]);
  });

  it("asks to be a viewer that can be chosen, not to take over", () => {
    expect(pdf.role).toBe("Viewer");
    // "Alternate": listed under "Open with", never the default until the user says so.
    expect(pdf.rank).toBe("Alternate");
  });

  it("targets every system that has an installer", () => {
    expect(conf.bundle.targets).toEqual(expect.arrayContaining(["msi", "nsis", "dmg", "app", "deb"]));
  });

  it("passes the files on Linux: %F in Exec and a closed MimeType list", () => {
    expect(desktopTemplate).toMatch(/^Exec=\{\{exec\}\} %F$/m);
    expect(desktopTemplate).toMatch(/^MimeType=\{\{mime_type\}\};$/m);
  });

  it("keeps the strict content security policy", () => {
    const csp = conf.app.security.csp;
    expect(csp).toContain("default-src 'self'");
    expect(csp).toContain("script-src 'self'");
    expect(csp).not.toMatch(/unsafe-/);
    // Pictures only from the app itself and the pages prepared for printing (blob: pictures).
    expect(csp).toContain("img-src 'self' blob:;");
    expect(csp).not.toMatch(/img-src[^;]*(data:|\*)/);
    // The only address is the app's own channel to its Rust side, on this computer.
    expect(csp.replace("http://ipc.localhost", "")).not.toMatch(/https?:/);
  });
});

describe("checking what the build made", () => {
  const goodPlist = {
    CFBundleDocumentTypes: [
      {
        CFBundleTypeExtensions: ["pdf"],
        CFBundleTypeRole: "Viewer",
        LSHandlerRank: "Alternate",
        LSItemContentTypes: ["com.adobe.pdf"],
      },
    ],
  };

  it("accepts a good macOS plist and names each problem in a bad one", () => {
    expect(checkMacPlist(goodPlist)).toEqual([]);
    expect(checkMacPlist({})).toHaveLength(1);
    const bad = JSON.parse(JSON.stringify(goodPlist));
    bad.CFBundleDocumentTypes[0].CFBundleTypeRole = "Editor";
    bad.CFBundleDocumentTypes[0].LSHandlerRank = "Owner";
    bad.CFBundleDocumentTypes[0].LSItemContentTypes = [];
    expect(checkMacPlist(bad)).toHaveLength(3);
  });

  it("accepts a good MSI and refuses one without the extension", () => {
    expect(checkMsiRows(["pdf"])).toEqual([]);
    expect(checkMsiRows(["PDF"])).toEqual([]);
    expect(checkMsiRows([])).toHaveLength(1);
    expect(checkMsiRows(["txt"])).toHaveLength(1);
  });

  it("checks the desktop file of the deb", () => {
    expect(checkDesktopFile("Exec=patient-partridge %F\nMimeType=application/pdf;\n")).toEqual([]);
    expect(checkDesktopFile("Exec=patient-partridge\nMimeType=application/pdf\n")).toHaveLength(2);
  });
});

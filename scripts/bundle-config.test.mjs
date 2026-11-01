import { describe, expect, it } from "vitest";
import { MAC_TRIPLES, UNIVERSAL_TRIPLE, bundleConfig, codesignArgs, wantsUpdaterArtifacts } from "./bundle-config.mjs";

describe("update files", () => {
  it("are made only when the signing key is there", () => {
    expect(wantsUpdaterArtifacts({})).toBe(false);
    expect(wantsUpdaterArtifacts({ TAURI_SIGNING_PRIVATE_KEY: "" })).toBe(false);
    expect(wantsUpdaterArtifacts({ TAURI_SIGNING_PRIVATE_KEY: "key" })).toBe(true);
    expect(wantsUpdaterArtifacts({ TAURI_SIGNING_PRIVATE_KEY_PATH: "/k" })).toBe(true);
  });
});

describe("bundleConfig", () => {
  it("ships the engine as a sidecar and the library with its licences from the right folder", () => {
    const c = bundleConfig({ folder: "mac-universal", file: "libpdfium.dylib", env: {} });
    expect(c.bundle.externalBin).toEqual(["binaries/partridge-engine"]);
    expect(c.bundle.resources).toEqual({
      "../vendor/pdfium/mac-universal/libpdfium.dylib": "libpdfium.dylib",
      "../vendor/pdfium/mac-universal/LICENSE": "pdfium-LICENSE",
      "../vendor/pdfium/mac-universal/licenses": "pdfium-licenses",
    });
    expect(c.bundle.createUpdaterArtifacts).toBe(false);
    expect(bundleConfig({ folder: "win-x64", file: "pdfium.dll", env: { TAURI_SIGNING_PRIVATE_KEY: "k" } }).bundle.createUpdaterArtifacts).toBe(true);
  });
});

describe("macOS", () => {
  it("builds the universal app from the two Mac targets, and Tauri's name for the joined sidecar", () => {
    expect(MAC_TRIPLES).toEqual(["aarch64-apple-darwin", "x86_64-apple-darwin"]);
    expect(UNIVERSAL_TRIPLE).toBe("universal-apple-darwin");
  });

  it("signs a library with the hardened runtime and a secure timestamp", () => {
    const args = codesignArgs("Developer ID Application: Someone (TEAM)", "/x/libpdfium.dylib");
    expect(args).toContain("--options");
    expect(args[args.indexOf("--options") + 1]).toBe("runtime");
    expect(args).toContain("--timestamp");
    expect(args.at(-1)).toBe("/x/libpdfium.dylib");
    expect(() => codesignArgs("", "/x")).toThrow();
  });
});

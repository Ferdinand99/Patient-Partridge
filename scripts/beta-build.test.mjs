import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  BETA_IDENTIFIER,
  BETA_PRODUCT_NAME,
  BETA_WINDOWS_BINARY_NAME,
  badgeIcon,
  betaOverride,
  betaVersion,
  decodePng,
  encodePng,
  halve,
  mergeConfig,
} from "./beta-build.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const stable = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8"));

describe("the beta settings", () => {
  const generated = {
    productName: "Patient Partridge",
    identifier: "com.ferdinand99.patientpartridge",
    version: "0.1.0",
    bundle: { createUpdaterArtifacts: true, externalBin: ["binaries/partridge-engine"], fileAssociations: [{ ext: ["pdf"] }], icon: ["icons/icon.icns"] },
    plugins: { updater: { endpoints: ["https://example.invalid/latest.json"], pubkey: "key" } },
  };
  const merged = mergeConfig(generated, betaOverride({ version: "0.1.0", build: "42" }));

  it("name the app and keep its data apart from the stable app", () => {
    expect(merged.productName).toBe(BETA_PRODUCT_NAME);
    expect(merged.productName).not.toBe(stable.productName);
    expect(merged.identifier).toBe(BETA_IDENTIFIER);
    expect(merged.identifier).not.toBe(stable.identifier);
    expect(merged.version).toBe("0.1.0-beta.42");
  });

  it("make no update file, ask no update server and take no file type", () => {
    expect(merged.bundle.createUpdaterArtifacts).toBe(false);
    expect(merged.plugins.updater.endpoints).toEqual([]);
    expect(merged.bundle.fileAssociations).toEqual([]);
  });

  it("leave the rest of the signed build alone and use the beta icons", () => {
    expect(merged.bundle.externalBin).toEqual(["binaries/partridge-engine"]);
    expect(merged.plugins.updater.pubkey).toBe("key");
    expect(merged.bundle.icon.every((f) => f.startsWith("icons-beta/"))).toBe(true);
    expect(merged.bundle.icon).toContain("icons-beta/icon.icns");
  });

  it("only need files that the beta icon set has", () => {
    expect(new Set(betaOverride({ version: "0.1.0", build: "1" }).bundle.icon.map((f) => f.split("/")[1]))).toEqual(
      new Set(["32x32.png", "128x128.png", "128x128@2x.png", "icon.icns", "icon.ico", "icon.png"]),
    );
  });

  it("do not touch the config of the stable app", () => {
    expect(stable.productName).toBe("Patient Partridge");
    expect(stable.identifier).toBe("com.ferdinand99.patientpartridge");
    expect(stable.bundle.createUpdaterArtifacts).toBe(true);
  });
});

describe("the Windows beta settings", () => {
  const generated = {
    productName: "Patient Partridge",
    identifier: "com.ferdinand99.patientpartridge",
    version: "0.1.0",
    bundle: { createUpdaterArtifacts: false, externalBin: ["binaries/partridge-engine"], fileAssociations: [{ ext: ["pdf"] }], icon: ["icons/icon.ico"] },
    plugins: { updater: { endpoints: ["https://example.invalid/latest.json"], pubkey: "key" } },
  };
  const merged = mergeConfig(generated, betaOverride({ version: "0.1.0", build: "42", platform: "windows" }));

  it("keep everything the other systems get", () => {
    expect(merged.productName).toBe("Patient Partridge Beta");
    expect(merged.identifier).toBe("com.ferdinand99.patientpartridge.beta");
    expect(merged.version).toBe("0.1.0-beta.42");
    expect(merged.plugins.updater.endpoints).toEqual([]);
    expect(merged.bundle.fileAssociations).toEqual([]);
    expect(merged.bundle.createUpdaterArtifacts).toBe(false);
  });

  it("use the icon.ico of the ribbon icon set", () => {
    expect(merged.bundle.icon).toContain("icons-beta/icon.ico");
    expect(merged.bundle.icon.some((f) => !f.startsWith("icons-beta/"))).toBe(false);
  });

  it("name the program differently from the stable one, so the installers never end each other's app", () => {
    expect(merged.mainBinaryName).toBe(BETA_WINDOWS_BINARY_NAME);
    expect(BETA_WINDOWS_BINARY_NAME).toBe("patient-partridge-beta");
    expect(BETA_WINDOWS_BINARY_NAME).not.toBe("patient-partridge");
    expect(stable.mainBinaryName).toBeUndefined();
  });

  it("leave the name of the program alone on the other systems", () => {
    expect(betaOverride({ version: "0.1.0", build: "1" }).mainBinaryName).toBeUndefined();
    expect(betaOverride({ version: "0.1.0", build: "1", platform: "macos" }).mainBinaryName).toBeUndefined();
  });
});

describe("the beta version", () => {
  it("is the version of the app with a beta mark and the number of the run", () => {
    expect(betaVersion("0.1.0", "7")).toBe("0.1.0-beta.7");
    expect(betaVersion("1.2.3-rc.1", 12)).toBe("1.2.3-beta.12");
  });
  it("is refused without a number", () => {
    expect(() => betaVersion("0.1.0", undefined)).toThrow();
    expect(() => betaVersion("0.1.0", "x")).toThrow();
  });
});

describe("mergeConfig", () => {
  it("merges objects, replaces arrays and removes what the patch sets to null", () => {
    expect(mergeConfig({ a: { b: 1, c: 2 }, d: [1, 2], e: 1 }, { a: { b: 9 }, d: [3], e: null })).toEqual({ a: { b: 9, c: 2 }, d: [3] });
  });
});

describe("the ribbon", () => {
  const icon = decodePng(readFileSync(join(root, "assets", "app-icon.png")));
  const badge = badgeIcon(icon);
  const at = (img, x, y) => [...img.data.subarray((y * img.width + x) * 4, (y * img.width + x) * 4 + 4)];

  it("reads and writes PNG files without loss", () => {
    const again = decodePng(encodePng(icon));
    expect(again.width).toBe(1024);
    expect(Buffer.from(again.data).equals(Buffer.from(icon.data))).toBe(true);
  });

  it("changes the lower right corner and leaves the upper left alone", () => {
    for (const [x, y] of [[100, 100], [512, 200], [300, 300], [60, 700], [700, 60]]) expect(at(badge, x, y)).toEqual(at(icon, x, y));
    // The middle of the ribbon, off the letters: violet.
    const [r, g, b] = at(badge, 950, 545);
    expect([r, g, b]).not.toEqual(at(icon, 950, 545).slice(0, 3));
    expect(b).toBeGreaterThan(r);
  });

  it("keeps the shape of the icon: see-through stays see-through, and the opacity is the same everywhere", () => {
    for (let i = 3; i < icon.data.length; i += 4) if (icon.data[i] !== badge.data[i]) throw new Error(`opacity changed at ${(i - 3) / 4}`);
  });

  it("puts white letters on the ribbon", () => {
    let white = 0;
    for (let y = 400; y < 1000; y++) {
      for (let x = 400; x < 1000; x++) {
        const [r, g, b, a] = at(badge, x, y);
        if (a === 255 && r > 250 && g > 250 && b > 250) white++;
      }
    }
    expect(white).toBeGreaterThan(5000);
  });

  it("makes a smaller icon with the same opacity rule", () => {
    const small = halve(badge);
    expect(small.width).toBe(512);
    expect(at(small, 0, 0)[3]).toBe(0);
  });
});

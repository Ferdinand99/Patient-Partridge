import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { REQUIRED, assetOf, problemsOf } from "./check-latest.mjs";

// Real files of Nimble Newt v0.3.0, made by the same tauri-action commit that the release workflow
// pins: its latest.json, and the assets of that release (id and name) as the GitHub API lists them.
const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/nimble-newt-v0.3.0/${name}`, import.meta.url), "utf8"));
const real = fixture("latest.json");
const release = fixture("release.json");
const assets = release.assets.map(({ id, name }) => ({ id, name }));
// Nimble Newt has no Linux build, so these are the systems it has.
const hasWindowsAndMac = Object.fromEntries(Object.entries(REQUIRED).filter(([k]) => !k.startsWith("linux")));
const api = (id) => `https://api.github.com/repos/Ferdinand99/Nimble-Newt/releases/assets/${id}`;

describe("the real latest.json of Nimble Newt v0.3.0", () => {
  it("has the shape that the check must accept: API addresses that end in an asset number", () => {
    expect(real.platforms["windows-x86_64"].url).toBe(api(686300632));
  });

  it("passes for the systems it has", () => {
    expect(problemsOf(real, assets, "0.3.0", hasWindowsAndMac)).toEqual([]);
  });

  it("finds the asset by its number, also for the universal macOS file that two systems share", () => {
    expect(assetOf(real.platforms["windows-x86_64"].url, assets)?.name).toBe("Nimble.Newt_0.3.0_x64-setup.exe");
    expect(assetOf(real.platforms["darwin-aarch64"].url, assets)?.name).toBe("Nimble.Newt_0.3.0_universal.app.tar.gz");
    expect(assetOf(real.platforms["darwin-x86_64"].url, assets)?.name).toBe("Nimble.Newt_0.3.0_universal.app.tar.gz");
  });

  it("asks for Linux when Linux is required, as it is for Patient Partridge", () => {
    expect(problemsOf(real, assets, "0.3.0")).toEqual(["linux-x86_64 is missing from latest.json"]);
  });

  it("passes for four systems when Linux is written the way tauri-action writes the others", () => {
    const linuxAssets = [
      ...assets,
      { id: 700000001, name: "Patient.Partridge_0.3.0_amd64.AppImage" },
      { id: 700000002, name: "Patient.Partridge_0.3.0_amd64.AppImage.sig" },
    ];
    const withLinux = {
      ...real,
      platforms: { ...real.platforms, "linux-x86_64": { signature: real.platforms["windows-x86_64"].signature, url: api(700000001) } },
    };
    expect(problemsOf(withLinux, linuxAssets, "0.3.0")).toEqual([]);
  });

  it("notices an address with a number that is not an asset of the release", () => {
    const wrong = { ...real, platforms: { ...real.platforms, "windows-x86_64": { ...real.platforms["windows-x86_64"], url: api(1) } } };
    expect(problemsOf(wrong, assets, "0.3.0", hasWindowsAndMac).join("\n")).toContain(`windows-x86_64 points to ${api(1)}, which is not an asset`);
  });

  it("notices a missing .sig asset, a missing signature, a wrong file, a wrong version and empty notes", () => {
    const noSig = assets.filter((a) => a.name !== "Nimble.Newt_0.3.0_x64-setup.exe.sig");
    expect(problemsOf(real, noSig, "0.3.0", hasWindowsAndMac)).toEqual(["Nimble.Newt_0.3.0_x64-setup.exe has no .sig asset next to it"]);
    const broken = {
      ...real,
      version: "0.2.0",
      notes: " ",
      platforms: {
        ...real.platforms,
        "windows-x86_64": { ...real.platforms["windows-x86_64"], signature: "" },
        "darwin-aarch64": { ...real.platforms["darwin-aarch64"], url: api(686310187) },
      },
    };
    const text = problemsOf(broken, assets, "0.3.0", hasWindowsAndMac).join("\n");
    expect(text).toContain("latest.json says version 0.2.0");
    expect(text).toContain("windows-x86_64 has no signature");
    expect(text).toContain("darwin-aarch64 points to Nimble.Newt_0.3.0_universal.dmg, which is not an update file for it");
    expect(text).toContain("no release notes");
  });
});

describe("addresses that end in a file name", () => {
  const list = [{ id: 5, name: "App.exe" }];
  it("are followed by name", () => {
    expect(assetOf("https://github.com/o/r/releases/download/v1/App.exe", list)?.id).toBe(5);
    expect(assetOf("https://github.com/o/r/releases/download/v1/Other.exe", list)).toBeNull();
  });
  it("a number that is not at the end of an API asset address is not taken for an id", () => {
    expect(assetOf("https://example.com/5", list)).toBeNull();
    expect(assetOf("not an address", list)).toBeNull();
  });
});

describe("something that is not a manifest", () => {
  it("is refused", () => {
    expect(problemsOf(null, [], "0.2.0")).toHaveLength(1);
  });
});

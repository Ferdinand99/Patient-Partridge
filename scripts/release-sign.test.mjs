import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { bundleDirFor, notaryAccepted, notaryArgs, optionOf, payloadsFor, shellArg, signerArgs } from "./release-sign.mjs";

const dirs = [];
afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));
function bundle(files) {
  const dir = mkdtempSync(join(tmpdir(), "bundle-"));
  dirs.push(dir);
  for (const f of files) {
    mkdirSync(join(dir, f, ".."), { recursive: true });
    writeFileSync(join(dir, f), "x");
  }
  return dir;
}

describe("payloadsFor", () => {
  it("finds the installers of Windows and the AppImage of Linux, and nothing for macOS", () => {
    const win = bundle(["nsis/Patient Partridge_0.1.0_x64-setup.exe", "msi/Patient Partridge_0.1.0_x64_en-US.msi", "nsis/other.txt"]);
    expect(payloadsFor("win32", win).map((p) => p.split(/[\\/]/).pop())).toEqual(["Patient Partridge_0.1.0_x64-setup.exe", "Patient Partridge_0.1.0_x64_en-US.msi"]);
    const linux = bundle(["appimage/Patient Partridge_0.1.0_amd64.AppImage", "deb/Patient Partridge_0.1.0_amd64.deb"]);
    expect(payloadsFor("linux", linux).map((p) => p.split(/[\\/]/).pop())).toEqual(["Patient Partridge_0.1.0_amd64.AppImage"]);
    expect(payloadsFor("darwin", bundle(["macos/x.app.tar.gz"]))).toEqual([]);
  });
  it("gives an empty list for a folder that is not there, for the caller to report", () => {
    expect(payloadsFor("linux", join(tmpdir(), "no-such-bundle-folder"))).toEqual([]);
  });
});

describe("the arguments of a build", () => {
  it("find the bundle folder, with and without --target", () => {
    expect(bundleDirFor(["--config", "c.json"], "/t")).toBe(join("/t", "release", "bundle"));
    expect(bundleDirFor(["--target", "universal-apple-darwin"], "/t")).toBe(join("/t", "universal-apple-darwin", "release", "bundle"));
    expect(optionOf(["--config", "c.json", "--bundles", "app,dmg"], "--bundles")).toBe("app,dmg");
    expect(optionOf([], "--bundles")).toBeUndefined();
  });
});

describe("signerArgs", () => {
  it("binds the signature to the version, which requireSignedVersion needs", () => {
    expect(signerArgs("/x/a.AppImage", "0.1.0")).toEqual(["signer", "sign", "--app-version", "0.1.0", "/x/a.AppImage"]);
    expect(() => signerArgs("/x", "latest")).toThrow();
  });
});

describe("notarisation of the disk image", () => {
  const env = { APPLE_API_KEY_PATH: "/k.p8", APPLE_API_KEY: "KEYID", APPLE_API_ISSUER: "ISSUER" };
  it("asks notarytool with the key and waits for the answer", () => {
    const args = notaryArgs("/x/a.dmg", env);
    expect(args.slice(0, 3)).toEqual(["notarytool", "submit", "/x/a.dmg"]);
    expect(args).toContain("--wait");
    expect(args[args.indexOf("--key-id") + 1]).toBe("KEYID");
    expect(() => notaryArgs("/x", { ...env, APPLE_API_ISSUER: "" })).toThrow(/APPLE_API_ISSUER/);
  });
  it("takes only Accepted for success", () => {
    expect(notaryAccepted('{"id":"1","status":"Accepted"}')).toBe(true);
    expect(notaryAccepted('{"status":"Invalid"}')).toBe(false);
    expect(notaryAccepted("not json")).toBe(false);
  });
});

describe("shellArg", () => {
  it("keeps a file name with a space in one piece on Windows, and leaves everything alone elsewhere", () => {
    expect(shellArg("D:/a/Patient Partridge_0.1.0_x64-setup.exe", true)).toBe('"D:/a/Patient Partridge_0.1.0_x64-setup.exe"');
    expect(shellArg("--app-version", true)).toBe("--app-version");
    expect(shellArg("a b", false)).toBe("a b");
  });
});

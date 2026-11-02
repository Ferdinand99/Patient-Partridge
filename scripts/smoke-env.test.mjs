import { mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { FIRST_WINDOWS_LAUNCH_LIMIT_MS, LAUNCH_LIMIT_MS, launchEnvironment, launchLimitMs } from "./smoke-env.mjs";

const isFolder = (path) => {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
};

describe("launchEnvironment", () => {
  const made = [];
  const folder = () => {
    const dir = mkdtempSync(join(tmpdir(), "smoke-env-test-"));
    made.push(dir);
    return dir;
  };
  afterEach(() => {
    for (const dir of made.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  for (const platform of ["darwin", "linux"]) {
    it(`${platform}: HOME and the XDG folders are inside the launch folder and they exist`, () => {
      const dir = folder();
      const env = launchEnvironment({ PATH: "/bin", HOME: "/Users/someone" }, dir, platform);
      expect(env.PATH).toBe("/bin");
      for (const name of ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"]) {
        expect(env[name].startsWith(dir), name).toBe(true);
        // A HOME that does not exist makes the web view of some macOS machines hang at start.
        expect(isFolder(env[name]), `${name} must be an existing folder`).toBe(true);
      }
    });
  }

  it("windows: the environment is left as it is and nothing is created", () => {
    const dir = folder();
    const base = { PATH: "C:\\Windows", USERPROFILE: "C:\\Users\\someone" };
    expect(launchEnvironment(base, dir, "win32")).toEqual(base);
    expect(isFolder(join(dir, "home"))).toBe(false);
  });

  it("does not change the environment it is given", () => {
    const base = { HOME: "/Users/someone" };
    launchEnvironment(base, folder(), "darwin");
    expect(base).toEqual({ HOME: "/Users/someone" });
  });
});

describe("launchLimitMs", () => {
  it("gives only the first launch on Windows more time", () => {
    expect(launchLimitMs(0, "win32")).toBe(FIRST_WINDOWS_LAUNCH_LIMIT_MS);
    expect(launchLimitMs(1, "win32")).toBe(LAUNCH_LIMIT_MS);
    expect(launchLimitMs(2, "win32")).toBe(LAUNCH_LIMIT_MS);
    expect(FIRST_WINDOWS_LAUNCH_LIMIT_MS).toBeGreaterThan(LAUNCH_LIMIT_MS);
  });
  it("leaves Linux and macOS at the usual limit", () => {
    for (const platform of ["linux", "darwin"]) {
      expect(launchLimitMs(0, platform)).toBe(LAUNCH_LIMIT_MS);
      expect(launchLimitMs(1, platform)).toBe(LAUNCH_LIMIT_MS);
    }
  });
});

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const read = (path) => JSON.parse(readFileSync(new URL(`../${path}`, import.meta.url), "utf8"));

describe("the first release", () => {
  const config = read("release-please-config.json").packages["."];
  const manifest = read(".release-please-manifest.json")["."];

  // With no earlier release (the manifest says 0.0.0), release-please takes `initial-version` for
  // the first release PR (release-please 17.6.0, src/strategies/base.ts, initialReleaseVersion).
  // Without it the first PR proposes 1.0.0 or 0.0.1, and a 1.0.0 cannot be taken back: the updater
  // only offers a greater version. Wise Wren has the same pair, and its first release was 0.1.0.
  it("is 0.1.0: no earlier release, and the version pinned", () => {
    expect(manifest).toBe("0.0.0");
    expect(config["initial-version"]).toBe("0.1.0");
  });

  it("keeps the versions of every file in step", () => {
    const paths = config["extra-files"].map((f) => f.path);
    for (const path of ["package.json", "package-lock.json", "src-tauri/tauri.conf.json", "Cargo.toml"]) {
      expect(paths).toContain(path);
    }
    expect(config["release-type"]).toBe("simple");
    expect(config.draft).toBe(true);
  });
});

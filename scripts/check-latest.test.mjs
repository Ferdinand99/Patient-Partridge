import { describe, expect, it } from "vitest";
import { problemsOf } from "./check-latest.mjs";

const base = "https://github.com/Ferdinand99/Patient-Partridge/releases/download/v0.2.0";
const sig = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZQ==";
const good = {
  version: "0.2.0",
  notes: "## 0.2.0\n* things",
  platforms: {
    "windows-x86_64": { signature: sig, url: `${base}/Patient.Partridge_0.2.0_x64-setup.exe` },
    "darwin-aarch64": { signature: sig, url: `${base}/Patient.Partridge_universal.app.tar.gz` },
    "darwin-x86_64": { signature: sig, url: `${base}/Patient.Partridge_universal.app.tar.gz` },
    "linux-x86_64": { signature: sig, url: `${base}/Patient.Partridge_0.2.0_amd64.AppImage` },
  },
};
const assets = [
  "Patient.Partridge_0.2.0_x64-setup.exe",
  "Patient.Partridge_0.2.0_x64-setup.exe.sig",
  "Patient.Partridge_universal.app.tar.gz",
  "Patient.Partridge_universal.app.tar.gz.sig",
  "Patient.Partridge_0.2.0_amd64.AppImage",
  "Patient.Partridge_0.2.0_amd64.AppImage.sig",
  "latest.json",
];

describe("problemsOf", () => {
  it("has nothing to say about a complete release", () => {
    expect(problemsOf(good, assets, "0.2.0")).toEqual([]);
  });

  it("notices a system that is missing (the three build jobs can overwrite each other's entries)", () => {
    const rest = Object.fromEntries(Object.entries(good.platforms).filter(([system]) => system !== "linux-x86_64"));
    const problems = problemsOf({ ...good, platforms: rest }, assets, "0.2.0");
    expect(problems).toEqual(["linux-x86_64 is missing from latest.json"]);
  });

  it("notices a missing signature, a missing asset, a wrong file and a wrong version", () => {
    const broken = {
      ...good,
      version: "0.1.0",
      notes: " ",
      platforms: {
        ...good.platforms,
        "windows-x86_64": { signature: "", url: `${base}/Patient.Partridge_0.2.0_x64-setup.exe` },
        "linux-x86_64": { signature: sig, url: `${base}/Patient.Partridge_0.2.0_amd64.deb` },
      },
    };
    const problems = problemsOf(broken, assets.filter((a) => !a.endsWith("exe.sig")), "0.2.0");
    expect(problems.join("\n")).toContain("latest.json says version 0.1.0");
    expect(problems.join("\n")).toContain("windows-x86_64 has no signature");
    expect(problems.join("\n")).toContain("Patient.Partridge_0.2.0_x64-setup.exe has no .sig asset");
    expect(problems.join("\n")).toContain("which is not an update file for it");
    expect(problems.join("\n")).toContain("no release notes");
  });

  it("refuses something that is not a manifest", () => {
    expect(problemsOf(null, [], "0.2.0")).toHaveLength(1);
  });
});

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { acceptedLicenses, group, licenseAllowed, npmEntries, render, rustEntries } from "./third-party-notices.mjs";

const accepted = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-3-Clause"];

describe("acceptedLicenses", () => {
  it("reads the list", () => {
    expect(acceptedLicenses('x = 1\naccepted = [\n  "MIT", # note\n  "Apache-2.0",\n]\n')).toEqual(["MIT", "Apache-2.0"]);
  });
  it("refuses a missing or empty list", () => {
    expect(() => acceptedLicenses("targets = []")).toThrow(/no `accepted`/);
    expect(() => acceptedLicenses("accepted = []")).toThrow(/empty/);
  });
});

describe("licenseAllowed", () => {
  it.each([
    ["MIT", true],
    ["MIT OR Apache-2.0", true],
    ["GPL-3.0-only OR MIT", true],
    ["GPL-3.0-only OR LGPL-2.1", false],
    ["MIT AND Apache-2.0", true],
    ["MIT AND GPL-3.0-only", false],
    ["(MIT OR GPL-3.0-only) AND BSD-3-Clause", true],
    ["(GPL-3.0-only OR LGPL-2.1) AND MIT", false],
    ["MIT OR GPL-3.0-only AND LGPL-2.1", true],
    ["Apache-2.0 WITH LLVM-exception", true],
    ["Apache-2.0 WITH Other-exception", false],
    ["(MIT)", true],
    ["", false],
    ["SEE LICENSE IN LICENSE.txt", false],
  ])("%s -> %s", (expression, expected) => {
    expect(licenseAllowed(expression, accepted)).toBe(expected);
  });
});

describe("rustEntries and group", () => {
  const report = (id, text, crates) => ({
    licenses: [{ id, text, used_by: crates.map(([name, version]) => ({ crate: { name, version, repository: `https://x/${name}` } })) }],
  });
  it("joins crates with the same text and keeps the order stable", () => {
    const a = report("MIT", "MIT text\r\n", [["zeta", "1.0.0"], ["alpha", "2.0.0"]]);
    const b = report("MIT", "MIT text", [["alpha", "2.0.0"]]);
    const c = report("Zlib", "Zlib text", [["flate", "0.1.0"]]);
    const groups = group(rustEntries([a, b, c]));
    expect(groups).toHaveLength(2);
    expect(groups[0].ids).toEqual(["MIT"]);
    expect(groups[0].packages.map(([label]) => label)).toEqual(["alpha 2.0.0", "zeta 1.0.0"]);
    expect(group(rustEntries([c, b, a]))).toEqual(groups);
  });
});

describe("npmEntries", () => {
  const dirs = [];
  afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));
  function modules(files) {
    const dir = mkdtempSync(join(tmpdir(), "notices-"));
    dirs.push(dir);
    for (const [path, content] of Object.entries(files)) {
      mkdirSync(join(dir, path, ".."), { recursive: true });
      writeFileSync(join(dir, path), content);
    }
    return dir;
  }
  const lock = (packages) => ({ packages: { "": {}, ...packages } });

  it("takes production packages with their licence files, and skips dev packages", () => {
    const dir = modules({ "a/LICENSE": "A licence", "a/package.json": "{}", "dev/LICENSE": "x" });
    const entries = npmEntries(
      lock({
        "node_modules/a": { version: "1.0.0", license: "MIT" },
        "node_modules/dev": { version: "1.0.0", license: "GPL-3.0-only", dev: true },
      }),
      accepted,
      dir,
    );
    expect(entries).toHaveLength(1);
    expect(entries[0].packages[0].label).toBe("a 1.0.0");
  });
  it("fails for a licence that is not allowed", () => {
    const dir = modules({ "a/LICENSE": "A licence" });
    expect(() => npmEntries(lock({ "node_modules/a": { version: "1.0.0", license: "GPL-3.0-only" } }), accepted, dir)).toThrow(
      /a 1.0.0 has the licence "GPL-3.0-only"/,
    );
  });
  it("fails for a package with no licence file", () => {
    const dir = modules({ "a/package.json": "{}" });
    expect(() => npmEntries(lock({ "node_modules/a": { version: "1.0.0", license: "MIT" } }), accepted, dir)).toThrow(/no licence file/);
  });
  it("reads scoped packages and several licence files", () => {
    const dir = modules({ "@s/p/LICENSE_MIT": "mit", "@s/p/LICENSE_APACHE-2.0": "apache", "@s/p/README.md": "no" });
    const entries = npmEntries(lock({ "node_modules/@s/p": { version: "2.0.0", license: "Apache-2.0 OR MIT" } }), accepted, dir);
    expect(entries.map((e) => e.text).sort()).toEqual(["apache", "mit"]);
  });
});

describe("render", () => {
  it("is the same for the same input and ends with one newline", () => {
    const rust = rustEntries([{ licenses: [{ id: "MIT", text: "MIT text", used_by: [{ crate: { name: "a", version: "1.0.0" } }] }] }]);
    const input = { rust, npm: [], fontLicense: "OFL text" };
    const text = render(input);
    expect(render(input)).toBe(text);
    expect(text.endsWith("\n")).toBe(true);
    expect(text.endsWith("\n\n")).toBe(false);
    expect(text).toContain("a 1.0.0");
    expect(text).toContain("OFL text");
    expect(text).toContain("pdfium-licenses/");
  });
});

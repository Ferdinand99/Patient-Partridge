import { readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
import { describe, expect, it } from "vitest";
import { downloadUrl, libraryFor, platformId, readTgz, sha256Hex, verifyChecksum } from "./pdfium.mjs";

const lock = JSON.parse(readFileSync(new URL("../pdfium.lock.json", import.meta.url), "utf8"));

/** Builds a .tgz in memory from [name, content] pairs. */
function makeTgz(entries) {
  const blocks = [];
  for (const [name, content] of entries) {
    const data = Buffer.from(content);
    const header = Buffer.alloc(512);
    header.write(name, 0, "utf8");
    header.write("0000644\0", 100);
    header.write(data.length.toString(8).padStart(11, "0") + "\0", 124);
    header.write("0", 156);
    blocks.push(header, data, Buffer.alloc((512 - (data.length % 512)) % 512));
  }
  blocks.push(Buffer.alloc(1024));
  return gzipSync(Buffer.concat(blocks));
}

describe("verifyChecksum", () => {
  const data = Buffer.from("some bytes");
  it("accepts the right checksum", () => {
    expect(() => verifyChecksum(data, sha256Hex(data))).not.toThrow();
  });

  it("refuses a different file", () => {
    expect(() => verifyChecksum(Buffer.from("other"), sha256Hex(data), "x.tgz")).toThrow(/mismatch/);
  });

  it("refuses a checksum that is not a SHA-256", () => {
    expect(() => verifyChecksum(data, "abc")).toThrow(/mismatch/);
    expect(() => verifyChecksum(data, "")).toThrow(/mismatch/);
  });
});

describe("readTgz", () => {
  const tgz = makeTgz([
    ["VERSION", "BUILD=8125\n"],
    ["lib/libpdfium.so", "library bytes"],
    ["include/fpdfview.h", "header"],
    ["licenses/pdfium.txt", "license"],
    ["../escape.txt", "bad"],
  ]);

  it("takes out only the wanted files", () => {
    const files = readTgz(tgz, ["lib/libpdfium.so", "VERSION", "licenses/"]);
    expect([...files.keys()].sort()).toEqual(["VERSION", "lib/libpdfium.so", "licenses/pdfium.txt"]);
    expect(files.get("lib/libpdfium.so").toString()).toBe("library bytes");
  });

  it("never returns a path that climbs out of the folder", () => {
    const files = readTgz(tgz, ["../escape.txt"]);
    expect(files.size).toBe(0);
  });

  it("rejects a damaged archive", () => {
    const tar = Buffer.alloc(512);
    tar.write("lib/libpdfium.so", 0);
    tar.write("77777777777\0", 124);
    expect(() => readTgz(gzipSync(tar), ["lib/libpdfium.so"])).toThrow(/damaged/);
  });
});

describe("platforms", () => {
  it("maps Node platform names to ids", () => {
    expect(platformId("win32", "x64")).toBe("win-x64");
    expect(platformId("darwin", "arm64")).toBe("mac-arm64");
    expect(platformId("linux", "x64")).toBe("linux-x64");
    expect(() => platformId("freebsd", "x64")).toThrow();
  });

  it("knows where the library is for each platform", () => {
    expect(libraryFor("win-x64")).toEqual({ inArchive: "bin/pdfium.dll", file: "pdfium.dll" });
    expect(libraryFor("mac-arm64").file).toBe("libpdfium.dylib");
    expect(libraryFor("linux-arm64").file).toBe("libpdfium.so");
  });
});

describe("pdfium.lock.json", () => {
  it("pins every platform the project builds for, with a real checksum", () => {
    for (const id of ["linux-x64", "win-x64", "mac-arm64", "mac-x64"]) {
      const entry = lock.platforms[id];
      expect(entry, id).toBeDefined();
      expect(entry.sha256, id).toMatch(/^[0-9a-f]{64}$/);
      expect(entry.asset, id).toBe(`pdfium-${id}.tgz`);
    }
  });

  it("uses the plain build, not the one with a JavaScript engine", () => {
    for (const entry of Object.values(lock.platforms)) {
      expect(entry.asset).not.toContain("v8");
    }
  });

  it("belongs to the same PDFium release as the pdfium-render feature", () => {
    const cargo = readFileSync(new URL("../crates/engine/Cargo.toml", import.meta.url), "utf8");
    expect(cargo).toContain(`"pdfium_${lock.build}"`);
    expect(lock.release).toBe(`chromium/${lock.build}`);
  });

  it("downloads from the official release page", () => {
    expect(downloadUrl(lock, "pdfium-win-x64.tgz")).toBe(
      "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F8125/pdfium-win-x64.tgz",
    );
  });
});

import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { checkPaths, findLibraries } from "./check-pdfium-v8.mjs";
import { findV8Markers } from "./pdfium.mjs";

const dirs = [];
function folder() {
  const dir = mkdtempSync(join(tmpdir(), "pdfium-v8-"));
  dirs.push(dir);
  return dir;
}
afterEach(() => {
  vi.restoreAllMocks();
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
});

describe("findV8Markers", () => {
  it("accepts a library without V8", () => {
    expect(findV8Markers(Buffer.from("FPDF_InitLibrary\0FPDF_LoadDocument\0FORM_OnLButtonDown"))).toEqual([]);
  });
  it("finds the V8 symbols of a Linux or macOS build", () => {
    expect(findV8Markers(Buffer.from("x_ZN2v88internal6TaggedIx"))).toEqual(["_ZN2v8"]);
  });
  it("finds the V8 symbols of a Windows build", () => {
    expect(findV8Markers(Buffer.from("?As_BigInt@internal@v8@@YA"))).toEqual(["@v8@"]);
  });
  it("finds the V8 flags export", () => {
    expect(findV8Markers(Buffer.from("FPDF_GetRecommendedV8Flags"))).toEqual(["V8Flags"]);
  });
});

describe("checkPaths", () => {
  it("passes a plain library and finds it in a subfolder", () => {
    const dir = folder();
    mkdirSync(join(dir, "usr", "lib"), { recursive: true });
    writeFileSync(join(dir, "usr", "lib", "libpdfium.so"), "FPDF_InitLibrary");
    vi.spyOn(console, "log").mockImplementation(() => {});
    expect(checkPaths([dir])).toEqual([]);
  });
  it("fails a library with V8", () => {
    const dir = folder();
    writeFileSync(join(dir, "pdfium.dll"), "FPDF_InitLibrary?x@v8@@");
    const problems = checkPaths([dir]);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("contains V8");
  });
  it("fails an empty or foreign file named like the library", () => {
    const dir = folder();
    writeFileSync(join(dir, "pdfium.dll"), "");
    const problems = checkPaths([dir]);
    expect(problems).toHaveLength(1);
    expect(problems[0]).toContain("does not look like a PDFium library");
  });
  it("fails when no library is there", () => {
    const dir = folder();
    writeFileSync(join(dir, "other.txt"), "x");
    expect(checkPaths([dir])[0]).toContain("No PDFium library found");
  });
  it("only counts the known library names", () => {
    const dir = folder();
    writeFileSync(join(dir, "libpdfium.so"), "FPDF_InitLibrary");
    writeFileSync(join(dir, "libother.so"), "x");
    expect(findLibraries(dir)).toHaveLength(1);
  });
});

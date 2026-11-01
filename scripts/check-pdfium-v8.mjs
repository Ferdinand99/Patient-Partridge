#!/usr/bin/env node
// Fails if a PDFium library that ships in the app contains the V8 JavaScript engine. PDF
// JavaScript must stay off, and the way to be sure is to ship a build that has no engine for it.
// pdfium.lock.json only pins the asset name, so a wrong asset ("pdfium-v8-...") would otherwise
// pass unnoticed.
//
// Usage: node scripts/check-pdfium-v8.mjs <folder or library file>...
// Looks in each folder (recursively) for libpdfium.so, libpdfium.dylib and pdfium.dll, and fails
// if it finds none, so that a check pointed at the wrong folder can not pass.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { basename, join } from "node:path";
import { fileURLToPath } from "node:url";
import { findV8Markers } from "./pdfium.mjs";

const LIBRARY_NAMES = new Set(["libpdfium.so", "libpdfium.dylib", "pdfium.dll"]);

/** All PDFium library files at or below a path. */
export function findLibraries(path) {
  const info = statSync(path);
  if (info.isFile()) return LIBRARY_NAMES.has(basename(path)) ? [path] : [];
  return readdirSync(path, { withFileTypes: true }).flatMap((entry) =>
    entry.isSymbolicLink() ? [] : findLibraries(join(path, entry.name)),
  );
}

/** Returns a list of problems; an empty list means every library found is a plain build. */
export function checkPaths(paths) {
  const libraries = paths.flatMap(findLibraries);
  if (libraries.length === 0) return [`No PDFium library found in: ${paths.join(", ")}`];
  const problems = [];
  for (const library of libraries) {
    const found = findV8Markers(readFileSync(library));
    if (found.length > 0) problems.push(`${library} contains V8 (${found.join(", ")})`);
    else console.log(`${library}: no V8`);
  }
  return problems;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const paths = process.argv.slice(2);
  if (paths.length === 0) {
    console.error("Usage: node scripts/check-pdfium-v8.mjs <folder or library file>...");
    process.exit(2);
  }
  const problems = checkPaths(paths);
  for (const p of problems) console.error(`ERROR: ${p}`);
  process.exit(problems.length === 0 ? 0 : 1);
}

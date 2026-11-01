#!/usr/bin/env node
// Downloads the pinned PDFium build for this platform into vendor/pdfium/<platform>/ and checks
// its SHA-256 against pdfium.lock.json first. Usage: node scripts/fetch-pdfium.mjs [platform-id]
//
// The engine loads the library from that folder. Nothing else downloads code at run time.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { downloadUrl, findV8Markers, libraryFor, platformId, readTgz, sha256Hex, verifyChecksum } from "./pdfium.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

export async function fetchPdfium(id = platformId()) {
  const lock = JSON.parse(readFileSync(join(root, "pdfium.lock.json"), "utf8"));
  const entry = lock.platforms[id];
  if (!entry) throw new Error(`No PDFium build is pinned for ${id}.`);
  const target = join(root, "vendor", "pdfium", id);
  const lib = libraryFor(id);
  const stamp = join(target, ".sha256");

  // Reuse what is there only if the library on disk is still the one that was unpacked from the
  // checked download: its own hash is kept next to it, and it is hashed again here.
  if (existsSync(join(target, lib.file)) && existsSync(stamp)) {
    try {
      const saved = JSON.parse(readFileSync(stamp, "utf8"));
      if (
        saved.archive === entry.sha256 &&
        saved.library === sha256Hex(readFileSync(join(target, lib.file)))
      ) {
        console.log(`PDFium ${lock.release} for ${id} is already in ${target}`);
        return target;
      }
    } catch {
      // A stamp that cannot be read means: download again.
    }
  }

  const url = downloadUrl(lock, entry.asset);
  console.log(`Downloading ${url}`);
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Download failed: ${response.status} ${response.statusText}`);
  const tgz = Buffer.from(await response.arrayBuffer());
  verifyChecksum(tgz, entry.sha256, entry.asset);
  console.log(`Checksum ok (${sha256Hex(tgz).slice(0, 12)}...)`);

  const files = readTgz(tgz, [lib.inArchive, "VERSION", "LICENSE", "licenses/"]);
  if (!files.has(lib.inArchive)) throw new Error(`${lib.inArchive} is missing from ${entry.asset}.`);
  // The lock pins an asset name only. A V8 build ("pdfium-v8-...") has the checksum of whatever
  // was pinned, so look inside: PDF JavaScript must stay off, and a library without V8 is the
  // way to be sure.
  const v8 = findV8Markers(files.get(lib.inArchive));
  if (v8.length > 0) throw new Error(`${entry.asset} contains the V8 JavaScript engine (${v8.join(", ")}). Nothing was unpacked.`);
  mkdirSync(target, { recursive: true });
  for (const [name, data] of files) {
    const out = name === lib.inArchive ? join(target, lib.file) : join(target, name);
    mkdirSync(dirname(out), { recursive: true });
    writeFileSync(out, data);
  }
  const library = files.get(lib.inArchive);
  writeFileSync(stamp, JSON.stringify({ archive: entry.sha256, library: sha256Hex(library) }));
  console.log(`Unpacked into ${target}`);
  return target;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  fetchPdfium(process.argv[2]).catch((e) => {
    console.error(e.message);
    process.exit(1);
  });
}

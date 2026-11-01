// Shared helpers for fetching and checking the PDFium binaries. No dependencies.
// The download is checked against the pinned SHA-256 BEFORE anything is unpacked, and only a
// short list of known file names is taken out of the archive, so a bad archive can not write
// files anywhere else.

import { createHash } from "node:crypto";
import { gunzipSync } from "node:zlib";

/** Folder name of a platform, as used in pdfium.lock.json and vendor/pdfium. */
export function platformId(platform = process.platform, arch = process.arch) {
  const os = { win32: "win", darwin: "mac", linux: "linux" }[platform];
  const cpu = { x64: "x64", arm64: "arm64" }[arch];
  if (!os || !cpu) throw new Error(`No PDFium build is pinned for ${platform}/${arch}`);
  return `${os}-${cpu}`;
}

/** Path of the library inside the downloaded archive, and its file name on disk. */
export function libraryFor(id) {
  if (id.startsWith("win-")) return { inArchive: "bin/pdfium.dll", file: "pdfium.dll" };
  if (id.startsWith("mac-")) return { inArchive: "lib/libpdfium.dylib", file: "libpdfium.dylib" };
  return { inArchive: "lib/libpdfium.so", file: "libpdfium.so" };
}

export function sha256Hex(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

/** Throws unless the buffer has exactly the pinned checksum. */
export function verifyChecksum(buffer, expected, label = "download") {
  const actual = sha256Hex(buffer);
  if (!/^[0-9a-f]{64}$/.test(expected) || actual !== expected.toLowerCase()) {
    throw new Error(
      `Checksum mismatch for ${label}: expected ${expected}, got ${actual}. Nothing was unpacked.`,
    );
  }
}

export function downloadUrl(lock, asset) {
  const tag = encodeURIComponent(lock.release);
  return `https://github.com/${lock.repository}/releases/download/${tag}/${asset}`;
}

/**
 * Reads the files named in `wanted` (exact paths, or a prefix ending in "/") out of a .tgz.
 * Returns a Map from archive path to Buffer. Everything else in the archive is ignored.
 */
export function readTgz(tgz, wanted) {
  const tar = gunzipSync(tgz);
  const files = new Map();
  let offset = 0;
  while (offset + 512 <= tar.length) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((b) => b === 0)) break;
    const text = (from, to) => header.subarray(from, to).toString("utf8").replace(/\0.*$/s, "");
    const size = parseInt(text(124, 136).trim() || "0", 8);
    const type = String.fromCharCode(header[156] || 48);
    const prefix = text(345, 500);
    let name = (prefix ? `${prefix}/` : "") + text(0, 100);
    name = name.replace(/^\.\//, "");
    if (!Number.isFinite(size) || size < 0) throw new Error("The archive is damaged.");
    const dataStart = offset + 512;
    const dataEnd = dataStart + size;
    if (dataEnd > tar.length) throw new Error("The archive is damaged.");
    const isFile = type === "0" || type === "\0";
    const wantedHere = wanted.some((w) => (w.endsWith("/") ? name.startsWith(w) : name === w));
    if (isFile && wantedHere && !name.includes("..")) {
      files.set(name, Buffer.from(tar.subarray(dataStart, dataEnd)));
    }
    offset = dataStart + Math.ceil(size / 512) * 512;
  }
  return files;
}

/**
 * Byte strings that only a PDFium build with the V8 JavaScript engine contains. A V8 build
 * exports the C++ symbols of V8 (`_ZN2v8...` on Linux and macOS, `...@v8@...` on Windows) and
 * `FPDF_GetRecommendedV8Flags`; the plain build contains none of them (checked on the Linux,
 * macOS and Windows builds of chromium/8125). The symbol names sit in the library's string
 * tables in all three file formats, so one byte search covers all three systems.
 */
export const V8_MARKERS = ["_ZN2v8", "@v8@", "v8::", "V8Flags"];

/** Returns the V8 markers found in a library file, or an empty list for a plain build. */
export function findV8Markers(library) {
  return V8_MARKERS.filter((marker) => library.includes(marker));
}

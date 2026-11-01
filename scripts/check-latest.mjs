// Checks latest.json before a release is published. The release is a draft until this passes, so
// nobody gets a release that some systems cannot update from.
//
//   node scripts/check-latest.mjs latest.json assets.txt 0.2.0
//
// `assets.txt` has the names of the assets of the release, one per line.

import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** What every release must be able to update: system, and what its update file looks like. */
export const REQUIRED = {
  "windows-x86_64": /\.(exe|msi)$/i,
  "darwin-aarch64": /\.app\.tar\.gz$/,
  "darwin-x86_64": /\.app\.tar\.gz$/,
  "linux-x86_64": /\.AppImage(\.tar\.gz)?$/,
};

/** The file name at the end of an address, or null when it is not an address. */
function fileOf(address) {
  try {
    return decodeURIComponent(new URL(address).pathname.split("/").pop() ?? "");
  } catch {
    return null;
  }
}

/** The problems of a manifest, as sentences. An empty list means it is fine. */
export function problemsOf(latest, assetNames, version) {
  const problems = [];
  if (!latest || typeof latest !== "object") return ["latest.json is not an object"];
  if (String(latest.version ?? "").replace(/^v/, "") !== version) {
    problems.push(`latest.json says version ${latest.version}, the release is ${version}`);
  }
  const names = new Set(assetNames);
  for (const [system, pattern] of Object.entries(REQUIRED)) {
    const entry = latest.platforms?.[system];
    if (!entry) {
      problems.push(`${system} is missing from latest.json`);
      continue;
    }
    if (typeof entry.signature !== "string" || entry.signature.trim().length < 20) {
      problems.push(`${system} has no signature`);
    }
    const file = fileOf(entry.url);
    if (file === null) {
      problems.push(`${system} has no usable address`);
      continue;
    }
    if (!pattern.test(file)) problems.push(`${system} points to ${file}, which is not an update file for it`);
    if (!names.has(file)) problems.push(`${system} points to ${file}, which is not an asset of the release`);
    if (!names.has(`${file}.sig`)) problems.push(`${file} has no .sig asset next to it`);
  }
  if (!String(latest.notes ?? "").trim()) problems.push("latest.json has no release notes");
  return problems;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [, , latestPath, assetsPath, version] = process.argv;
  if (!latestPath || !assetsPath || !version) {
    console.error("usage: check-latest.mjs latest.json assets.txt <version>");
    process.exit(2);
  }
  const assets = readFileSync(assetsPath, "utf8").split("\n").map((l) => l.trim()).filter(Boolean);
  const problems = problemsOf(JSON.parse(readFileSync(latestPath, "utf8")), assets, version);
  for (const p of problems) console.error(`::error title=latest.json::${p}`);
  if (problems.length) process.exit(1);
  console.log("latest.json lists every system with a signature and an asset.");
}

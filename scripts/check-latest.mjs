// Checks latest.json before a release is published. The release is a draft until this passes, so
// nobody gets a release that some systems cannot update from.
//
//   node scripts/check-latest.mjs latest.json assets.json 0.2.0
//
// `assets.json` is the list of the assets of the release as the GitHub API gives it: an array of
// { id, name } (`gh api repos/<repo>/releases/<id> --jq '[.assets[] | {id, name}]'`).
//
// tauri-action writes the address of an update file as an API address with the number of the asset
// at the end (`https://api.github.com/repos/<owner>/<repo>/releases/assets/686300632`), not as a
// file name. The check follows that number to the asset; an address that ends in a file name (a
// "browser_download_url") is followed by name.

import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** What every release must be able to update: system, and what its update file looks like. */
export const REQUIRED = {
  // A copy asks for the entry of the installer it came from, and for the plain one only when it
  // does not know its installer (the plain entry holds whichever of the two tauri-action listed first).
  "windows-x86_64": /\.(exe|msi)$/i,
  "windows-x86_64-nsis": /-setup\.exe$/i,
  "windows-x86_64-msi": /\.msi$/i,
  "darwin-aarch64": /\.app\.tar\.gz$/,
  "darwin-x86_64": /\.app\.tar\.gz$/,
  "linux-x86_64": /\.AppImage(\.tar\.gz)?$/,
};

/** The asset that an address of latest.json points to, or null. */
export function assetOf(address, assets) {
  let url;
  try {
    url = new URL(address);
  } catch {
    return null;
  }
  const last = decodeURIComponent(url.pathname.split("/").pop() ?? "");
  if (/\/releases\/assets\/\d+$/.test(url.pathname)) {
    return assets.find((a) => String(a.id) === last) ?? null;
  }
  return assets.find((a) => a.name === last) ?? null;
}

/** The problems of a manifest, as sentences. An empty list means it is fine. */
export function problemsOf(latest, assets, version, required = REQUIRED) {
  const problems = [];
  if (!latest || typeof latest !== "object") return ["latest.json is not an object"];
  if (String(latest.version ?? "").replace(/^v/, "") !== version) {
    problems.push(`latest.json says version ${latest.version}, the release is ${version}`);
  }
  const names = new Set(assets.map((a) => a.name));
  for (const [system, pattern] of Object.entries(required)) {
    const entry = latest.platforms?.[system];
    if (!entry) {
      problems.push(`${system} is missing from latest.json`);
      continue;
    }
    if (typeof entry.signature !== "string" || entry.signature.trim().length < 20) {
      problems.push(`${system} has no signature`);
    }
    if (typeof entry.url !== "string") {
      problems.push(`${system} has no usable address`);
      continue;
    }
    const asset = assetOf(entry.url, assets);
    if (!asset) {
      problems.push(`${system} points to ${entry.url}, which is not an asset of the release`);
      continue;
    }
    if (!pattern.test(asset.name)) problems.push(`${system} points to ${asset.name}, which is not an update file for it`);
    if (!names.has(`${asset.name}.sig`)) problems.push(`${asset.name} has no .sig asset next to it`);
  }
  if (!String(latest.notes ?? "").trim()) problems.push("latest.json has no release notes");
  // Several entries can name the same file (the plain Windows entry and the one of its installer).
  return [...new Set(problems)];
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [, , latestPath, assetsPath, version] = process.argv;
  if (!latestPath || !assetsPath || !version) {
    console.error("usage: check-latest.mjs latest.json assets.json <version>");
    process.exit(2);
  }
  const assets = JSON.parse(readFileSync(assetsPath, "utf8"));
  const problems = problemsOf(JSON.parse(readFileSync(latestPath, "utf8")), assets, version);
  for (const p of problems) console.error(`::error title=latest.json::${p}`);
  if (problems.length) process.exit(1);
  console.log("latest.json lists every system with a signature and an asset.");
}

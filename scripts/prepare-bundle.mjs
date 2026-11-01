#!/usr/bin/env node
// Prepares an installer build: fetches PDFium (checked), builds the engine in release mode,
// puts it where Tauri expects a sidecar, and writes a config that adds the engine and the PDFium
// library to the bundle. Used by `npm run bundle` and by CI.
//
// The plain `cargo` and `tauri dev` builds do not need any of this, which is why it lives in a
// separate generated config instead of tauri.conf.json.

import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { fetchPdfium } from "./fetch-pdfium.mjs";
import { libraryFor, platformId } from "./pdfium.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const shell = process.platform === "win32";

function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", shell });
  if (result.status !== 0) process.exit(result.status ?? 1);
}

const id = platformId();
const libraryDir = await fetchPdfium(id);
const lib = libraryFor(id);

run("cargo", ["build", "--release", "-p", "partridge-engine", "--locked"]);

const host = spawnSync("rustc", ["-vV"], { encoding: "utf8", shell }).stdout;
const triple = /^host: (.+)$/m.exec(host)?.[1]?.trim();
if (!triple) throw new Error("Could not read the Rust host triple from `rustc -vV`.");

const exe = process.platform === "win32" ? ".exe" : "";
const binaries = join(root, "src-tauri", "binaries");
mkdirSync(binaries, { recursive: true });
copyFileSync(join(root, "target", "release", `partridge-engine${exe}`), join(binaries, `partridge-engine-${triple}${exe}`));

// Paths in `resources` are relative to src-tauri. The map form sets where each file lands.
const config = {
  bundle: {
    externalBin: ["binaries/partridge-engine"],
    resources: {
      [`../vendor/pdfium/${id}/${lib.file}`]: lib.file,
      [`../vendor/pdfium/${id}/LICENSE`]: "pdfium-LICENSE",
      [`../vendor/pdfium/${id}/licenses`]: "pdfium-licenses",
    },
  },
};
writeFileSync(join(root, "src-tauri", "tauri.bundle.generated.conf.json"), JSON.stringify(config, null, 2));
console.log(`Bundle config written for ${id} (${triple}); library from ${libraryDir}`);

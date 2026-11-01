#!/usr/bin/env node
// Prepares an installer build: fetches PDFium (checked), builds the engine in release mode,
// puts it where Tauri expects a sidecar, and writes a config that adds the engine and the PDFium
// library to the bundle. Used by `npm run bundle`, by CI and by the release workflow.
//
// The plain `cargo` and `tauri dev` builds do not need any of this, which is why it lives in a
// separate generated config instead of tauri.conf.json.
//
// A universal macOS app (PARTRIDGE_UNIVERSAL=1, on macOS) needs one engine and one library that
// run on both kinds of Mac, so each is built or fetched for both and joined with `lipo`. Both
// PDFium libraries come from the pinned, checksum-checked downloads; the joined library is made
// here and never downloaded. The result is built with `tauri build --target universal-apple-darwin`.
//
// Nothing here needs a secret, and none is wanted: this script runs the build scripts of every
// dependency of the engine. The macOS signing of the PDFium library (Tauri signs the engine and the
// app, but not files in `resources`) is done later, in scripts/release-build.mjs.

import { spawnSync } from "node:child_process";
import { copyFileSync, cpSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { MAC_TRIPLES, UNIVERSAL_TRIPLE, bundleConfig } from "./bundle-config.mjs";
import { fetchPdfium } from "./fetch-pdfium.mjs";
import { libraryFor, platformId } from "./pdfium.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const shell = process.platform === "win32";
const universal = process.env.PARTRIDGE_UNIVERSAL === "1";
const targetDir = process.env.CARGO_TARGET_DIR ?? join(root, "target");

function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", shell });
  if (result.status !== 0) process.exit(result.status ?? 1);
}

const exe = process.platform === "win32" ? ".exe" : "";
const binaries = join(root, "src-tauri", "binaries");
mkdirSync(binaries, { recursive: true });

let folder;
let file;
let description;

if (universal) {
  if (process.platform !== "darwin") throw new Error("A universal app can only be built on macOS.");
  const arm = await fetchPdfium("mac-arm64");
  const x64 = await fetchPdfium("mac-x64");
  const lib = libraryFor("mac-arm64");
  folder = "mac-universal";
  file = lib.file;
  const out = join(root, "vendor", "pdfium", folder);
  mkdirSync(out, { recursive: true });
  run("lipo", ["-create", "-output", join(out, file), join(arm, file), join(x64, file)]);
  // The licences are the same for both builds.
  copyFileSync(join(arm, "LICENSE"), join(out, "LICENSE"));
  cpSync(join(arm, "licenses"), join(out, "licenses"), { recursive: true });

  // The repository pins its Rust version (rust-toolchain.toml); the targets belong to that version,
  // which a plain "install these targets" step of a workflow does not touch.
  run("rustup", ["target", "add", ...MAC_TRIPLES]);
  for (const triple of MAC_TRIPLES) {
    run("cargo", ["build", "--release", "-p", "partridge-engine", "--locked", "--target", triple]);
    copyFileSync(
      join(targetDir, triple, "release", "partridge-engine"),
      join(binaries, `partridge-engine-${triple}`),
    );
  }
  run("lipo", [
    "-create",
    "-output",
    join(binaries, `partridge-engine-${UNIVERSAL_TRIPLE}`),
    ...MAC_TRIPLES.map((t) => join(binaries, `partridge-engine-${t}`)),
  ]);
  description = `universal macOS (${MAC_TRIPLES.join(" + ")})`;
} else {
  const id = platformId();
  const libraryDir = await fetchPdfium(id);
  folder = id;
  file = libraryFor(id).file;
  run("cargo", ["build", "--release", "-p", "partridge-engine", "--locked"]);
  const host = spawnSync("rustc", ["-vV"], { encoding: "utf8", shell }).stdout;
  const triple = /^host: (.+)$/m.exec(host)?.[1]?.trim();
  if (!triple) throw new Error("Could not read the Rust host triple from `rustc -vV`.");
  copyFileSync(
    join(targetDir, "release", `partridge-engine${exe}`),
    join(binaries, `partridge-engine-${triple}${exe}`),
  );
  description = `${id} (${triple}); library from ${libraryDir}`;
}

const config = bundleConfig({ folder, file, env: process.env });
writeFileSync(join(root, "src-tauri", "tauri.bundle.generated.conf.json"), JSON.stringify(config, null, 2));
console.log(
  `Bundle config written for ${description}; the bundler makes the update file: ${config.bundle.createUpdaterArtifacts ? "yes" : "no"}.`,
);

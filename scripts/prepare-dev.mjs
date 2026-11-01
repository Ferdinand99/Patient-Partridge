#!/usr/bin/env node
// Gets a source checkout ready for `npm run tauri dev`: the PDFium library for this platform
// (checked against its pinned checksum) and a debug build of the engine next to the app.

import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { fetchPdfium } from "./fetch-pdfium.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

await fetchPdfium();
const build = spawnSync("cargo", ["build", "-p", "partridge-engine"], {
  cwd: root,
  stdio: "inherit",
  shell: process.platform === "win32",
});
process.exit(build.status ?? 1);

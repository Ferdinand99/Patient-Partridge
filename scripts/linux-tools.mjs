#!/usr/bin/env node
// Puts the tools that make an AppImage where the Tauri bundler looks for them, each one checked
// against the SHA-256 in linux-tools.lock.json before it is used. The bundler downloads a missing
// tool without any check; with the files in place it downloads nothing.
//
//   node scripts/linux-tools.mjs [folder]     (default: the cache folder of Tauri, ~/.cache/tauri)

import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { isMain } from "./is-main.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

/** Throws unless `bytes` has exactly the pinned checksum. */
export function verify(bytes, tool) {
  if (!/^[0-9a-f]{64}$/.test(tool.sha256) || sha256(bytes) !== tool.sha256) {
    throw new Error(`Checksum mismatch for ${tool.file} (from ${tool.url}): expected ${tool.sha256}, got ${sha256(bytes)}. It was not used.`);
  }
}

/** The folder where Tauri keeps its tools: $XDG_CACHE_HOME/tauri or ~/.cache/tauri. */
export function toolsFolder(env = process.env) {
  return join(env.XDG_CACHE_HOME || join(homedir(), ".cache"), "tauri");
}

/**
 * Makes sure every pinned tool is in `folder` with the pinned content. A file that is there but
 * differs is replaced (the bundler changes bytes of linuxdeploy after its first use), one that
 * cannot be verified after the download stops the run. `fetchBytes` is a parameter for tests.
 */
export async function install(lock, folder, fetchBytes) {
  mkdirSync(folder, { recursive: true });
  for (const tool of lock.tools) {
    const target = join(folder, tool.file);
    if (existsSync(target)) {
      try {
        verify(readFileSync(target), tool);
        console.log(`${tool.file}: already there, checksum ok`);
        continue;
      } catch {
        // Not the pinned file (or changed by an earlier run): fetch it again.
      }
    }
    const bytes = await fetchBytes(tool.url);
    verify(bytes, tool);
    writeFileSync(target, bytes);
    chmodSync(target, 0o755);
    console.log(`${tool.file}: downloaded, checksum ok`);
  }
}

async function download(url) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Download failed: ${url}: ${response.status} ${response.statusText}`);
  return Buffer.from(await response.arrayBuffer());
}

if (isMain(import.meta.url)) {
  const lock = JSON.parse(readFileSync(join(root, "scripts", "linux-tools.lock.json"), "utf8"));
  install(lock, process.argv[2] ?? toolsFolder(), download).catch((error) => {
    console.error(`::error title=AppImage tools::${error.message}`);
    process.exit(1);
  });
}

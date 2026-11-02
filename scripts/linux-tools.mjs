#!/usr/bin/env node
// Puts the tools that make an AppImage where the Tauri bundler looks for them, each one checked
// against the SHA-256 in linux-tools.lock.json before it is used. The bundler downloads a missing
// tool without any check; with the files in place it downloads nothing.
//
//   node scripts/linux-tools.mjs [folder]     (default: the cache folder of Tauri, ~/.cache/tauri)
//   node scripts/linux-tools.mjs --check      (only compare what is online with the pins; writes nothing)

import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { isMain } from "./is-main.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

export const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

/** What to say when a tool is not the pinned file. A tool from a rolling release gets the likely reason and the way out. */
export function mismatchMessage(tool, got) {
  const plain = `Checksum mismatch for ${tool.file} (from ${tool.url}): expected ${tool.sha256}, got ${got}. It was not used.`;
  if (!tool.rolling) return plain;
  return (
    `${plain} ${tool.file} comes from a rolling release that its authors replace from time to time, so the file online is probably a newer build, ` +
    `not a damaged download. Nothing was built with it. Download the new file, read what changed, and if it is fine put its SHA-256 in scripts/linux-tools.lock.json. ` +
    `Do this before the release PR is merged: the Linux build runs last, after the macOS app is notarised, and a tag cannot take a new pin.`
  );
}

/** Throws unless `bytes` has exactly the pinned checksum. */
export function verify(bytes, tool) {
  if (!/^[0-9a-f]{64}$/.test(tool.sha256) || sha256(bytes) !== tool.sha256) {
    throw new Error(mismatchMessage(tool, sha256(bytes)));
  }
}

/**
 * Downloads every pinned tool and compares it with its pin, without writing anything. Gives one
 * sentence for each tool that differs or cannot be downloaded (an empty list means all is well).
 * Run before a release so that a replaced tool is found while there is time to look at it.
 */
export async function check(lock, fetchBytes) {
  const problems = [];
  for (const tool of lock.tools) {
    try {
      verify(await fetchBytes(tool.url), tool);
    } catch (error) {
      problems.push(error.message);
    }
  }
  return problems;
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
  if (process.argv[2] === "--check") {
    const problems = await check(lock, download);
    for (const p of problems) console.error(`::error title=AppImage tools::${p}`);
    if (problems.length) process.exit(1);
    console.log(`All ${lock.tools.length} AppImage tools are the pinned files.`);
    process.exit(0);
  }
  install(lock, process.argv[2] ?? toolsFolder(), download).catch((error) => {
    console.error(`::error title=AppImage tools::${error.message}`);
    process.exit(1);
  });
}

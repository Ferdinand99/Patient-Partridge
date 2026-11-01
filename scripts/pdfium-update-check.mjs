#!/usr/bin/env node
// Compares the PDFium build pinned in pdfium.lock.json with the latest release of
// bblanchon/pdfium-binaries, and writes the result for the "PDFium update check" workflow.
//
// Usage: node scripts/pdfium-update-check.mjs
// Reads GITHUB_TOKEN (optional, avoids rate limits), writes GITHUB_OUTPUT and GITHUB_STEP_SUMMARY
// when they are set, and prints the result.
//
// Exit codes: 0 = the check ran (also when a newer release exists: that is reported through the
// outputs, and the workflow opens one issue for it); 1 = the check could not run, or the lock and
// the pdfium-render feature in crates/engine/Cargo.toml disagree. Both fail the workflow visibly.

import { appendFileSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { isMain } from "./is-main.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const TAG = /^chromium\/(\d+)$/;

/** The build number of a release tag such as "chromium/8125". */
export function buildOfTag(tag) {
  const match = TAG.exec(tag ?? "");
  if (!match) throw new Error(`Unexpected release tag "${tag}": expected chromium/<number>.`);
  return Number(match[1]);
}

/** The `pdfium_NNNN` feature of pdfium-render that a Cargo.toml asks for, or null. */
export function featureOf(cargoToml) {
  const found = [...cargoToml.matchAll(/\bpdfium_(\d+)\b/g)].map((m) => Number(m[1]));
  return found.length === 1 ? found[0] : null;
}

/** Facts about the pinned build and the latest release. `latestTag` must look like chromium/N. */
export function compare(lock, latestTag, cargoToml) {
  const latest = buildOfTag(latestTag);
  const feature = featureOf(cargoToml);
  return {
    current: lock.build,
    latest,
    latestTag,
    newer: latest > lock.build,
    feature,
    featureMatchesLock: feature === lock.build,
  };
}

/** A plain version number such as 0.9.5. Anything else from the network is left out. */
export function plainVersion(text) {
  return typeof text === "string" && /^\d{1,4}\.\d{1,4}\.\d{1,4}$/.test(text) ? text : null;
}

export function issueTitle(latestTag) {
  return `PDFium update available: ${latestTag}`;
}

/**
 * Text of the issue. Built only from checked values: numbers, a tag that matches TAG, and a
 * crate version that matches plainVersion. Nothing else from the network gets in.
 */
export function issueBody(result, lock, crate) {
  const { current, latest, latestTag } = result;
  const lines = [
    "bblanchon/pdfium-binaries has a newer PDFium build than the one Patient Partridge ships.",
    "",
    `- Pinned in \`pdfium.lock.json\`: build ${current} (\`${lock.release}\`)`,
    `- Latest release: build ${latest} (\`${latestTag}\`), https://github.com/${lock.repository}/releases/tag/${latestTag}`,
  ];
  const crateVersion = crate ? plainVersion(crate.version) : null;
  if (crate && crateVersion) {
    const newest = Number.isSafeInteger(crate.newestFeature) ? crate.newestFeature : null;
    lines.push(
      `- pdfium-render ${crateVersion} has features up to build ${newest ?? "unknown"}` +
        (newest !== null && newest < latest
          ? `. It has no feature for build ${latest} yet: check that the library still works with the newest feature, or wait for a new pdfium-render.`
          : "."),
    );
  }
  lines.push(
    "",
    "To update, follow docs/pdfium-updates.md. Close this issue when the update is merged, or when you decide to skip this build; the workflow opens one issue per release and does not repeat it.",
  );
  return lines.join("\n");
}

async function getJson(url, headers = {}) {
  const response = await fetch(url, { headers: { "User-Agent": "patient-partridge-pdfium-check", ...headers } });
  if (!response.ok) throw new Error(`${url}: ${response.status} ${response.statusText}`);
  return response.json();
}

async function latestRelease(repository) {
  const headers = { Accept: "application/vnd.github+json" };
  if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const release = await getJson(`https://api.github.com/repos/${repository}/releases/latest`, headers);
  return release.tag_name;
}

/** Best effort: which PDFium builds does the newest pdfium-render know? Never fails the check. */
async function crateInfo() {
  try {
    const crate = await getJson("https://crates.io/api/v1/crates/pdfium-render");
    const newest = crate.versions[0];
    const builds = Object.keys(newest.features)
      .map((name) => /^pdfium_(\d+)$/.exec(name)?.[1])
      .filter(Boolean)
      .map(Number);
    return { version: newest.num, newestFeature: builds.length ? Math.max(...builds) : null };
  } catch (error) {
    console.log(`Could not read pdfium-render from crates.io (${error.message}); continuing without it.`);
    return null;
  }
}

function setOutput(name, value) {
  if (!process.env.GITHUB_OUTPUT) return;
  // Multi-line values need a delimiter that can not occur in the value.
  const delimiter = `EOF_${Math.random().toString(36).slice(2)}`;
  appendFileSync(process.env.GITHUB_OUTPUT, `${name}<<${delimiter}\n${value}\n${delimiter}\n`);
}

async function main() {
  const lock = JSON.parse(readFileSync(join(root, "pdfium.lock.json"), "utf8"));
  const cargoToml = readFileSync(join(root, "crates", "engine", "Cargo.toml"), "utf8");
  const tag = await latestRelease(lock.repository);
  const result = compare(lock, tag, cargoToml);

  const summary = [];
  if (!result.featureMatchesLock) {
    summary.push(
      `**Mismatch:** pdfium.lock.json pins build ${lock.build} but crates/engine/Cargo.toml asks pdfium-render for feature pdfium_${result.feature}. They must match.`,
    );
  }
  if (result.newer) summary.push(`PDFium build ${result.latest} (\`${tag}\`) is newer than the pinned build ${result.current}.`);
  else summary.push(`PDFium build ${result.current} is the latest release (\`${tag}\`).`);
  console.log(summary.join("\n"));
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${summary.join("\n\n")}\n`);

  setOutput("outdated", String(result.newer));
  setOutput("latest", tag);
  if (result.newer) {
    setOutput("title", issueTitle(tag));
    setOutput("body", issueBody(result, lock, await crateInfo()));
  }
  if (!result.featureMatchesLock) process.exit(1);
}

if (isMain(import.meta.url)) {
  main().catch((error) => {
    console.error(`ERROR: ${error.message}`);
    process.exit(1);
  });
}

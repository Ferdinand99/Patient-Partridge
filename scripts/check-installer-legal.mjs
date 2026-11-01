#!/usr/bin/env node
// Checks that the legal texts really are inside an unpacked installer (see unpack-installer.sh):
//   - THIRD-PARTY-NOTICES, the same as the one in the repository,
//   - the AGPL text (LICENSE in the app folder, and on Linux also the Debian copyright file),
//   - the PDFium licence texts: pdfium-LICENSE and every file of pdfium-licenses/, each the same
//     as the file that came with the PDFium download in vendor/pdfium/<platform>/.
//
// Usage: node scripts/check-installer-legal.mjs <linux|macos|windows> <unpacked folder>
// The unpacked folder has one folder per installer format: deb (linux), dmg (macos), and msi and
// nsis (windows). Each format is checked on its own, so an installer that lacks a text fails even
// when another format of the same system has it.
// Looks at the files that are there, not at the build configuration.

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { isMain } from "./is-main.mjs";
import { platformId } from "./pdfium.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const normal = (buffer) => buffer.toString("utf8").replace(/\r\n/g, "\n");

/** The format folders that unpack-installer.sh makes, per system. */
export const FORMATS = { linux: ["deb"], macos: ["dmg"], windows: ["msi", "nsis"] };

function walk(folder) {
  return readdirSync(folder, { withFileTypes: true }).flatMap((entry) => {
    const path = join(folder, entry.name);
    if (entry.isSymbolicLink()) return [];
    return entry.isDirectory() ? walk(path) : [path];
  });
}

/**
 * Returns a list of problems for ONE installer format; an empty list means it carries all the
 * legal texts. `expected.pdfium` is a Map from file name in pdfium-licenses/ (and "LICENSE" for
 * pdfium-LICENSE) to its text, or null to skip the comparison with the download.
 */
export function checkLegal(kind, folder, expected) {
  const files = walk(folder).map((path) => ({ path, rel: relative(folder, path).split(sep).join("/") }));
  const named = (name) => files.filter((f) => basename(f.path) === name);
  const problems = [];

  const notices = named("THIRD-PARTY-NOTICES");
  if (notices.length === 0) problems.push("THIRD-PARTY-NOTICES is not in the installer");
  for (const f of notices) {
    if (normal(readFileSync(f.path)) !== expected.notices) problems.push(`${f.rel} is not the THIRD-PARTY-NOTICES of the repository`);
  }

  const licenses = named("LICENSE").filter((f) => !f.rel.includes("pdfium-licenses/"));
  if (licenses.length === 0) problems.push("LICENSE (the AGPL text) is not in the installer");
  for (const f of licenses) {
    if (normal(readFileSync(f.path)) !== expected.license) problems.push(`${f.rel} is not the LICENSE of the repository`);
  }

  if (kind === "linux") {
    const copyright = files.filter((f) => f.rel.endsWith("usr/share/doc/patient-partridge/copyright"));
    if (copyright.length !== 1) problems.push("usr/share/doc/patient-partridge/copyright (the AGPL text) is not in the deb");
    else if (normal(readFileSync(copyright[0].path)) !== expected.license) problems.push("the deb copyright file is not the LICENSE of the repository");
    if (!files.some((f) => f.rel.endsWith("usr/share/doc/patient-partridge/THIRD-PARTY-NOTICES"))) {
      problems.push("usr/share/doc/patient-partridge/THIRD-PARTY-NOTICES is not in the deb");
    }
  }

  const pdfiumLicense = named("pdfium-LICENSE");
  if (pdfiumLicense.length === 0 || statSync(pdfiumLicense[0].path).size === 0) problems.push("pdfium-LICENSE is missing or empty");
  const pdfiumTexts = files.filter((f) => f.rel.includes("pdfium-licenses/") && statSync(f.path).size > 0);
  if (!pdfiumTexts.some((f) => basename(f.path) === "pdfium.txt")) problems.push("pdfium-licenses/pdfium.txt is not in the installer");
  if (expected.pdfium) {
    for (const [name, text] of expected.pdfium) {
      const candidates = name === "LICENSE" ? pdfiumLicense : pdfiumTexts.filter((f) => f.rel.endsWith(`pdfium-licenses/${name}`));
      const label = name === "LICENSE" ? "pdfium-LICENSE" : `pdfium-licenses/${name}`;
      if (candidates.length === 0) problems.push(`${label} is not in the installer`);
      else if (!candidates.some((f) => normal(readFileSync(f.path)) === text)) problems.push(`${label} is not the file of the PDFium download`);
    }
  } else if (pdfiumTexts.length < 5) {
    problems.push(`pdfium-licenses/ has only ${pdfiumTexts.length} files`);
  }

  return problems;
}

/** Checks every format of a system, each on its own. Returns "format: problem" lines. */
export function checkSystem(kind, unpacked, expected) {
  const problems = [];
  for (const format of FORMATS[kind]) {
    const folder = join(unpacked, format);
    if (!existsSync(folder)) {
      problems.push(`${format}: the ${format} installer was not unpacked (no folder ${folder})`);
      continue;
    }
    for (const p of checkLegal(kind, folder, expected)) problems.push(`${format}: ${p}`);
  }
  return problems;
}

/** Reads the licence files that came with the PDFium download of this platform. */
export function pdfiumLicenses(vendorFolder) {
  const map = new Map();
  const license = join(vendorFolder, "LICENSE");
  if (!existsSync(license)) throw new Error(`${license} is not there: fetch PDFium first`);
  map.set("LICENSE", normal(readFileSync(license)));
  const folder = join(vendorFolder, "licenses");
  if (!existsSync(folder)) throw new Error(`${folder} is not there: fetch PDFium first`);
  for (const name of readdirSync(folder)) map.set(name, normal(readFileSync(join(folder, name))));
  if (map.size < 5) throw new Error(`${vendorFolder} has only ${map.size} PDFium licence files`);
  return map;
}

if (isMain(import.meta.url)) {
  const [kind, folder] = process.argv.slice(2);
  if (!FORMATS[kind] || !folder) {
    console.error("Usage: node scripts/check-installer-legal.mjs <linux|macos|windows> <unpacked folder>");
    process.exit(2);
  }
  try {
    const expected = {
      notices: normal(readFileSync(join(root, "THIRD-PARTY-NOTICES"))),
      license: normal(readFileSync(join(root, "LICENSE"))),
      pdfium: pdfiumLicenses(join(root, "vendor", "pdfium", platformId())),
    };
    if (!expected.license.includes("GNU AFFERO GENERAL PUBLIC LICENSE")) throw new Error("LICENSE in the repository is not the AGPL text.");
    const problems = checkSystem(kind, folder, expected);
    for (const p of problems) console.error(`ERROR: ${p}`);
    if (problems.length === 0) {
      console.log(`Checked ${FORMATS[kind].join(" and ")}, each on its own: THIRD-PARTY-NOTICES, the AGPL text and the PDFium licences are in all.`);
    }
    process.exit(problems.length === 0 ? 0 : 1);
  } catch (error) {
    console.error(`ERROR: ${error.message}`);
    process.exit(1);
  }
}

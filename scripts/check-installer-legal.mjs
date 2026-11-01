#!/usr/bin/env node
// Checks that the legal texts really are inside an unpacked installer (see unpack-installer.sh):
//   - THIRD-PARTY-NOTICES, the same as the one in the repository,
//   - the AGPL text (LICENSE in the app folder, and on Linux also the Debian copyright file),
//   - the PDFium licence texts (pdfium-LICENSE, and the pdfium-licenses folder with files in it).
//
// Usage: node scripts/check-installer-legal.mjs <linux|macos|windows> <unpacked folder>
// Looks at the files that are there, not at the build configuration.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const normal = (buffer) => buffer.toString("utf8").replace(/\r\n/g, "\n");

function walk(folder) {
  return readdirSync(folder, { withFileTypes: true }).flatMap((entry) => {
    const path = join(folder, entry.name);
    if (entry.isSymbolicLink()) return [];
    return entry.isDirectory() ? walk(path) : [path];
  });
}

/** Returns a list of problems; an empty list means the installer carries all the legal texts. */
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

  if (named("pdfium-LICENSE").length === 0) problems.push("pdfium-LICENSE is not in the installer");
  const pdfiumTexts = files.filter((f) => f.rel.includes("pdfium-licenses/") && statSync(f.path).size > 0);
  if (!pdfiumTexts.some((f) => basename(f.path) === "pdfium.txt")) problems.push("pdfium-licenses/pdfium.txt is not in the installer");
  if (pdfiumTexts.length < 5) problems.push(`pdfium-licenses/ has only ${pdfiumTexts.length} files`);

  return problems;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const [kind, folder] = process.argv.slice(2);
  if (!["linux", "macos", "windows"].includes(kind) || !folder) {
    console.error("Usage: node scripts/check-installer-legal.mjs <linux|macos|windows> <unpacked folder>");
    process.exit(2);
  }
  const expected = {
    notices: normal(readFileSync(join(root, "THIRD-PARTY-NOTICES"))),
    license: normal(readFileSync(join(root, "LICENSE"))),
  };
  if (!expected.license.includes("GNU AFFERO GENERAL PUBLIC LICENSE")) {
    console.error("ERROR: LICENSE in the repository is not the AGPL text.");
    process.exit(1);
  }
  const problems = checkLegal(kind, folder, expected);
  for (const p of problems) console.error(`ERROR: ${p}`);
  if (problems.length === 0) console.log(`The ${kind} installer has THIRD-PARTY-NOTICES, the AGPL text and the PDFium licences.`);
  process.exit(problems.length === 0 ? 0 : 1);
}

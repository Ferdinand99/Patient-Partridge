#!/usr/bin/env node
// Writes THIRD-PARTY-NOTICES: the licence texts of the Rust crates and npm packages that are in
// the app, and fails when one of them has a licence that is not on the allowlist in about.toml.
//
// Usage:
//   node scripts/third-party-notices.mjs --rust app.json --rust engine.json --out THIRD-PARTY-NOTICES
//   node scripts/third-party-notices.mjs --rust app.json --rust engine.json --check
// The --rust files are the output of `cargo about generate --format json` (see
// scripts/notices.sh, which runs everything). --check does not write: it fails if the file on disk
// differs from what would be written, which is what CI uses to catch a stale file.
//
// The npm packages are the production dependencies in package-lock.json (what Vite bundles into
// dist/); their licence files are read from node_modules, so run `npm ci` first.

import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { isMain } from "./is-main.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

/** The allowlist: the `accepted` array of about.toml, the single place where it is kept. */
export function acceptedLicenses(aboutToml) {
  const block = /^accepted\s*=\s*\[([^\]]*)\]/m.exec(aboutToml);
  if (!block) throw new Error("about.toml has no `accepted` list.");
  const ids = [...block[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
  if (ids.length === 0) throw new Error("The `accepted` list of about.toml is empty.");
  return ids;
}

/**
 * Is an SPDX expression covered by the allowlist? "A OR B" needs one side, "A AND B" needs both,
 * and "WITH" exceptions are part of the identifier ("Apache-2.0 WITH LLVM-exception").
 */
export function licenseAllowed(expression, accepted) {
  const allowed = new Set(accepted);
  const parse = (text) => {
    let stripped = text.trim();
    // Remove one pair of parentheses only when it encloses the whole expression.
    while (stripped.startsWith("(") && closingOf(stripped) === stripped.length - 1) {
      stripped = stripped.slice(1, -1).trim();
    }
    // Split on a top-level operator only.
    for (const operator of [" OR ", " AND "]) {
      const parts = splitTopLevel(stripped, operator);
      if (parts.length > 1) {
        return operator === " OR " ? parts.some(parse) : parts.every(parse);
      }
    }
    return allowed.has(stripped);
  };
  return parse(expression);
}

/** Index of the parenthesis that closes the one at position 0, or -1. */
function closingOf(text) {
  let depth = 0;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "(") depth++;
    else if (text[i] === ")" && --depth === 0) return i;
  }
  return -1;
}

function splitTopLevel(text, operator) {
  const parts = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "(") depth++;
    else if (text[i] === ")") depth--;
    else if (depth === 0 && text.startsWith(operator, i)) {
      parts.push(text.slice(start, i));
      start = i + operator.length;
      i = start - 1;
    }
  }
  parts.push(text.slice(start));
  return parts;
}

const clean = (text) => text.replace(/\r\n/g, "\n").trim();

/**
 * A licence template that was never filled in. cargo-about writes the bare SPDX template when it
 * finds no licence file for a crate, and a template is not a copyright notice: MIT and the BSD
 * licences want the copyright notice reproduced. The Apache-2.0 appendix has its own
 * "[yyyy] [name of copyright owner]" boilerplate, which is not matched here.
 */
export const PLACEHOLDER = /<year>|<owner>|<copyright holders?>|<name of author>|<organi[sz]ation>/i;

export function hasPlaceholder(text) {
  return PLACEHOLDER.test(text);
}

const sha256 = (data) => createHash("sha256").update(data).digest("hex");

/** Puts the copyright holder where the template has its placeholders. */
export function fillTemplate(text, holder) {
  return text.replace(/<year>\s+<(?:copyright holders?|owner)>/gi, holder).replace(/<year>\s+<name of author>/gi, holder);
}

/**
 * Reads the `licenses` of cargo-about JSON files into [{ licenseId, text, packages: [label] }].
 *
 * A licence text that still has a placeholder is filled only from `extra.copyright` (notices-extra.json),
 * for the exact crate name and version. A crate without such an entry, an entry whose crate
 * authors differ from the recorded ones, and an entry that no crate needs are all errors.
 */
export function rustEntries(reports, extra = { copyright: [] }) {
  const entries = [];
  const problems = [];
  const wanted = new Map((extra.copyright ?? []).map((c) => [`${c.name} ${c.version}`, c]));
  const used = new Set();
  const asPackage = (crate) => ({ label: `${crate.name} ${crate.version}`, url: crate.repository ?? "" });
  for (const report of reports) {
    for (const license of report.licenses) {
      const text = clean(license.text);
      if (!hasPlaceholder(text)) {
        entries.push({ licenseId: license.id, text, packages: license.used_by.map(({ crate }) => asPackage(crate)) });
        continue;
      }
      for (const { crate } of license.used_by) {
        const key = `${crate.name} ${crate.version}`;
        const entry = wanted.get(key);
        if (!entry) {
          problems.push(`${key} (${license.id}) has no copyright notice, only a licence template. Clarify it in about.toml or add it to notices-extra.json.`);
          continue;
        }
        used.add(key);
        if (entry.license !== license.id) {
          problems.push(`${key}: notices-extra.json says ${entry.license} but the licence is ${license.id}`);
          continue;
        }
        if (entry.basis === "authors") {
          const have = JSON.stringify(crate.authors ?? []);
          if (have !== JSON.stringify(entry.authors) || entry.holder !== entry.authors.join(", ") || entry.authors.length === 0) {
            problems.push(`${key}: the authors in the crate (${have}) are not the ones in notices-extra.json`);
            continue;
          }
        } else if (entry.basis !== "repository" || !entry.holder || !entry.source) {
          problems.push(`${key}: an entry of notices-extra.json needs basis "authors", or basis "repository" with holder and source`);
          continue;
        }
        const filled = fillTemplate(text, entry.holder);
        if (hasPlaceholder(filled)) {
          problems.push(`${key}: the licence template has a placeholder that notices-extra.json can not fill`);
          continue;
        }
        entries.push({ licenseId: license.id, text: filled, packages: [asPackage(crate)] });
      }
    }
  }
  for (const key of wanted.keys()) {
    if (!used.has(key)) problems.push(`notices-extra.json has an entry for ${key} that no crate needs (the version changed, or it is clarified now). Remove or update it.`);
  }
  if (problems.length > 0) throw new Error(problems.join("\n"));
  return entries;
}

/**
 * The WebView2 loader that the Windows build links. The crate webview2-com-sys carries Microsoft's
 * WebView2LoaderStatic.lib; its licence is not in the crate. The text is committed from the
 * NuGet package of the exact version that these files come from, and the files are pinned by hash:
 * a new crate version with a new SDK fails here until the licence is looked at again.
 */
export function webview2Section(extra, reports, readLicense = (file) => readFileSync(join(root, file), "utf8")) {
  const w = extra.webview2;
  if (!w) throw new Error("notices-extra.json has no webview2 entry.");
  const crates = reports.flatMap((r) => r.licenses.flatMap((l) => l.used_by.map((u) => u.crate))).filter((c) => c.name === w.vendoredBy);
  if (crates.length === 0) throw new Error(`${w.vendoredBy} is not in the dependencies, so the webview2 entry in notices-extra.json is not needed. Remove it.`);
  const problems = [];
  for (const crate of crates) {
    for (const [file, hash] of Object.entries(w.vendoredFiles)) {
      const path = join(dirname(crate.manifest_path), file);
      if (!existsSync(path)) problems.push(`${crate.name} ${crate.version}: ${file} is not there`);
      else if (sha256(readFileSync(path)) !== hash) {
        problems.push(`${crate.name} ${crate.version}: ${file} is not the file of ${w.package} ${w.version}. Take the licence from the package this crate uses now.`);
      }
    }
  }
  const raw = readLicense(w.licenseFile).replace(/\r\n/g, "\n");
  if (sha256(raw) !== w.licenseSha256) problems.push(`${w.licenseFile} does not have the hash in notices-extra.json`);
  const text = clean(raw);
  if (problems.length > 0) throw new Error(problems.join("\n"));
  return { title: "WebView2 loader (Windows only)", package: w.package, version: w.version, crate: `${crates[0].name} ${crates[0].version}`, text };
}

/**
 * The production npm packages of a package-lock.json, with licence files read from node_modules.
 * Throws for a licence that is not allowed and for a package without a licence text.
 */
export function npmEntries(lock, accepted, projectRoot) {
  const entries = [];
  const problems = [];
  for (const [path, info] of Object.entries(lock.packages ?? {})) {
    if (!path.startsWith("node_modules/") || info.dev || info.devOptional) continue;
    const name = path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
    const label = `${name} ${info.version}`;
    const expression = info.license ?? "";
    if (!licenseAllowed(expression, accepted)) {
      problems.push(`${label} has the licence "${expression || "(none)"}", which is not on the allowlist in about.toml`);
      continue;
    }
    // The lock path itself, so a package nested below another one reads its own files.
    const folder = join(projectRoot, path);
    const files = existsSync(folder)
      ? readdirSync(folder).filter((f) => /^(licen[cs]e|copying|notice)([-._].*)?$/i.test(f)).sort()
      : [];
    if (files.length === 0) {
      problems.push(`${label}: no licence file found in ${folder} (run npm ci)`);
      continue;
    }
    for (const file of files) {
      entries.push({
        licenseId: expression,
        text: clean(readFileSync(join(folder, file), "utf8")),
        packages: [{ label, url: "" }],
      });
    }
  }
  if (problems.length > 0) throw new Error(problems.join("\n"));
  return entries;
}

/** Groups entries with the same text, so a licence that many crates share is written once. */
export function group(entries) {
  const byText = new Map();
  for (const entry of entries) {
    const found = byText.get(entry.text) ?? { text: entry.text, ids: new Set(), packages: new Map() };
    found.ids.add(entry.licenseId);
    for (const p of entry.packages) found.packages.set(p.label, p.url);
    byText.set(entry.text, found);
  }
  return [...byText.values()]
    .map((g) => ({ text: g.text, ids: [...g.ids].sort(), packages: [...g.packages].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)) }))
    .sort((a, b) => {
      const key = (g) => `${g.ids.join(" ")}\n${g.packages[0][0]}`;
      return key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0;
    });
}

const RULE = "=".repeat(78);

function section(title, groups) {
  const count = new Set(groups.flatMap((g) => g.packages.map(([label]) => label))).size;
  const lines = [RULE, `${title} (${count} packages)`, RULE, ""];
  for (const g of groups) {
    lines.push(`--- ${g.ids.join(", ")} ---`, "", "Used by:");
    for (const [label, url] of g.packages) lines.push(url ? `  ${label} (${url})` : `  ${label}`);
    lines.push("", g.text, "", "");
  }
  return lines.join("\n");
}

/** The whole file. Same input, same text: no dates, no paths. */
export function render({ rust, npm, fontLicense, webview2 }) {
  const head = [
    "THIRD-PARTY NOTICES",
    "",
    "Patient Partridge is free software under the GNU Affero General Public License,",
    "version 3 (see the file LICENSE). It contains the third-party software listed below.",
    "Each section gives the licence text and the packages it applies to.",
    "",
    "This file is made by scripts/notices.sh from the dependencies of the program. Do not edit it.",
    "",
    "PDFium, the PDF library, is not listed here. Its licence texts are shipped next to the",
    "PDFium library, in the files pdfium-LICENSE and pdfium-licenses/.",
    "",
    "The Rust crates are those that the app and the engine are built from, for Windows, macOS",
    "and Linux. A crate that only runs while the program is built (a build script) is not listed.",
    "A crate that is only used at build time through a procedural macro may be listed.",
    "",
    "",
  ].join("\n");
  const font = [
    RULE,
    "Inter (font)",
    RULE,
    "",
    "The Inter font (InterVariable.woff2) is shipped with the window. Copyright (c) 2016 The",
    "Inter Project Authors (https://github.com/rsms/inter), SIL Open Font License 1.1:",
    "",
    clean(fontLicense),
    "",
    "",
  ].join("\n");
  const loader = [
    RULE,
    webview2.title,
    RULE,
    "",
    `The Windows program links Microsoft's WebView2 loader (WebView2LoaderStatic.lib), which the`,
    `crate ${webview2.crate} carries. The files are those of ${webview2.package} ${webview2.version}; this is the`,
    "LICENSE.txt of that package:",
    "",
    webview2.text,
    "",
    "",
  ].join("\n");
  const text = `${head}${section("Rust crates", group(rust))}${section("npm packages", group(npm))}${loader}${font}`.replace(/\n+$/, "\n");
  if (hasPlaceholder(text)) throw new Error("THIRD-PARTY-NOTICES would contain an unfilled licence template (<year>, <owner> or <copyright holders>).");
  return text;
}

function parseArgs(argv) {
  const options = { rust: [], out: null, check: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--rust") options.rust.push(argv[++i]);
    else if (argv[i] === "--out") options.out = argv[++i];
    else if (argv[i] === "--check") options.check = true;
    else throw new Error(`Unknown argument ${argv[i]}`);
  }
  if (options.rust.length === 0) throw new Error("Give at least one --rust <cargo-about json>.");
  if (!options.check && !options.out) throw new Error("Give --out <file> or --check.");
  return options;
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const accepted = acceptedLicenses(readFileSync(join(root, "about.toml"), "utf8"));
  const extra = JSON.parse(readFileSync(join(root, "notices-extra.json"), "utf8"));
  const reports = options.rust.map((file) => JSON.parse(readFileSync(file, "utf8")));
  const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
  const text = render({
    rust: rustEntries(reports, extra),
    npm: npmEntries(lock, accepted, root),
    webview2: webview2Section(extra, reports),
    fontLicense: readFileSync(join(root, "public", "fonts", "Inter-LICENSE.txt"), "utf8"),
  });
  const target = join(root, options.out ?? "THIRD-PARTY-NOTICES");
  if (options.check) {
    const current = existsSync(target) ? readFileSync(target, "utf8").replace(/\r\n/g, "\n") : "";
    if (current !== text) {
      console.error("THIRD-PARTY-NOTICES is out of date. Run scripts/notices.sh and commit the result.");
      process.exit(1);
    }
    console.log("THIRD-PARTY-NOTICES is up to date.");
  } else {
    writeFileSync(target, text);
    console.log(`Wrote ${target} (${text.length} characters).`);
  }
}

if (isMain(import.meta.url)) {
  try {
    main();
  } catch (error) {
    console.error(`ERROR: ${error.message}`);
    process.exit(1);
  }
}

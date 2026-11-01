// Checks that the app is registered as a viewer for PDF files in what the build made, on each
// system. Run by CI after the installers are built:
//
//   node scripts/association.mjs linux   <folder with the unpacked deb>
//   node scripts/association.mjs macos   <the .app folder>
//   node scripts/association.mjs windows <the .msi file>
//
// The app claims the role of a viewer with the rank "Alternate": it shows up in "Open with" and can
// be chosen as the default, but installing it never takes the default away from another reader.

import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

/** What the Info.plist of the macOS app must say, as parsed JSON. macOS names a file type by its
 * UTI (com.adobe.pdf), not by a MIME type. */
export function checkMacPlist(plist) {
  const problems = [];
  const types = Array.isArray(plist.CFBundleDocumentTypes) ? plist.CFBundleDocumentTypes : [];
  const pdf = types.find((t) => (t.CFBundleTypeExtensions ?? []).includes("pdf"));
  if (!pdf) {
    problems.push("Info.plist has no document type for the extension pdf");
    return problems;
  }
  if (!(pdf.LSItemContentTypes ?? []).includes("com.adobe.pdf")) problems.push("the PDF type lacks com.adobe.pdf");
  if (pdf.CFBundleTypeRole !== "Viewer") problems.push(`the role is ${pdf.CFBundleTypeRole}, not Viewer`);
  if (pdf.LSHandlerRank !== "Alternate") problems.push(`the rank is ${pdf.LSHandlerRank}, not Alternate`);
  return problems;
}

/** What the rows of the Extension and MIME tables of the MSI must hold. */
export function checkMsiRows(extensions, mimeTypes) {
  const problems = [];
  if (!extensions.map((e) => e.toLowerCase()).includes("pdf")) problems.push("the installer registers no .pdf extension");
  if (!mimeTypes.map((m) => m.toLowerCase()).includes("application/pdf")) problems.push("the installer registers no application/pdf type");
  return problems;
}

/** What the desktop file of the deb must say. */
export function checkDesktopFile(text) {
  const problems = [];
  if (!/^Exec=.* %F$/m.test(text)) problems.push("Exec does not pass the files (%F)");
  if (!/^MimeType=application\/pdf;$/m.test(text)) problems.push("MimeType is not application/pdf;");
  return problems;
}

function findFiles(dir, test, found = []) {
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) findFiles(path, test, found);
    else if (test(name)) found.push(path);
  }
  return found;
}

function msiRows(msi, query) {
  const script = `
$ErrorActionPreference = 'Stop'
$installer = New-Object -ComObject WindowsInstaller.Installer
$db = $installer.GetType().InvokeMember('OpenDatabase', 'InvokeMethod', $null, $installer, @('${msi.replace(/'/g, "''")}', 0))
$view = $db.GetType().InvokeMember('OpenView', 'InvokeMethod', $null, $db, @('${query}'))
$view.GetType().InvokeMember('Execute', 'InvokeMethod', $null, $view, $null)
while ($true) {
  $record = $view.GetType().InvokeMember('Fetch', 'InvokeMethod', $null, $view, $null)
  if ($null -eq $record) { break }
  $record.GetType().InvokeMember('StringData', 'GetProperty', $null, $record, @(1))
}
`;
  const out = execFileSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], { encoding: "utf8" });
  return out.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
}

function main() {
  const [system, target] = process.argv.slice(2);
  let problems;
  if (system === "linux") {
    const files = findFiles(target, (n) => n.endsWith(".desktop"));
    if (files.length === 0) problems = ["the deb has no desktop file"];
    else problems = checkDesktopFile(readFileSync(files[0], "utf8"));
  } else if (system === "macos") {
    const json = execFileSync("plutil", ["-convert", "json", "-o", "-", join(target, "Contents", "Info.plist")], { encoding: "utf8" });
    const plist = JSON.parse(json);
    console.log(JSON.stringify(plist.CFBundleDocumentTypes ?? null, null, 2));
    problems = checkMacPlist(plist);
  } else if (system === "windows") {
    const msi = resolve(target);
    problems = checkMsiRows(msiRows(msi, "SELECT Extension FROM Extension"), msiRows(msi, "SELECT ContentType FROM MIME"));
  } else {
    console.error("usage: association.mjs <linux|macos|windows> <path>");
    process.exit(2);
  }
  if (problems.length > 0) {
    console.error(`File association problems on ${system}:`);
    for (const p of problems) console.error(`- ${p}`);
    process.exit(1);
  }
  console.log(`ok: the ${system} build registers the app as a viewer for PDF files.`);
}

if (process.argv[1]?.endsWith("association.mjs")) main();

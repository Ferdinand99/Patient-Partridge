// What a Windows computer looks like when a test waits for the app in vain. Printed on a timeout
// (and when the app hands over to the installer), so that a failure shows whether the app and its
// web view were running, and not only that nothing came back.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { join } from "node:path";

const IDENTIFIER = "com.ferdinand99.patientpartridge";

/** The programs of the app, its web view and the installer, with start time, CPU and memory. */
export function windowsProcessList() {
  const command =
    "Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'partridge|msedgewebview2|setup|msiexec|nsis' } | " +
    "Select-Object Name,Id,StartTime,@{n='CpuSeconds';e={[int]$_.CPU}},@{n='WorkingSetMB';e={[int]($_.WorkingSet64/1MB)}} | " +
    "Format-Table -AutoSize | Out-String -Width 200";
  const r = spawnSync("powershell", ["-NoProfile", "-Command", command], { encoding: "utf8", timeout: 60_000 });
  const text = (r.stdout ?? "").trim();
  return text || "(none of them is running)";
}

/** Process list plus whether the web view has made its profile folder. */
export function describeWindows(label) {
  const profile = join(process.env.LOCALAPPDATA ?? "", IDENTIFIER, "EBWebView");
  return [
    `    [windows] ${label} at ${new Date().toISOString()}`,
    `    [windows] processes:\n${windowsProcessList()}`,
    `    [windows] web view profile folder exists: ${existsSync(profile)}`,
  ].join("\n");
}

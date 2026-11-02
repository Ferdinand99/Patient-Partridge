#!/usr/bin/env node
// Starts the built app with a PDF, waits until it has drawn the first page, and prints how long
// that took. Fails when the app never gets that far, so CI notices a broken engine, library or
// policy. Usage:
//   node scripts/smoke.mjs --bin <app binary> --pdf <file.pdf> [--runs 3] [--memory]
//
// The app writes a small JSON report when PARTRIDGE_MEASURE_REPORT is set (see src-tauri), then
// closes itself. With --memory the script also reads the memory of the app and the processes it
// started while the app is still up (unix only; see treeMemory for what the numbers mean).

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { launchEnvironment, launchLimitMs } from "./smoke-env.mjs";
import { describeWindows } from "./windows-diagnostics.mjs";

function parse(argv) {
  const out = { runs: 3, memory: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--bin") out.bin = argv[++i];
    else if (a === "--pdf") out.pdf = argv[++i];
    else if (a === "--runs") out.runs = parseInt(argv[++i], 10);
    else if (a === "--memory") out.memory = true;
    else throw new Error(`unknown argument ${a}`);
  }
  if (!out.bin || !out.pdf) throw new Error("--bin and --pdf are required");
  return out;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Memory of a process and all its descendants, via `ps`. `rss_mib` adds up resident memory, which
 * counts shared pages once per process; on Linux `pss_mib` shares those pages out between the
 * processes that use them, which is the fairer number. `processes` lists the biggest ones.
 */
function treeMemory(rootPid) {
  const ps = spawnSync("ps", ["-A", "-o", "pid=,ppid=,rss=,comm="], { encoding: "utf8" });
  if (ps.status !== 0) return null;
  const rows = ps.stdout
    .trim()
    .split("\n")
    .map((line) => {
      const m = /^\s*(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/.exec(line);
      return m ? { pid: +m[1], ppid: +m[2], rss: +m[3], name: m[4].trim() } : null;
    })
    .filter(Boolean);
  const byParent = new Map();
  for (const row of rows) {
    if (!byParent.has(row.ppid)) byParent.set(row.ppid, []);
    byParent.get(row.ppid).push(row);
  }
  const byPid = new Map(rows.map((r) => [r.pid, r]));
  const tree = [];
  const stack = [rootPid];
  while (stack.length) {
    const pid = stack.pop();
    const row = byPid.get(pid);
    if (row) tree.push(row);
    for (const child of byParent.get(pid) ?? []) stack.push(child.pid);
  }
  let rss = 0;
  let pss = 0;
  let havePss = true;
  const processes = [];
  for (const row of tree) {
    let pssKb = null;
    try {
      const rollup = readFileSync(`/proc/${row.pid}/smaps_rollup`, "utf8");
      pssKb = +/^Pss:\s+(\d+) kB/m.exec(rollup)[1];
    } catch {
      havePss = false;
    }
    rss += row.rss;
    if (pssKb !== null) pss += pssKb;
    processes.push({
      name: row.name,
      rss_mib: +(row.rss / 1024).toFixed(1),
      pss_mib: pssKb === null ? null : +(pssKb / 1024).toFixed(1),
    });
  }
  processes.sort((a, b) => b.rss_mib - a.rss_mib);
  return {
    rss_mib: rss / 1024,
    pss_mib: havePss ? pss / 1024 : null,
    processes: processes.slice(0, 6),
  };
}

function killTree(child) {
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/T", "/F", "/PID", String(child.pid)], { stdio: "ignore" });
  } else {
    child.kill("SIGKILL");
  }
}

async function runOnce({ bin, pdf, memory }, index) {
  const limitMs = launchLimitMs(index);
  const dir = mkdtempSync(join(tmpdir(), "partridge-smoke-"));
  const report = join(dir, "report.json");
  // A fresh profile, so each run starts as a first run (see smoke-env.mjs).
  const env = launchEnvironment({ ...process.env, PARTRIDGE_MEASURE_REPORT: report }, dir);
  const child = spawn(bin, [pdf], { env: { ...env, RUST_BACKTRACE: "1" }, stdio: ["ignore", "pipe", "pipe"] });
  // Keep the end of what the app wrote, to show it if the run fails.
  let output = "";
  const keep = (chunk) => {
    output = (output + chunk).slice(-6000);
  };
  child.stdout.on("data", keep);
  child.stderr.on("data", keep);
  let exited = false;
  child.on("exit", (code, signal) => {
    exited = true;
    keep(`\n[the app exited: code ${code}, signal ${signal}]\n`);
  });
  try {
    const deadline = Date.now() + limitMs;
    while (Date.now() < deadline) {
      if (existsSync(report) && readFileSync(report, "utf8").trim()) break;
      if (exited) break;
      await sleep(100);
    }
    if (!existsSync(report) || !readFileSync(report, "utf8").trim()) {
      // On Windows, say whether the app and its web view were still running: a hang or an exit.
      const state =
        process.platform === "win32" ? `\n${describeWindows(exited ? "after the app exited" : "while the app was still running")}` : "";
      throw new Error(
        `The app did not report a drawn page (it exited or took longer than ${limitMs / 1000} s, run ${index + 1}). What it wrote:\n${output}${state}`,
      );
    }
    await sleep(300);
    const result = JSON.parse(readFileSync(report, "utf8"));
    if (memory && process.platform !== "win32") {
      // Give the page cache and the web view a moment to settle, then measure.
      await sleep(1500);
      Object.assign(result, treeMemory(child.pid));
    }
    return result;
  } finally {
    killTree(child);
    await sleep(200);
    // The web view may still be writing its caches for a moment. A folder that cannot be removed
    // is not a failure of the app, so it is only retried, never reported.
    try {
      rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 300 });
    } catch {
      // Left behind in the temporary folder.
    }
  }
}

const median = (values) => {
  const v = values.filter((x) => typeof x === "number").sort((a, b) => a - b);
  return v.length ? v[Math.floor(v.length / 2)] : null;
};

const options = parse(process.argv.slice(2));
if (!existsSync(options.bin)) throw new Error(`App binary not found: ${options.bin}`);
const results = [];
for (let i = 0; i < options.runs; i++) {
  const r = await runOnce(options, i);
  results.push(r);
  console.log(`run ${i + 1}: ${JSON.stringify(r)}`);
}
const summary = {
  pages: results[0].pages,
  runs: results.length,
  open_ms_median: median(results.map((r) => r.open_ms)),
  first_page_ms_median: median(results.map((r) => r.first_page_ms)),
  since_process_start_ms_median: median(results.map((r) => r.since_process_start_ms)),
  rss_mib_median: median(results.map((r) => r.rss_mib)),
  pss_mib_median: median(results.map((r) => r.pss_mib)),
};
console.log(`summary: ${JSON.stringify(summary)}`);

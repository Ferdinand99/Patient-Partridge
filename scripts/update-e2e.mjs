#!/usr/bin/env node
// Runs a real update from start to end on this computer, with nothing faked on the app side.
//
//   1. Makes a throwaway signing key. The real key is never used and never read: the script refuses
//      to start when TAURI_SIGNING_PRIVATE_KEY is set.
//   2. Builds an old version (0.0.1) and a new one (0.0.2) of the real app, signed with that key,
//      with a test public key and a local address in the updater configuration, and with the
//      `update-driver` feature, which lets the app start and finish an update by itself (see
//      src-tauri/src/updates.rs). The code that checks, downloads, verifies, stops the engines and
//      installs is the code of the release.
//   3. Serves latest.json and the new version from 127.0.0.1.
//   4. Installs the old version the way a user gets it (an AppImage file, the NSIS installer, an
//      app folder), starts it with a PDF, and lets it update itself. The new version starts again,
//      opens what was open before, and reports itself.
//   5. Two refused updates: one whose file was changed after it was signed, and one whose version
//      does not match the one it was signed for. Both must be refused with the old version intact.
//   6. On Linux with --deb: the deb package must be recognised and only offered as a notice.
//
// Usage: node scripts/update-e2e.mjs [--skip-build] [--deb] [--work <folder>]
// What happened is written to <work>/summary.md. Exit code 0 only when every check passed.

import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const OLD = "0.0.1";
const NEW = "0.0.2";
const PORT = 18099;
const win = process.platform === "win32";
const mac = process.platform === "darwin";
const linux = process.platform === "linux";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const args = process.argv.slice(2);
const skipBuild = args.includes("--skip-build");
const withDeb = args.includes("--deb");
const work = args.includes("--work") ? args[args.indexOf("--work") + 1] : join(root, "e2e-update");
const targetDir = process.env.CARGO_TARGET_DIR ?? join(root, "target");
const bundleDir = join(targetDir, "release", "bundle");

if (process.env.TAURI_SIGNING_PRIVATE_KEY || process.env.TAURI_SIGNING_PRIVATE_KEY_PATH) {
  throw new Error("A signing key is set in the environment. This test makes its own throwaway key and refuses to run next to a real one.");
}

const results = [];
function check(name, ok, detail = "") {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? `  (${detail})` : ""}`);
}

function sh(command, argv, options = {}) {
  const r = spawnSync(command, argv, { cwd: root, stdio: "inherit", shell: win, ...options });
  if (r.status !== 0) throw new Error(`${command} ${argv.join(" ")} failed with ${r.status}`);
}

const sha256 = (file) => createHash("sha256").update(readFileSync(file)).digest("hex");

function find(dir, test) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter(test)
    .map((n) => join(dir, n));
}

// ---- the key and the builds -------------------------------------------------------------------

mkdirSync(work, { recursive: true });
const keyFile = join(work, "throwaway.key");
const otherKeyFile = join(work, "other.key");
const artifacts = { [OLD]: {}, [NEW]: {} };
// The test builds skip link-time optimisation, which only makes the build slow; the code is the same.
const buildEnv = {
  ...process.env,
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "throwaway",
  CARGO_PROFILE_RELEASE_LTO: "off",
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16",
};

function makeKey(file) {
  rmSync(file, { force: true });
  rmSync(`${file}.pub`, { force: true });
  sh("npx", ["tauri", "signer", "generate", "--ci", "-w", file, "-p", "throwaway"]);
}

const realKey = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8")).plugins.updater.pubkey;

function platformKey() {
  const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
  return `${win ? "windows" : mac ? "darwin" : "linux"}-${arch}`;
}

function bundlesForThisSystem() {
  if (win) return "nsis";
  if (mac) return "app";
  return withDeb ? "appimage,deb" : "appimage";
}

function collect(version) {
  const dest = join(work, "build", version);
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dest, { recursive: true });
  const take = (dir, test) => {
    const files = find(join(bundleDir, dir), test);
    for (const f of files) copyFileSync(f, join(dest, f.split(/[\\/]/).pop()));
    return files.map((f) => join(dest, f.split(/[\\/]/).pop()));
  };
  if (linux) {
    const [payload] = take("appimage", (n) => /\.AppImage$/.test(n));
    const [sig] = take("appimage", (n) => /\.AppImage\.sig$/.test(n));
    const [deb] = withDeb ? take("deb", (n) => n.endsWith(".deb")) : [];
    return { payload, sig, installable: payload, deb };
  }
  if (win) {
    const [payload] = take("nsis", (n) => /-setup\.exe$/.test(n));
    const [sig] = take("nsis", (n) => /-setup\.exe\.sig$/.test(n));
    return { payload, sig, installable: payload };
  }
  const [payload] = take("macos", (n) => n.endsWith(".app.tar.gz"));
  const [sig] = take("macos", (n) => n.endsWith(".app.tar.gz.sig"));
  const app = find(join(bundleDir, "macos"), (n) => n.endsWith(".app"))[0];
  const appCopy = join(dest, "app", app.split(/[\\/]/).pop());
  cpSync(app, appCopy, { recursive: true, verbatimSymlinks: true });
  return { payload, sig, installable: appCopy };
}

function build(version, pubkey) {
  const generated = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.bundle.generated.conf.json"), "utf8"));
  const conf = {
    ...generated,
    version,
    bundle: { ...generated.bundle, createUpdaterArtifacts: true },
    plugins: {
      updater: {
        pubkey,
        endpoints: [`http://127.0.0.1:${PORT}/latest.json`],
        dangerousInsecureTransportProtocol: true,
      },
    },
  };
  const confFile = join(root, "src-tauri", "tauri.e2e.generated.conf.json");
  writeFileSync(confFile, JSON.stringify(conf, null, 2));
  rmSync(join(bundleDir), { recursive: true, force: true });
  sh("npx", ["tauri", "build", "--config", confFile, "--features", "update-driver", "--bundles", bundlesForThisSystem(), "--", "--locked"], {
    env: { ...buildEnv, TAURI_SIGNING_PRIVATE_KEY: readFileSync(keyFile, "utf8") },
  });
  artifacts[version] = collect(version);
  rmSync(confFile, { force: true });
}

if (!skipBuild) {
  makeKey(keyFile);
  makeKey(otherKeyFile);
  // The file holds the public key as Tauri writes it (the text of the minisign file, base64), which
  // is also what `plugins.updater.pubkey` takes.
  const pub = readFileSync(`${keyFile}.pub`, "utf8").trim();
  if (pub === realKey) throw new Error("The throwaway key equals the real public key.");
  sh("node", ["scripts/prepare-bundle.mjs"], { env: { ...buildEnv, TAURI_SIGNING_PRIVATE_KEY: readFileSync(keyFile, "utf8") } });
  build(OLD, pub);
  build(NEW, pub);
  writeFileSync(join(work, "artifacts.json"), JSON.stringify(artifacts, null, 2));
} else {
  Object.assign(artifacts, JSON.parse(readFileSync(join(work, "artifacts.json"), "utf8")));
}

// ---- the server -------------------------------------------------------------------------------

const serveDir = join(work, "serve");
let manifest = "{}";
const server = createServer((req, res) => {
  const name = decodeURIComponent((req.url ?? "/").split("?")[0]).replace(/^\/+/, "");
  if (name === "latest.json") {
    res.writeHead(200, { "content-type": "application/json" }).end(manifest);
    return;
  }
  const file = join(serveDir, name.replace(/[\\/]/g, "_"));
  if (existsSync(file)) {
    res.writeHead(200, { "content-type": "application/octet-stream" }).end(readFileSync(file));
  } else {
    res.writeHead(404).end("not found");
  }
});
await new Promise((r) => server.listen(PORT, "127.0.0.1", r));

function publish({ version, payload, signature, label }) {
  rmSync(serveDir, { recursive: true, force: true });
  mkdirSync(serveDir, { recursive: true });
  const name = `update-${label}.bin`;
  writeFileSync(join(serveDir, name), payload);
  manifest = JSON.stringify({
    version,
    notes: "## 0.0.2\n\n* **e2e:** a test release",
    pub_date: "2026-11-01T00:00:00Z",
    platforms: { [platformKey()]: { signature, url: `http://127.0.0.1:${PORT}/${name}` } },
  });
}

const goodPayload = () => readFileSync(artifacts[NEW].payload);
const goodSignature = () => readFileSync(artifacts[NEW].sig, "utf8").trim();

// ---- running an old copy ----------------------------------------------------------------------

function appEnv(home, log) {
  const env = { ...process.env, PARTRIDGE_UPDATE_DRIVER: log, RUST_BACKTRACE: "1" };
  delete env.TAURI_SIGNING_PRIVATE_KEY;
  if (!win) {
    env.HOME = home;
    env.XDG_CONFIG_HOME = join(home, ".config");
    env.XDG_DATA_HOME = join(home, ".local", "share");
    env.XDG_CACHE_HOME = join(home, ".cache");
  }
  if (linux) {
    env.WEBKIT_DISABLE_COMPOSITING_MODE = "1";
    env.WEBKIT_DISABLE_DMABUF_RENDERER = "1";
    // Without FUSE (a plain container) an AppImage runs by unpacking itself; E2E_FUSE=1 mounts it.
    if (process.env.E2E_FUSE !== "1") env.APPIMAGE_EXTRACT_AND_RUN = "1";
  }
  return env;
}

let xvfb = null;
function startDisplay() {
  if (!linux || process.env.DISPLAY) return;
  xvfb = spawn("Xvfb", [":97", "-screen", "0", "1280x800x24"], { stdio: "ignore" });
  process.env.DISPLAY = ":97";
}

function readLog(file) {
  return existsSync(file) ? readFileSync(file, "utf8").split("\n").filter(Boolean) : [];
}

async function waitFor(file, test, seconds) {
  const end = Date.now() + seconds * 1000;
  while (Date.now() < end) {
    if (readLog(file).some(test)) return true;
    await sleep(500);
  }
  return false;
}

function killApps() {
  if (win) {
    for (const image of ["patient-partridge.exe", "partridge-engine.exe"]) spawnSync("taskkill", ["/IM", image, "/F", "/T"], { stdio: "ignore" });
  } else {
    spawnSync("pkill", ["-9", "-f", "patient-partridge|partridge-engine|Partridge.*AppImage"], { stdio: "ignore" });
  }
}

/** Puts the old version where a user would have it, and returns how to start it. */
function installOld(dir) {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  const old = artifacts[OLD];
  if (linux) {
    const file = join(dir, "Patient-Partridge.AppImage");
    copyFileSync(old.installable, file);
    chmodSync(file, 0o755);
    return { command: file, file };
  }
  if (win) {
    const installDir = join(dir, "app");
    sh(old.installable, ["/S", `/D=${installDir}`], { shell: false });
    return { command: join(installDir, "patient-partridge.exe"), file: join(installDir, "patient-partridge.exe") };
  }
  const app = join(dir, "Patient Partridge.app");
  cpSync(old.installable, app, { recursive: true, verbatimSymlinks: true });
  return { command: join(app, "Contents", "MacOS", "patient-partridge"), file: app };
}

const pdf = join(work, "test.pdf");
function makePdf() {
  if (existsSync(pdf)) return;
  sh("cargo", ["run", "--release", "--locked", "-p", "partridge-testkit", "--bin", "make-pdf", "--", "3", pdf]);
}

async function runCase(name, setup, expectations, { seconds = 150 } = {}) {
  console.log(`\n=== ${name} ===`);
  killApps();
  const dir = join(work, "run", name);
  const log = join(dir, "driver.log");
  const home = join(dir, "home");
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(home, { recursive: true });
  if (win) {
    const data = join(process.env.APPDATA ?? "", "com.ferdinand99.patientpartridge");
    rmSync(join(data, "restore-after-update.json"), { force: true });
    rmSync(join(data, "settings.json"), { force: true });
  }
  const installed = installOld(join(dir, "install"));
  setup?.();
  const before = existsSync(installed.file) && !mac ? sha256(installed.file) : null;
  const child = spawn(installed.command, [pdf], { env: appEnv(home, log), stdio: "ignore", detached: !win });
  child.unref();
  let done = await waitFor(log, expectations.finish, seconds);
  const startedByInstaller = done;
  if (win && !done && has(readLog(log), /^documents open before the stop/)) {
    // The app handed over to the installer. Say what the computer looks like, and, if the new
    // version is there but was not started, start it, so that the rest can still be checked.
    const ps = (command) => spawnSync("powershell", ["-NoProfile", "-Command", command], { encoding: "utf8" }).stdout.trim();
    console.log(`    [windows] processes:\n${ps("Get-Process | Where-Object { $_.Name -match 'partridge|setup|msiexec|nsis' } | Select-Object Name,Id | Out-String")}`);
    const version = ps(`(Get-Item '${installed.file}').VersionInfo.ProductVersion`);
    console.log(`    [windows] version of the installed program: ${version}`);
    console.log(`    [windows] install folder:\n${ps(`Get-ChildItem '${dirname(installed.file)}' | Select-Object Name,Length,LastWriteTime | Out-String`)}`);
    if (version.startsWith(NEW)) {
      const again = spawn(installed.command, [pdf], { env: appEnv(home, log), stdio: "ignore" });
      again.unref();
      done = await waitFor(log, expectations.finish, 90);
    }
  }
  await sleep(1500);
  const lines = readLog(log);
  console.log(lines.map((l) => `    ${l}`).join("\n"));
  killApps();
  return { lines, done, installed, before, dir, startedByInstaller };
}

const has = (lines, re) => lines.some((l) => re.test(l));
const index = (lines, re) => lines.findIndex((l) => re.test(l));

// ---- the cases --------------------------------------------------------------------------------

makePdf();
startDisplay();
const summary = [];

try {
  // 1. A good update.
  publish({ version: NEW, payload: goodPayload(), signature: goodSignature(), label: "good" });
  const good = await runCase("update", null, { finish: (l) => /^up-to-date version=0\.0\.2/.test(l) });
  const L = good.lines;
  check("the old version started", has(L, /^started version=0\.0\.1 method=Automatic restored=0/));
  check("it found the new version", has(L, /^found version=0\.0\.2 from=0\.0\.1/));
  check("a document was open in the old version", has(L, /^documents open=1$/));
  check(
    "the engines were stopped before the install started",
    has(L, /^documents open before the stop=1, when the install starts=0$/),
  );
  check("the new version started", has(L, /^started version=0\.0\.2/));
  check("the update started the new version by itself", good.startedByInstaller);
  check("the open document came back", has(L, /^started version=0\.0\.2 .* restored=1$/));
  check("the new version is up to date and ends", good.done);
  const order = [/^found/, /^documents open before the stop/, /^started version=0\.0\.2/, /^up-to-date/].map((re) => index(L, re));
  check("things happened in that order", order.every((v, i) => v >= 0 && (i === 0 || v > order[i - 1])), order.join(","));
  if (linux) check("the AppImage file is now the new one", sha256(good.installed.file) === sha256(artifacts[NEW].payload));
  summary.push(`### Update (${platformKey()})\n\n\`\`\`\n${L.join("\n")}\n\`\`\``);

  // 2. The file was changed after it was signed.
  const tampered = Buffer.from(goodPayload());
  tampered[Math.floor(tampered.length / 2)] ^= 0xff;
  publish({ version: NEW, payload: tampered, signature: goodSignature(), label: "tampered" });
  const t = await runCase("tampered", null, { finish: (l) => /^install-failed|^up-to-date/.test(l) }, { seconds: 90 });
  check("a changed file is refused", has(t.lines, /^install-failed kind=InvalidSignature/));
  check("a refused update does not stop the engines", !has(t.lines, /documents open before the stop/));
  if (linux) check("a refused update leaves the old file as it was", sha256(t.installed.file) === t.before);
  summary.push(`### Changed file\n\n\`\`\`\n${t.lines.join("\n")}\n\`\`\``);

  // 3. The version of the answer is not the version the file was signed for.
  publish({ version: "0.0.3", payload: goodPayload(), signature: goodSignature(), label: "mismatch" });
  const m = await runCase("mismatch", null, { finish: (l) => /^install-failed|^up-to-date/.test(l) }, { seconds: 90 });
  check("an update announced with another version is refused", has(m.lines, /^install-failed kind=InvalidSignature/));
  check("that refusal does not stop the engines", !has(m.lines, /documents open before the stop/));
  summary.push(`### Wrong version\n\n\`\`\`\n${m.lines.join("\n")}\n\`\`\``);

  // 4. A signature from another key.
  // (signed here with the second throwaway key, which the app does not know)
  const wrongSig = (() => {
    const file = join(work, "wrong-key-payload.bin");
    writeFileSync(file, goodPayload());
    rmSync(`${file}.sig`, { force: true });
    sh("npx", ["tauri", "signer", "sign", "--private-key-path", otherKeyFile, "--password", "throwaway", file]);
    return readFileSync(`${file}.sig`, "utf8").trim();
  })();
  publish({ version: NEW, payload: goodPayload(), signature: wrongSig, label: "wrongkey" });
  const w = await runCase("wrongkey", null, { finish: (l) => /^install-failed|^up-to-date/.test(l) }, { seconds: 90 });
  check("an update signed with another key is refused", has(w.lines, /^install-failed kind=InvalidSignature/));
  summary.push(`### Another key\n\n\`\`\`\n${w.lines.join("\n")}\n\`\`\``);

  // 5. The deb package (Linux): recognised, and only offered as a notice.
  if (linux && withDeb && artifacts[OLD].deb) {
    publish({ version: NEW, payload: goodPayload(), signature: goodSignature(), label: "deb" });
    const asRoot = process.getuid?.() === 0;
    const dpkgArgs = ["-i", "--force-depends", artifacts[OLD].deb];
    const dpkg = asRoot
      ? spawnSync("dpkg", dpkgArgs, { stdio: "inherit" })
      : spawnSync("sudo", ["dpkg", ...dpkgArgs], { stdio: "inherit" });
    check("the deb package installs", dpkg.status === 0);
    for (const variant of ["plain", "inherited-appimage-variables"]) {
      const dir = join(work, "run", `deb-${variant}`);
      const log = join(dir, "driver.log");
      rmSync(dir, { recursive: true, force: true });
      mkdirSync(join(dir, "home"), { recursive: true });
      const env = appEnv(join(dir, "home"), log);
      if (variant !== "plain") {
        // Started from a terminal that is itself an AppImage: its variables are inherited.
        const fake = join(dir, "Terminal.AppImage");
        writeFileSync(fake, "x");
        env.APPIMAGE = fake;
        env.APPDIR = join(dir, "mount");
      }
      const child = spawn("/usr/bin/patient-partridge", [pdf], { env, stdio: "ignore", detached: true });
      child.unref();
      await waitFor(log, (l) => /^manual-notice|^install-failed|^up-to-date/.test(l), 90);
      const lines = readLog(log);
      console.log(lines.map((l) => `    ${l}`).join("\n"));
      killApps();
      check(`the deb package is recognised as manual (${variant})`, has(lines, /^started version=0\.0\.1 method=Manual/));
      check(`it is offered a notice and nothing is installed (${variant})`, has(lines, /^manual-notice version=0\.0\.2/) && !has(lines, /documents open before the stop/));
      summary.push(`### deb (${variant})\n\n\`\`\`\n${lines.join("\n")}\n\`\`\``);
    }
  }
} finally {
  killApps();
  server.close();
  xvfb?.kill();
  const failed = results.filter((r) => !r.ok);
  const text = [
    `## Update test on ${platformKey()}`,
    "",
    ...results.map((r) => `- ${r.ok ? "PASS" : "FAIL"} ${r.name}${r.detail ? ` (${r.detail})` : ""}`),
    "",
    ...summary,
    "",
  ].join("\n");
  writeFileSync(join(work, "summary.md"), text);
  console.log(`\n${results.length - failed.length} of ${results.length} checks passed`);
  if (failed.length || results.length === 0) process.exitCode = 1;
}

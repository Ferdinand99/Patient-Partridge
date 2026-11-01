#!/usr/bin/env node
// The last step of a release build, the only one that has the signing secrets. It is started by
// tauri-action as its "tauri script" (`node scripts/release-build.mjs build <arguments of tauri build>`),
// and by the macOS signing check in the same way.
//
// Everything that runs third-party code has already run, in steps that have no secret: `npm ci`,
// the build scripts of the Rust dependencies and Vite (`tauri build --no-bundle`), the bundling of
// the Windows installers and the Linux packages (`tauri bundle`, with the tools it downloads
// checked against hashes). What is left is signing:
//
//   Windows, Linux: the update file is the installer or the AppImage itself. It is signed with the
//                   update key here (`tauri signer sign`, bound to the version).
//   macOS:          the update file is an archive of the app that the bundler makes after the app is
//                   signed and notarised, so the bundler runs here (`tauri bundle`), with the update
//                   key and the Apple secrets. Before it, the certificate is put into a temporary
//                   keychain and the PDFium library, which Tauri does not sign (it is a resource), is
//                   signed with the hardened runtime. After it, the disk image is notarised and
//                   stapled too.
//
// What cannot be split: the Tauri CLI itself (an npm package with a native program) holds the
// secrets while it signs and notarises. That is the code that must have them.

import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { codesignArgs } from "./bundle-config.mjs";
import { bundleDirFor, notaryAccepted, notaryArgs, payloadsFor, shellArg, signerArgs } from "./release-sign.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const win = process.platform === "win32";

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", shell: win, ...options });
  if (result.status !== 0) throw new Error(`${command} ${args[0] ?? ""} failed (${result.status})`);
}

const tauri = (args, env = process.env) => run("npx", ["tauri", ...args].map((a) => shellArg(a, win)), { env });

function macOs(args, env) {
  const identity = env.APPLE_SIGNING_IDENTITY;
  if (env.APPLE_CERTIFICATE) {
    run("bash", ["scripts/macos-import-certificate.sh"], { env, shell: false });
    // The key file for notarisation: written here, from the secret, for the bundler and notarytool.
    const keyPath = join(env.RUNNER_TEMP ?? tmpdir(), `AuthKey_${env.APPLE_API_KEY}.p8`);
    writeFileSync(keyPath, env.APPLE_API_KEY_CONTENT ?? "", { mode: 0o600 });
    chmodSync(keyPath, 0o600);
    env = { ...env, APPLE_API_KEY_PATH: keyPath };
    const library = join(root, "vendor", "pdfium", "mac-universal", "libpdfium.dylib");
    if (!existsSync(library)) throw new Error(`${library} is not there: run prepare-bundle with PARTRIDGE_UNIVERSAL=1 first`);
    run("codesign", codesignArgs(identity, library), { env, shell: false });
    console.log("Signed libpdfium.dylib (hardened runtime).");
  } else {
    console.warn("APPLE_CERTIFICATE is not set: this build is not signed.");
  }

  tauri(["bundle", ...args], env);

  if (!env.APPLE_CERTIFICATE) return;
  const targetDir = process.env.CARGO_TARGET_DIR ?? join(root, "target");
  const dmgDir = join(bundleDirFor(args, targetDir), "dmg");
  const dmgs = readdirSync(dmgDir).filter((n) => n.endsWith(".dmg"));
  if (dmgs.length === 0) throw new Error(`no disk image in ${dmgDir}`);
  for (const name of dmgs) {
    const dmg = join(dmgDir, name);
    // Tauri signs the disk image but notarises only the app. A signed disk image that is not
    // notarised is "Unnotarized Developer ID" to Gatekeeper.
    const submitted = spawnSync("xcrun", notaryArgs(dmg, env), { cwd: root, encoding: "utf8", env });
    process.stdout.write(submitted.stdout ?? "");
    process.stderr.write(submitted.stderr ?? "");
    if (submitted.status !== 0 || !notaryAccepted(submitted.stdout ?? "")) throw new Error(`the disk image ${name} was not accepted by the notary service`);
    run("xcrun", ["stapler", "staple", dmg], { env, shell: false });
    console.log(`Notarised and stapled ${name}.`);
  }
}

/** The version of the app: tauri.conf.json, unless a config file in the arguments sets another. */
function versionFor(args) {
  let version = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8")).version;
  args.forEach((a, i) => {
    if (args[i - 1] !== "--config" || !existsSync(a)) return;
    const extra = JSON.parse(readFileSync(a, "utf8"));
    if (typeof extra.version === "string") version = extra.version;
  });
  return version;
}

function signPayloads(args, env) {
  const version = versionFor(args);
  const targetDir = process.env.CARGO_TARGET_DIR ?? join(root, "target");
  const files = payloadsFor(process.platform, bundleDirFor(args, targetDir));
  if (files.length === 0) throw new Error(`no installer to sign in ${bundleDirFor(args, targetDir)}`);
  for (const file of files) {
    tauri(signerArgs(file, version), env);
    if (!existsSync(`${file}.sig`)) throw new Error(`${file}.sig was not made`);
    console.log(`Signed ${file}`);
  }
}

const [command, ...args] = process.argv.slice(2);
if (command !== "build") {
  console.error("usage: release-build.mjs build <arguments of tauri build>");
  process.exit(2);
}
try {
  if (process.platform === "darwin") macOs(args, process.env);
  else signPayloads(args, process.env);
} catch (error) {
  console.error(`::error title=Release build::${error.message}`);
  process.exit(1);
}

// The parts of a release build that need a secret, as plain functions that can be tested. The build
// itself (which runs the build scripts of every dependency) is done in earlier steps that have no
// secret; see scripts/release-build.mjs.

import { readdirSync } from "node:fs";
import { join } from "node:path";

/** The folder of the bundles, from the arguments given to `tauri build` and the Cargo target folder. */
export function bundleDirFor(args, targetDir) {
  const at = args.indexOf("--target");
  const target = at >= 0 ? args[at + 1] : undefined;
  return join(targetDir, ...(target ? [target] : []), "release", "bundle");
}

/** The value of `--flag value` in an argument list, or undefined. */
export function optionOf(args, flag) {
  const at = args.indexOf(flag);
  return at >= 0 ? args[at + 1] : undefined;
}

/**
 * The files that are the update file themselves and have to be signed after they are made: the
 * NSIS installer and the MSI on Windows, the AppImage on Linux. (On macOS the update file is an
 * archive that the bundler makes and signs.) A system that has none of them is an error for the
 * caller to report, not an empty success.
 */
export function payloadsFor(platform, bundleDir) {
  const list = (folder, test) => {
    try {
      return readdirSync(join(bundleDir, folder))
        .filter(test)
        .sort()
        .map((n) => join(bundleDir, folder, n));
    } catch {
      return [];
    }
  };
  if (platform === "win32") return [...list("nsis", (n) => n.endsWith("-setup.exe")), ...list("msi", (n) => n.endsWith(".msi"))];
  if (platform === "linux") return list("appimage", (n) => n.endsWith(".AppImage"));
  return [];
}

/** Arguments of `tauri signer sign`. The signature is bound to the version it is made for. */
export function signerArgs(file, version) {
  if (!/^\d+\.\d+\.\d+([-+].+)?$/.test(version)) throw new Error(`Not a version: ${version}`);
  return ["signer", "sign", "--app-version", version, file];
}

/** Arguments of `xcrun notarytool submit`, with the key file and the ids from the environment. */
export function notaryArgs(file, env) {
  for (const name of ["APPLE_API_KEY_PATH", "APPLE_API_KEY", "APPLE_API_ISSUER"]) {
    if (!env[name]) throw new Error(`${name} is not set`);
  }
  return [
    "notarytool",
    "submit",
    file,
    "--key",
    env.APPLE_API_KEY_PATH,
    "--key-id",
    env.APPLE_API_KEY,
    "--issuer",
    env.APPLE_API_ISSUER,
    "--wait",
    "--output-format",
    "json",
  ];
}

/** True when the JSON that `notarytool submit --wait` printed says the file was accepted. */
export function notaryAccepted(text) {
  try {
    return JSON.parse(text).status === "Accepted";
  } catch {
    return false;
  }
}

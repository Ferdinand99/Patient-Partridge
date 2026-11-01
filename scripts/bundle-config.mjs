// What the installer build needs besides tauri.conf.json: the engine as a sidecar, the PDFium
// library and its licences as resources, and whether update files (and their signatures) are made.
// Pure functions, so that they can be tested without building anything.

/** The two Rust targets that make a universal macOS app. */
export const MAC_TRIPLES = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

/** The name Tauri expects for the sidecar of a universal macOS build. */
export const UNIVERSAL_TRIPLE = "universal-apple-darwin";

/**
 * Whether the bundler itself makes the update file and signs it (`createUpdaterArtifacts`). It is
 * a plain flag, not a test for the signing key: the key must not be in the environment of the
 * steps that prepare the build. Only the macOS bundle works this way (its update file is an archive
 * of the app that must be made after notarisation). For the other systems the update file is the
 * installer itself, which `scripts/release-sign.mjs` signs in a separate step.
 */
export function wantsUpdaterArtifacts(env) {
  return env.PARTRIDGE_UPDATER_ARTIFACTS === "1";
}

/**
 * The config merged over tauri.conf.json. `folder` is the folder in vendor/pdfium that holds the
 * library to ship ("linux-x64", ..., or "mac-universal" for a universal macOS app), `file` its name.
 * Paths in `resources` are relative to src-tauri; the map form sets where each file lands.
 */
export function bundleConfig({ folder, file, env }) {
  return {
    bundle: {
      createUpdaterArtifacts: wantsUpdaterArtifacts(env),
      externalBin: ["binaries/partridge-engine"],
      resources: {
        [`../vendor/pdfium/${folder}/${file}`]: file,
        [`../vendor/pdfium/${folder}/LICENSE`]: "pdfium-LICENSE",
        [`../vendor/pdfium/${folder}/licenses`]: "pdfium-licenses",
      },
    },
  };
}

/** Arguments of `codesign` for a file that goes into the app: hardened runtime, secure timestamp. */
export function codesignArgs(identity, file) {
  if (!identity) throw new Error("No signing identity.");
  return ["--force", "--timestamp", "--options", "runtime", "--sign", identity, file];
}

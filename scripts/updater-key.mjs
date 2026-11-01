// Checks that a signature made with the updater signing key is one that the public key in
// tauri.conf.json accepts. The release workflow signs a small file with the secret key and checks
// the result here before anything is built: with a key that does not belong to the public key,
// every installed copy would refuse the update, and could never get a working one.
//
// The format is minisign's. A Tauri public key and signature are base64 text of a minisign file.
// A signature file has a comment line and then base64 of: algorithm (2 bytes: "Ed" signs the file,
// "ED" signs its BLAKE2b-512 hash), the key id (8), and the signature (64). A public key file has
// base64 of: "Ed" (2), the key id (8), the Ed25519 key (32).
//
//   node scripts/updater-key.mjs <signed file> <its .sig file> [tauri.conf.json]

import { createPublicKey, verify, createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** The base64 line of a minisign file (the second line) as bytes. */
function blobOf(text) {
  const lines = text.replace(/\r/g, "").split("\n").filter(Boolean);
  if (lines.length < 2) throw new Error("not a minisign file");
  return Buffer.from(lines[1], "base64");
}

/** `tauriBase64` is what Tauri keeps: the base64 text of the whole minisign file. */
function decodeTauri(tauriBase64) {
  return Buffer.from(tauriBase64.trim(), "base64").toString("utf8");
}

export function parsePublicKey(tauriBase64) {
  const blob = blobOf(decodeTauri(tauriBase64));
  if (blob.length !== 42 || blob.toString("latin1", 0, 2) !== "Ed") throw new Error("not a minisign public key");
  return { id: blob.subarray(2, 10), key: blob.subarray(10, 42) };
}

export function parseSignature(tauriBase64) {
  const blob = blobOf(decodeTauri(tauriBase64));
  const algorithm = blob.toString("latin1", 0, 2);
  if (blob.length !== 74 || (algorithm !== "Ed" && algorithm !== "ED")) throw new Error("not a minisign signature");
  return { algorithm, id: blob.subarray(2, 10), signature: blob.subarray(10, 74) };
}

/** True when `signatureBase64` is a valid signature of `data` by the key `publicKeyBase64`. */
export function signatureMatches(data, signatureBase64, publicKeyBase64) {
  const pub = parsePublicKey(publicKeyBase64);
  const sig = parseSignature(signatureBase64);
  if (!pub.id.equals(sig.id)) return false;
  const spki = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), pub.key]);
  const key = createPublicKey({ key: spki, format: "der", type: "spki" });
  const message = sig.algorithm === "ED" ? createHash("blake2b512").update(data).digest() : data;
  return verify(null, message, key, sig.signature);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [, , file, sigFile, conf = "src-tauri/tauri.conf.json"] = process.argv;
  if (!file || !sigFile) {
    console.error("usage: updater-key.mjs <file> <file.sig> [tauri.conf.json]");
    process.exit(2);
  }
  const pubkey = JSON.parse(readFileSync(conf, "utf8")).plugins?.updater?.pubkey ?? "";
  let ok = false;
  try {
    ok = signatureMatches(readFileSync(file), readFileSync(sigFile, "utf8"), pubkey);
  } catch (e) {
    console.error(`::error title=Updater key::${e.message}`);
    process.exit(1);
  }
  if (!ok) {
    console.error(
      "::error title=Updater key::The signing key in the repository secrets does not belong to plugins.updater.pubkey in src-tauri/tauri.conf.json. Installed copies would refuse every update. Nothing was built.",
    );
    process.exit(1);
  }
  console.log("The signing key belongs to the public key in tauri.conf.json.");
}

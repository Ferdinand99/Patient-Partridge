import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { parsePublicKey, parseSignature, signatureMatches } from "./updater-key.mjs";

/** A key pair and a way to sign, in the formats Tauri keeps (base64 of the minisign text files). */
function makeKey(idByte) {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const raw = publicKey.export({ format: "der", type: "spki" }).subarray(-32);
  const id = Buffer.alloc(8, idByte);
  const pubBlob = Buffer.concat([Buffer.from("Ed"), id, raw]);
  const publicKeyText = `untrusted comment: minisign public key: ${Buffer.from(id).reverse().toString("hex").toUpperCase()}\n${pubBlob.toString("base64")}\n`;
  const signFile = (data, algorithm = "ED") => {
    const message = algorithm === "ED" ? createHash("blake2b512").update(data).digest() : data;
    const signature = sign(null, message, privateKey);
    const blob = Buffer.concat([Buffer.from(algorithm), id, signature]);
    const text = `untrusted comment: signature from tauri secret key\n${blob.toString("base64")}\ntrusted comment: timestamp:1\tfile:x\tversion:0.0.2\nAAAA\n`;
    return Buffer.from(text).toString("base64");
  };
  return { publicKey: Buffer.from(publicKeyText).toString("base64"), signFile };
}

const data = Buffer.from("a small file that is signed");

describe("signatureMatches", () => {
  it("accepts a signature of the file made by the key, in both minisign forms", () => {
    const k = makeKey(7);
    expect(signatureMatches(data, k.signFile(data), k.publicKey)).toBe(true);
    expect(signatureMatches(data, k.signFile(data, "Ed"), k.publicKey)).toBe(true);
  });

  it("refuses another key, a changed file and a signature that is not one", () => {
    const k = makeKey(7);
    const other = makeKey(8);
    expect(signatureMatches(data, other.signFile(data), k.publicKey)).toBe(false);
    expect(signatureMatches(Buffer.from("changed"), k.signFile(data), k.publicKey)).toBe(false);
    // The same key id but another key: the numbers are checked, not only the id.
    const sameId = makeKey(7);
    expect(signatureMatches(data, sameId.signFile(data), k.publicKey)).toBe(false);
    expect(() => signatureMatches(data, "bm90IGEgc2lnbmF0dXJl", k.publicKey)).toThrow();
  });
});

describe("the public key of the app", () => {
  it("is a minisign public key", () => {
    const conf = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
    const { id, key } = parsePublicKey(conf.plugins.updater.pubkey);
    expect(id.length).toBe(8);
    expect(key.length).toBe(32);
  });

  it("is refused when it is not", () => {
    expect(() => parsePublicKey("bm90IGEga2V5")).toThrow();
    expect(() => parseSignature("bm90IGEga2V5")).toThrow();
  });
});

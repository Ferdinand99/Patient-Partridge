// Signing a document with a certificate: the words, and the small rules behind them.
//
// Everything the window says about it is here, so that one test can read all of it. Two rules hold:
//  - Nothing promises what the signature is worth. The window says what the program did (it added a
//    signature made with the key in the certificate file) and what a program that opens the file
//    can check (that the file has not changed since). It never says that anyone will accept it.
//  - Text from the certificate file (a name, an issuer) is shown as text, in a field of its own.

import type { AppErrorLike } from "./errors";

/** What the app process says about the certificate that was opened. */
export interface CertificateInfo {
  name: string;
  issuer: string;
  selfSigned: boolean;
  serial: string;
  fingerprint: string;
  /** First and last second of the period the certificate is in use, seconds since 1970. */
  notBefore: number;
  notAfter: number;
  /** The time on this computer when the file was opened, seconds since 1970. */
  now: number;
  /** `RSA 3072 bit`, `ECDSA P-256`. */
  key: string;
  /** False when the certificate says it is not meant for signing documents. */
  maySign: boolean;
  chain: number;
}

/** A signature field of the document that nobody has signed. */
export interface SignField {
  page: number;
  /** `[left, top, right, bottom]` in points from the top left of the page as it is drawn. */
  rect: [number, number, number, number];
  name: string;
}

export type SignPlace =
  | { kind: "invisible" }
  | { kind: "rect"; page: number; rect: [number, number, number, number] }
  | { kind: "field"; page: number; name: string };

export type SignMark =
  | { kind: "none" }
  | { kind: "draw"; width: number; height: number; line: number; strokes: [number, number][][] }
  | { kind: "type"; font: string; text: string }
  | { kind: "image" };

export interface SignRequestArgs {
  place: SignPlace;
  reason: string;
  location: string;
  mark: SignMark;
  color: [number, number, number, number];
}

/** The file a signature was written to. */
export interface SignedFile {
  path: string;
  name: string;
  field: string;
  signer: string;
  fingerprint: string;
}

/** The size of the box of a new signature, in points, when it is placed with a click. */
export const DEFAULT_BOX = { width: 220, height: 70 } as const;
/** The smallest box the engine takes, in points on a side. */
export const MIN_BOX_SIDE = 24;

/** Longest name, reason and place, in characters (the engine takes 256 bytes). */
export const MAX_TEXT_CHARS = 80;

export const CERT_WORDS = {
  tool: "Sign with a certificate",
  title: "Sign with a certificate",
  intro:
    "Patient Partridge adds a digital signature made with the key in your certificate file. Programs that open the file can check that it has not changed since, and show the name in your certificate. Patient Partridge does not say whether anyone will accept the signature.",
  chooseFile: "Choose certificate file…",
  fileHint: "A file ending in .p12 or .pfx.",
  password: "Password of the certificate file",
  passwordHint: "The password opens the certificate file. It is not saved.",
  open: "Open the file",
  notKept: "Patient Partridge does not keep the certificate or the password.",
  nameLabel: "Name",
  issuerLabel: "Issued by",
  periodLabel: "In use",
  keyLabel: "Key",
  chainLabel: "Certificates in the signature",
  selfMade:
    "This certificate was made by its owner. Programs that open the file will usually show the signer as unknown.",
  expired:
    "This certificate has expired. Programs that open the file will probably report a problem.",
  notYet:
    "This certificate is not in use yet. Programs that open the file will probably report a problem.",
  notForSigning:
    "This certificate says it is not meant for signing documents. Programs that open the file may report a problem.",
  clock: "The signing time is taken from this computer's clock. Nobody else has confirmed it.",
  placeTitle: "Where the signature shows",
  placeBox: "In a box on a page",
  placeField: "In a signature field of this file",
  placeNone: "No box on any page",
  placeBoxHint: "Drag on the page to draw the box.",
  placeFieldHint: "Choose a field.",
  pageLabel: "Page",
  textsTitle: "What the signature says",
  signerName: "The signature carries the name in the certificate.",
  keyLetGo: "The key is no longer open. Type the password again to try once more.",
  reason: "Reason (optional)",
  location: "Place (optional)",
  pictureTitle: "Picture of your signature (optional)",
  pictureAdd: "Add a picture…",
  pictureChange: "Change the picture…",
  pictureRemove: "Take the picture away",
  pictureNote:
    "A picture is only a picture. It does not show who placed it, and anyone can copy it.",
  afterSigning:
    "After signing, changing the file breaks the signature. To change it, save a new copy.",
  fileBeing: (name: string, pages: number) =>
    `The file you are signing: ${name}, ${pages} ${pages === 1 ? "page" : "pages"}.`,
  sign: "Sign and save a copy…",
  cancel: "Cancel",
  close: "Close",
  working: "Signing…",
  openWorking: "Opening the certificate file…",
  readOnly:
    "This file is signed. Changing it would break the signature, so annotations, form fields and page changes are off here, and it cannot be saved over. To change it, save a copy.",
  signedTitle: "This file is signed",
  signedOff: (what: string) =>
    `This file is signed, so ${what} are off here: changing it would break the signature. To change it, save a copy.`,
  signedSaveOnlyCopy: (name: string) =>
    `"${name}" is the signed file. Saving over it would break the signature, so it is not done. A copy leaves the signed file as it is.`,
  unsavedTitle: "Save the document first",
  unsavedDetail: (name: string) =>
    `"${name}" has changes that are not saved. Patient Partridge signs a file you have saved. Save it, then sign.`,
  unsavedSave: "Save and go on",
  cannotTitle: "This document cannot be signed",
  doneTitle: "Signed copy saved",
  done: (name: string) =>
    `Signed copy saved as "${name}". Signature intact; signer not verified. Patient Partridge does not check certificates against a list of trusted issuers.`,
  doneOpened: "The signed copy is open in a new tab.",
  choosePageFirst: "Draw the box for the signature on the page first.",
  noFields: "This file has no empty signature field.",
  previewFailed: "The page could not be shown. Pick the field or no box instead.",
} as const;

/** All the fixed words of the dialog, for the test that reads every one of them. */
export function allCertWords(): string[] {
  return Object.values(CERT_WORDS).flatMap((w) => (typeof w === "function" ? [w("a.pdf", 2), w("a.pdf", 1)] : [w]));
}

/** What to tell about a certificate before it is used: one line for each thing that is off. */
export function certificateWarnings(c: CertificateInfo): string[] {
  const lines: string[] = [];
  if (c.selfSigned) lines.push(CERT_WORDS.selfMade);
  if (c.now > c.notAfter) lines.push(CERT_WORDS.expired);
  else if (c.now < c.notBefore) lines.push(CERT_WORDS.notYet);
  if (!c.maySign) lines.push(CERT_WORDS.notForSigning);
  return lines;
}

/** A moment as `2026-01-31`, in UTC. */
export function dayOf(seconds: number): string {
  if (!Number.isFinite(seconds)) return "";
  const d = new Date(seconds * 1000);
  if (Number.isNaN(d.getTime())) return "";
  return d.toISOString().slice(0, 10);
}

/** The period a certificate is in use, as text. */
export function periodText(c: CertificateInfo): string {
  return `${dayOf(c.notBefore)} to ${dayOf(c.notAfter)}`;
}

// Control characters, line and paragraph separators, and the characters that hide text or steer it.
// eslint-disable-next-line no-control-regex
const HIDDEN = new RegExp("[\u0000-\u001f\u007f-\u009f\u2028\u2029\u200b-\u200f\u202a-\u202e\u2066-\u2069\ufeff]", "g");

/** The text of a name field: one line, no control characters, no longer than the cap. */
export function cleanText(text: string): string {
  const plain = text.replace(HIDDEN, " ");
  return plain.replace(/\s+/g, " ").trim().slice(0, MAX_TEXT_CHARS);
}

/**
 * A box of the default size with the point `(x, y)` (in points on the page as it is drawn) as its
 * centre, kept inside the page. `null` when the page is too small to hold a box.
 */
export function boxAround(
  x: number,
  y: number,
  page: { width: number; height: number },
): [number, number, number, number] | null {
  const w = Math.min(DEFAULT_BOX.width, page.width);
  const h = Math.min(DEFAULT_BOX.height, page.height);
  if (w < MIN_BOX_SIDE || h < MIN_BOX_SIDE) return null;
  const left = Math.min(Math.max(x - w / 2, 0), page.width - w);
  const top = Math.min(Math.max(y - h / 2, 0), page.height - h);
  return [left, top, left + w, top + h];
}

/** The box between two corners, in order, kept inside the page, or `null` when it is too small. */
export function boxBetween(
  a: [number, number],
  b: [number, number],
  page: { width: number; height: number },
): [number, number, number, number] | null {
  const clamp = (v: number, max: number) => Math.min(Math.max(v, 0), max);
  const left = clamp(Math.min(a[0], b[0]), page.width);
  const right = clamp(Math.max(a[0], b[0]), page.width);
  const top = clamp(Math.min(a[1], b[1]), page.height);
  const bottom = clamp(Math.max(a[1], b[1]), page.height);
  if (right - left < MIN_BOX_SIDE || bottom - top < MIN_BOX_SIDE) return null;
  return [left, top, right, bottom];
}

export interface Told {
  title: string;
  detail: string;
}

/** What to tell about a failure of opening the certificate file or of signing, by the code. */
export function describeSignError(error: AppErrorLike, fileName = "this file"): Told {
  const more = error.message && error.code === "sign_refused" ? ` ${error.message}` : "";
  switch (error.code) {
    case "signer_wrong_password":
      return { title: "That password is not right", detail: "Check the password and try again." };
    case "signer_not_a_certificate_file":
      return {
        title: "This is not a certificate file",
        detail: "The file is not a certificate file (.p12 or .pfx), or it is damaged.",
      };
    case "signer_no_key":
      return {
        title: "No key in this file",
        detail: "The file holds no private key with a certificate. Choose a file that has both.",
      };
    case "signer_key_unsupported": {
      // The signer says why in the last pair of brackets of its message (the kind of key).
      const why = /\(([^()]{1,120})\)\s*$/.exec(error.message)?.[1];
      return {
        title: "This kind of key is not supported",
        detail: `Patient Partridge signs with RSA keys of 2048 to 4096 bits and with ECDSA keys on the curves P-256 and P-384.${why ? ` This file: ${cleanText(why)}.` : ""}`,
      };
    }
    case "signer_protection_unsupported":
      return {
        title: "This certificate file cannot be opened here",
        detail: "The file is protected in a way that Patient Partridge does not open (a method it does not know, or more than a million rounds of work). Export the certificate again with the usual protection.",
      };
    case "signer_chain_too_long":
      return {
        title: "The chain of certificates is too long",
        detail: "The file leads through more than 12 certificates, which is more than a signature carries here. Export the certificate with a shorter chain.",
      };
    case "signer_password_too_long":
      return { title: "The password is too long", detail: "A password of more than 1,024 bytes is not taken. Check the password." };
    case "signer_key_mismatch":
      return {
        title: "The key and the certificate do not belong together",
        detail: "The private key in the file does not fit any certificate in the file.",
      };
    case "signer_too_large":
      return { title: "The certificate file is too big", detail: "This is not a certificate file, or it holds too much." };
    case "sign_file_unreadable":
    case "sign_no_file":
      return { title: "The certificate file could not be read", detail: "Choose the file again." };
    case "signer_not_loaded":
      return { title: "Open the certificate file first", detail: "Choose the file and type its password." };
    case "signer_sandbox_unavailable":
    case "signer_stopped":
    case "signer_internal":
    case "signer_invalid_request":
    case "signer_protocol_mismatch":
      return {
        title: "The signing program stopped",
        detail: "Nothing was signed. Try again. If it keeps happening, restart Patient Partridge.",
      };
    case "sign_unsaved":
      return {
        title: "Save the document first",
        detail: `${fileName} has changes that are not saved. Save it, then sign.`,
      };
    case "sign_target_open":
      return {
        title: "That file is open",
        detail: "The name you chose is a file that is open in another tab, and the signed copy would replace it. Close that tab, or choose another name.",
      };
    case "sign_same_file":
      return {
        title: "Choose another name",
        detail: "The signed copy cannot replace the file that is open. Give the copy a name of its own.",
      };
    case "sign_encrypted":
    case "save_metadata_not_encrypted":
      return {
        title: "This document is protected with a password",
        detail: "A document that is protected with a password cannot be signed here.",
      };
    case "sign_xfa":
      return {
        title: "This document has an XFA form",
        detail: "Patient Partridge cannot sign a document with an XFA form.",
      };
    case "sign_already_signed":
      return {
        title: "This file already has signatures",
        detail: "Signing it again would remove them, so Patient Partridge does not do it in this version.",
      };
    case "sign_structure":
      return {
        title: "This document cannot be signed",
        detail: "The document is built in a way that Patient Partridge cannot add a signature to. This says nothing about the document itself.",
      };
    case "sign_refused":
    case "page_out_of_range":
      return { title: "The signature could not be placed", detail: `Pick the box or the field again.${more}` };
    case "save_too_large":
      return { title: "The document is too big to sign", detail: "The document with the signature would be bigger than Patient Partridge writes." };
    default:
      return { title: "Nothing was signed", detail: "The signed copy could not be written. Nothing was changed." };
  }
}

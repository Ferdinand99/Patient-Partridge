// What the window says about digital signatures.
//
// The engine decides the state of a signature and gives the reason as a short code. Every word the
// reader sees about a state or a reason is written here, in the window. A PDF cannot change them:
// the text it does control (names, reasons, places) is shown apart, in the facts, as text.

/** The only states a signature can be in. */
export type SignatureState = "trusted" | "unknown_signer" | "changed" | "broken" | "unsupported";

/** How a signature's certificate stands against the clock of this computer. */
export type CertificateTime = "unknown" | "valid" | "expired" | "not_yet_valid";

/** One signature, as the app process hands it to the window. */
export interface SignatureItem {
  index: number;
  state: SignatureState;
  reason: string;
  certificateTime: CertificateTime;
  subFilter: string;
  byteRange: number[];
  fileLen: number;
  docMdp: number;
  certificates: number;
  signer: string | null;
  organisation: string | null;
  issuer: string | null;
  serial: string | null;
  fingerprint: string | null;
  validFrom: string | null;
  validTo: string | null;
  key: string | null;
  digest: string | null;
  claimedTime: string | null;
  statedReason: string | null;
  statedLocation: string | null;
  trustAnchor: string | null;
}

export interface SignaturesInfo {
  /** How many signatures the PDF library lists in all (the items are the first of them). */
  total: number;
  /** Places in the file that look like a signature but are not listed. */
  unlisted: number;
  /** Certificates on the trust list that ships with the app. */
  trustAnchors: number;
  items: SignatureItem[];
}

/** The words for each state. Plain, and the same everywhere they are used. */
export const STATE_LABEL: Record<SignatureState, string> = {
  trusted: "Signature intact; signer trusted",
  unknown_signer: "Signature intact; signer not verified",
  changed: "Changed after signing",
  broken: "Signature broken",
  unsupported: "Signature not checked",
};

/** What each state means, in a sentence. */
export const STATE_MEANING: Record<SignatureState, string> = {
  trusted:
    "The part of the file this signature covers is as it was when it was signed, and the signer's certificate leads to a certificate on this app's trust list.",
  unknown_signer:
    "The part of the file this signature covers is as it was when the holder of the key signed it. Who that is has not been verified.",
  changed: "The file is not the same as the one that was signed.",
  broken: "The signature does not hold. Nothing in the file can be taken as signed by it.",
  unsupported: "This version cannot check this signature. That says nothing for or against it.",
};

/** The reasons, by the code the engine sends. */
export const REASON_TEXT: Record<string, string> = {
  verified: "The signature is intact and the signer's certificate leads to a certificate on the trust list.",
  data_after_signature:
    "The file goes on after the part this signature covers. A later signature, a filled-in form or any other edit adds content like this, and this signature does not vouch for it.",
  content_changed: "The part of the file this signature covers is not what the signer signed.",
  signature_invalid: "The signature does not match the signer's key and the data it signs.",
  range_invalid: "The signature does not say correctly which part of the file it covers.",
  range_leaves_gap:
    "The signature leaves out more of the file than the space for itself, so it cannot vouch for the whole file.",
  contents_mismatch: "The signature that the PDF library lists is not the one stored in the file at the signed place.",
  contents_padding: "Other data follows the signature inside the space made for it.",
  unreadable: "The signature data cannot be read.",
  no_signer_certificate: "The signature does not carry the certificate of the signer.",
  certificate_mismatch: "The certificate in the signature is not the one the signer named.",
  attributes_invalid: "The signed attributes are missing, repeated or wrong.",
  sub_filter_unsupported: "This form of signature is not checked by this version.",
  document_timestamp: "This is a document time stamp, not a signature. Time stamps are not checked.",
  digest_unsupported: "The digest algorithm is not supported or is no longer safe (for example SHA-1).",
  algorithm_unsupported: "The signature algorithm or the kind of key is not supported.",
  key_too_small: "The key is too short to be checked safely.",
  no_signed_attributes: "The signature has no signed attributes. That form is not checked.",
  encoding_unsupported: "The signature is not encoded in the strict form (DER) that this version requires.",
  too_large: "The signature or the file is too large for the limits of the check.",
  no_trust_list:
    "This version of Patient Partridge ships no trust list, so no signer can be verified. The signature itself is intact.",
  not_in_trust_list: "The signer's certificate does not lead to a certificate on the trust list.",
  certificate_expired: "A certificate in the chain has expired.",
  certificate_not_yet_valid: "A certificate in the chain is not valid yet.",
  key_usage: "The signer's certificate is not meant for signing documents.",
};

/** The reason for a code; a code this window does not know gets a plain line. */
export function reasonText(code: string): string {
  return REASON_TEXT[code] ?? "No reason is given for this state.";
}

/** The states this window knows. A state it does not know is shown as "not checked", never as intact. */
export function knownState(state: string): SignatureState {
  return state === "trusted" ||
    state === "unknown_signer" ||
    state === "changed" ||
    state === "broken" ||
    state === "unsupported"
    ? state
    : "unsupported";
}

/** What the signer's certificate says against the clock, in words, or null when nothing is worth saying. */
export function certificateTimeText(time: CertificateTime): string | null {
  switch (time) {
    case "expired":
      return "The signer's certificate has expired by this computer's clock.";
    case "not_yet_valid":
      return "The signer's certificate is not valid yet by this computer's clock.";
    default:
      return null;
  }
}

/** What this version does not check. Shown in the panel; also in the README. */
export const NOT_CHECKED: readonly string[] = [
  "Whether a certificate was revoked. No network is used, so no revocation list is asked.",
  "Time stamps. The time a signature gives is only what the signer's computer said.",
  "Long-term validation (the data that keeps a signature checkable after its certificates expire).",
  "Policies, name limits and the extended use of certificates.",
  "Whether a change after signing was allowed by the signer. Any later content is reported as a change.",
];

/** What the badge shows for a document. */
export interface Summary {
  /** "trusted" and the other states, or "marks" for a file with signature marks that are not listed. */
  kind: SignatureState | "marks";
  label: string;
  /** The number of signatures behind the label. */
  count: number;
}

/** The order of the states from the one that matters most: the badge shows the worst. */
const SEVERITY: SignatureState[] = ["broken", "changed", "unsupported", "unknown_signer", "trusted"];

/**
 * The badge for a document, or null when the document has no signatures and none that are hidden.
 * The label is the worst state of any signature: a document is not shown as better than its worst.
 */
export function summarise(info: SignaturesInfo): Summary | null {
  const states = info.items.map((i) => knownState(i.state));
  const worst = SEVERITY.find((s) => states.includes(s));
  if (!worst) {
    if (info.unlisted > 0) {
      return { kind: "marks", label: "Signature not found", count: info.unlisted };
    }
    return null;
  }
  // Marks that are not listed make the document at best "not checked".
  if (info.unlisted > 0 && (worst === "trusted" || worst === "unknown_signer")) {
    return { kind: "unsupported", label: STATE_LABEL.unsupported, count: info.total + info.unlisted };
  }
  return { kind: worst, label: STATE_LABEL[worst], count: Math.max(info.total, info.items.length) };
}

/** A sentence about the signatures that are not listed. */
export function unlistedText(count: number): string {
  return count === 1
    ? "The file holds one more place that looks like a signature. It is not listed as one, so it cannot be checked. A signature can be hidden this way."
    : `The file holds ${count} more places that look like signatures. They are not listed as signatures, so they cannot be checked. Signatures can be hidden this way.`;
}

/** A sentence about listed signatures beyond the ones described. */
export function omittedText(total: number, shown: number): string | null {
  return total > shown ? `${total - shown} more signatures are listed in the file but not described here.` : null;
}

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  DO_NOT_RELY,
  NOT_CHECKED,
  REASON_TEXT,
  STALE_TEXT,
  STATE_LABEL,
  STATE_MEANING,
  certificateTimeText,
  knownState,
  omittedText,
  reasonText,
  summarise,
  unlistedText,
  type SignatureItem,
  type SignatureState,
  type SignaturesInfo,
} from "./signatures";

function item(state: SignatureState, over: Partial<SignatureItem> = {}): SignatureItem {
  return {
    index: 0,
    state,
    reason: "no_trust_list",
    certificateTime: "valid",
    subFilter: "adbe.pkcs7.detached",
    byteRange: [0, 10, 20, 30],
    fileLen: 50,
    docMdp: 0,
    certificates: 1,
    signer: null,
    organisation: null,
    issuer: null,
    serial: null,
    fingerprint: null,
    validFrom: null,
    validTo: null,
    key: null,
    digest: null,
    claimedTime: null,
    statedReason: null,
    statedLocation: null,
    trustAnchor: null,
    ...over,
  };
}

function info(states: SignatureState[], over: Partial<SignaturesInfo> = {}): SignaturesInfo {
  return {
    total: states.length,
    unlisted: 0,
    trustAnchors: 0,
    items: states.map((s, i) => item(s, { index: i })),
    ...over,
  };
}

/** The names the Rust side gives to the members of an enum: the strings after `=>` in `name()`. */
function rustNames(enumName: string): string[] {
  const source = readFileSync("crates/protocol/src/signatures.rs", "utf8");
  const start = source.indexOf(`impl ${enumName} {`);
  const body = source.slice(start, source.indexOf("\n}\n", start));
  const name = body.slice(body.indexOf("pub fn name(self)"));
  return [...name.matchAll(/=> "([a-z_]+)"/g)].map((m) => m[1]);
}

describe("the words for the states and the reasons", () => {
  it("has a label and a meaning for exactly the states the engine has", () => {
    const states = rustNames("SignatureState");
    expect(states.length).toBe(5);
    expect(Object.keys(STATE_LABEL).sort()).toEqual([...states].sort());
    expect(Object.keys(STATE_MEANING).sort()).toEqual([...states].sort());
  });

  it("has a sentence for every reason the engine can give, and for no other", () => {
    const reasons = rustNames("SignatureReason");
    expect(reasons.length).toBe(25);
    expect(Object.keys(REASON_TEXT).sort()).toEqual([...reasons].sort());
    for (const text of Object.values(REASON_TEXT)) expect(text.length).toBeGreaterThan(20);
  });

  it("says plainly that an intact signature of an unverified signer is only that", () => {
    expect(STATE_LABEL.unknown_signer).toBe("Signature intact; signer not verified");
    // No label of a state other than trusted says trusted, verified or valid.
    for (const state of ["unknown_signer", "changed", "broken", "unsupported"] as const) {
      expect(STATE_LABEL[state]).not.toMatch(/trusted|valid|verified signer|verified by/i);
    }
    expect(STATE_LABEL.trusted).toMatch(/trusted/);
  });

  it("gives a plain line for a reason or a state it does not know", () => {
    expect(reasonText("something_new")).toBe("No reason is given for this state.");
    expect(reasonText("data_after_signature")).toMatch(/goes on after/);
    expect(knownState("trusted")).toBe("trusted");
    // A state this window does not know is never shown as an intact signature.
    expect(knownState("great")).toBe("unsupported");
    expect(knownState("")).toBe("unsupported");
  });

  it("mentions the time of the certificate only when it is not valid", () => {
    expect(certificateTimeText("valid")).toBeNull();
    expect(certificateTimeText("unknown")).toBeNull();
    expect(certificateTimeText("expired")).toMatch(/expired/);
    expect(certificateTimeText("not_yet_valid")).toMatch(/not valid yet/);
  });

  it("lists what is not checked: revocation, time stamps and long-term validation", () => {
    const text = NOT_CHECKED.join(" ");
    expect(text).toMatch(/revoked/);
    expect(text).toMatch(/Time stamps/);
    expect(text).toMatch(/Long-term validation/);
  });
});

describe("the badge", () => {
  it("is not there for a document without signatures", () => {
    expect(summarise(info([]))).toBeNull();
  });

  it("shows the worst state of the signatures", () => {
    expect(summarise(info(["unknown_signer"]))).toEqual({
      kind: "unknown_signer",
      label: "Signature intact; signer not verified",
      count: 1,
    });
    expect(summarise(info(["trusted", "unknown_signer"]))?.kind).toBe("unknown_signer");
    expect(summarise(info(["trusted", "unknown_signer", "changed"]))?.kind).toBe("changed");
    expect(summarise(info(["unknown_signer", "unsupported"]))?.kind).toBe("unsupported");
    expect(summarise(info(["trusted", "changed", "broken", "unsupported"]))?.kind).toBe("broken");
    expect(summarise(info(["trusted"]))?.kind).toBe("trusted");
  });

  it("is never better than the signatures it could not see", () => {
    // A mark that is not listed makes an intact signature a signature that was not checked.
    expect(summarise(info(["unknown_signer"], { unlisted: 1 }))?.kind).toBe("unsupported");
    expect(summarise(info(["trusted"], { unlisted: 2 }))?.kind).toBe("unsupported");
    // ... and does not hide a worse one.
    expect(summarise(info(["broken"], { unlisted: 1 }))?.kind).toBe("broken");
    expect(summarise(info(["changed"], { unlisted: 1 }))?.kind).toBe("changed");
  });

  it("is never better than the signatures that are listed and not described", () => {
    // 64 are described and intact, and 5 more are listed: the five were never looked at.
    const capped = info(Array(64).fill("unknown_signer"), { total: 69 });
    expect(summarise(capped)).toEqual({ kind: "unsupported", label: "Signature not checked", count: 69 });
    expect(summarise(info(Array(64).fill("trusted"), { total: 65 }))?.kind).toBe("unsupported");
    // Nothing unseen: all 64 listed are described.
    expect(summarise(info(Array(64).fill("unknown_signer"), { total: 64 }))?.kind).toBe("unknown_signer");
    // A worse state that was seen is not hidden.
    expect(summarise(info(["broken"], { total: 70 }))?.kind).toBe("broken");
    expect(summarise(info(["changed"], { total: 70 }))?.kind).toBe("changed");
    // Signatures listed and none described is not "no signature".
    expect(summarise(info([], { total: 3 }))?.kind).toBe("unsupported");
  });

  it("shows a file with marks of signatures that are not listed", () => {
    const summary = summarise(info([], { total: 0, unlisted: 1 }));
    expect(summary).toEqual({ kind: "marks", label: "Possible signature marks", count: 1 });
  });

  it("counts the signatures the library lists, also those not described", () => {
    expect(summarise(info(["broken"], { total: 70 }))?.count).toBe(70);
  });

  it("treats a state it does not know as not checked", () => {
    const odd = info(["unknown_signer"]);
    odd.items[0].state = "excellent" as SignatureState;
    expect(summarise(odd)?.kind).toBe("unsupported");
  });

  it("words the unlisted marks and the omitted signatures", () => {
    expect(unlistedText(1)).toMatch(/one more place/);
    expect(unlistedText(3)).toMatch(/3 more places/);
    expect(omittedText(64, 64)).toBeNull();
    expect(omittedText(70, 64)).toBe("6 more signatures are listed in the file but not described here.");
  });
});

describe("what is said where a signature could not be checked", () => {
  it("tells the reader not to rely on it, in plain words and with no promise", () => {
    expect(DO_NOT_RELY).toBe(
      "Do not rely on this signature. The file may have been changed, and changing a file can cause this.",
    );
    expect(STATE_MEANING.unsupported).toContain(DO_NOT_RELY);
    for (const text of [...Object.values(STATE_MEANING), ...Object.values(REASON_TEXT), ...Object.values(STALE_TEXT)]) {
      // Nothing calms the reader about a signature that was not checked, and nothing promises.
      expect(text).not.toMatch(/says nothing for or against/i);
      expect(text).not.toMatch(/legal|court|guarantee/i);
    }
    for (const text of Object.values(STALE_TEXT)) {
      expect(text).toContain("Do not rely");
      expect(text).toContain("close this tab and open the file again");
    }
  });
});

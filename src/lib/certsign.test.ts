import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  CERT_WORDS,
  DEFAULT_BOX,
  MAX_TEXT_CHARS,
  MIN_BOX_SIDE,
  allCertWords,
  boxAround,
  boxBetween,
  certificateWarnings,
  cleanText,
  dayOf,
  describeSignError,
  periodText,
  type CertificateInfo,
} from "./certsign";

function certificate(over: Partial<CertificateInfo> = {}): CertificateInfo {
  return {
    name: "Anna Alvorlig",
    issuer: "Test Root",
    selfSigned: false,
    serial: "01",
    fingerprint: "ab".repeat(32),
    notBefore: 1_577_836_800, // 2020-01-01
    notAfter: 4_733_510_400, // 2120-01-01
    now: 1_781_524_800,
    key: "RSA 3072 bit",
    maySign: true,
    chain: 2,
    ...over,
  };
}

describe("what the box says about a certificate", () => {
  it("says nothing when the certificate is in order", () => {
    expect(certificateWarnings(certificate())).toEqual([]);
  });

  it("explains a certificate that its owner made, one that has expired and one that is not in use yet", () => {
    expect(certificateWarnings(certificate({ selfSigned: true }))).toEqual([CERT_WORDS.selfMade]);
    expect(certificateWarnings(certificate({ notAfter: 1_600_000_000 }))).toEqual([CERT_WORDS.expired]);
    expect(certificateWarnings(certificate({ notBefore: 1_900_000_000 }))).toEqual([CERT_WORDS.notYet]);
    expect(certificateWarnings(certificate({ maySign: false }))).toEqual([CERT_WORDS.notForSigning]);
    expect(
      certificateWarnings(certificate({ selfSigned: true, notAfter: 1_600_000_000, maySign: false })),
    ).toEqual([CERT_WORDS.selfMade, CERT_WORDS.expired, CERT_WORDS.notForSigning]);
  });

  it("judges the period by the clock of this computer, which the file reports", () => {
    expect(certificateWarnings(certificate({ now: 4_800_000_000 }))).toEqual([CERT_WORDS.expired]);
    expect(certificateWarnings(certificate({ now: 1_500_000_000 }))).toEqual([CERT_WORDS.notYet]);
  });

  it("shows the period as two days", () => {
    expect(dayOf(1_781_524_800)).toBe("2026-06-15");
    expect(dayOf(Number.NaN)).toBe("");
    expect(periodText(certificate())).toBe("2020-01-01 to 2120-01-01");
  });
});

describe("the texts of the signature", () => {
  it("is one plain line of at most the cap", () => {
    expect(cleanText("  Anna \n\t Alvorlig  ")).toBe("Anna Alvorlig");
    const hidden = String.fromCharCode(0x202e, 0x62, 0x200b, 0x63, 7, 0x64);
    expect(cleanText("a" + hidden)).toBe("a b c d");
    expect(cleanText("x".repeat(500))).toHaveLength(MAX_TEXT_CHARS);
    expect(cleanText("   ")).toBe("");
  });
});

describe("the box on the page", () => {
  const page = { width: 612, height: 792 };

  it("goes around a click and stays on the page", () => {
    expect(boxAround(300, 400, page)).toEqual([300 - DEFAULT_BOX.width / 2, 400 - DEFAULT_BOX.height / 2, 300 + DEFAULT_BOX.width / 2, 400 + DEFAULT_BOX.height / 2]);
    expect(boxAround(0, 0, page)).toEqual([0, 0, DEFAULT_BOX.width, DEFAULT_BOX.height]);
    expect(boxAround(612, 792, page)).toEqual([612 - DEFAULT_BOX.width, 792 - DEFAULT_BOX.height, 612, 792]);
    expect(boxAround(10, 10, { width: 20, height: 20 })).toBeNull();
  });

  it("is the one between two corners, in any direction, and not too small", () => {
    expect(boxBetween([100, 100], [300, 200], page)).toEqual([100, 100, 300, 200]);
    expect(boxBetween([300, 200], [100, 100], page)).toEqual([100, 100, 300, 200]);
    expect(boxBetween([-50, -50], [700, 900], page)).toEqual([0, 0, 612, 792]);
    expect(boxBetween([100, 100], [100 + MIN_BOX_SIDE - 1, 200], page)).toBeNull();
    expect(boxBetween([100, 100], [200, 100 + MIN_BOX_SIDE - 1], page)).toBeNull();
    expect(boxBetween([100, 100], [100 + MIN_BOX_SIDE, 100 + MIN_BOX_SIDE], page)).not.toBeNull();
  });
});

describe("the answers to what can go wrong", () => {
  const codes = [
    "signer_wrong_password",
    "signer_not_a_certificate_file",
    "signer_no_key",
    "signer_key_unsupported",
    "signer_key_mismatch",
    "signer_too_large",
    "sign_file_unreadable",
    "sign_no_file",
    "signer_not_loaded",
    "signer_stopped",
    "signer_protection_unsupported",
    "signer_chain_too_long",
    "signer_password_too_long",
    "signer_sandbox_unavailable",
    "sign_unsaved",
    "sign_same_file",
    "sign_target_open",
    "sign_target_signed",
    "sign_encrypted",
    "save_metadata_not_encrypted",
    "sign_xfa",
    "sign_already_signed",
    "sign_structure",
    "sign_refused",
    "page_out_of_range",
    "save_too_large",
    "anything_else",
  ];

  it("says why a key is refused, in the words of the signer, as text", () => {
    const told = describeSignError({
      code: "signer_key_unsupported",
      message: "this kind of key is not supported (RSA keys shorter than 2048 bits are not supported)",
    });
    expect(told.detail).toContain("This file: RSA keys shorter than 2048 bits are not supported.");
    // Nothing is added when the signer gave no reason.
    expect(describeSignError({ code: "signer_key_unsupported", message: "" }).detail).not.toContain("This file");
  });

  it("has a title and a detail for every code", () => {
    for (const code of codes) {
      const told = describeSignError({ code, message: "m" }, '"a.pdf"');
      expect(told.title.length, code).toBeGreaterThan(3);
      expect(told.detail.length, code).toBeGreaterThan(10);
    }
  });

  it("names the key kinds that are supported and tells how to go on after a wrong password", () => {
    expect(describeSignError({ code: "signer_key_unsupported", message: "" }).detail).toContain("RSA");
    expect(describeSignError({ code: "signer_key_unsupported", message: "" }).detail).toContain("P-384");
    expect(describeSignError({ code: "signer_wrong_password", message: "" }).detail).toContain("try again");
  });

  it("tells a file with signatures that signing it again would remove them", () => {
    expect(describeSignError({ code: "sign_already_signed", message: "" }).detail).toBe(
      "Signing it again would remove them, so Patient Partridge does not do it in this version.",
    );
  });
});

/** The words that make a promise, or name what must not be named. */
const FORBIDDEN = new RegExp(
  "\\b(legal|legally|court|guarantee|guaranteed|valid|validity|invalid|binding|qualified|advanced|eidas|compliant|certified|proof|prove|proves|authentic|authenticity|notarised|notarized|" +
    "ad" +
    "obe|acro" +
    "bat|ac" +
    "robat|fox" +
    "it|pdf" +
    "sig|pyhan" +
    "ko)\\b",
  "i",
);

describe("the words make no promise", () => {
  it("none of the fixed words, and none of the answers to errors, says what a signature is worth", () => {
    const texts = [
      ...allCertWords(),
      ...[
        "signer_wrong_password",
        "signer_not_a_certificate_file",
        "signer_no_key",
        "signer_key_unsupported",
        "signer_key_mismatch",
        "sign_unsaved",
        "sign_same_file",
        "sign_encrypted",
        "sign_xfa",
        "sign_already_signed",
        "sign_structure",
        "sign_refused",
        "nothing",
      ].flatMap((code) => {
        const t = describeSignError({ code, message: "" });
        return [t.title, t.detail];
      }),
    ];
    expect(texts.length).toBeGreaterThan(40);
    for (const text of texts) expect(text, text).not.toMatch(FORBIDDEN);
  });

  it("says in one line what a self-made certificate and the computer's clock are", () => {
    expect(CERT_WORDS.selfMade).toBe(
      "This certificate was made by its owner. Programs that open the file will usually show the signer as unknown.",
    );
    expect(CERT_WORDS.clock).toBe("The signing time is taken from this computer's clock. Nobody else has confirmed it.");
    expect(CERT_WORDS.intro).toContain("does not say whether anyone will accept the signature");
    expect(CERT_WORDS.passwordHint).toContain("not saved");
  });

  it("keeps the sources free of those words too", () => {
    for (const file of ["src/lib/certsign.ts", "src/app/certsign.ts"]) {
      const source = readFileSync(file, "utf8");
      const prose = source
        .split("\n")
        .filter((l) => /["'`]/.test(l) && !/^\s*(import|\/\/|\*)/.test(l))
        .join("\n");
      expect(prose, file).not.toMatch(/\b(legal|legally|court|guarantee|guaranteed|binding|eidas|compliant|certified)\b/i);
    }
  });

  it("names no date", () => {
    for (const text of allCertWords()) expect(text, text).not.toMatch(/\b(19|20)\d\d\b/);
  });
});

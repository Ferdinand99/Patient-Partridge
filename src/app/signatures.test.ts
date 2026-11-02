// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SignatureItem, SignatureState, SignaturesInfo } from "../lib/signatures";
import { SignaturePanel, describeRange, docMdpText } from "./signatures";

beforeEach(() => {
  const html = readFileSync("index.html", "utf8");
  const body = /<body>([\s\S]*)<\/body>/.exec(html)?.[1] ?? "";
  document.body.innerHTML = body.replace(/<script[\s\S]*?<\/script>/g, "");
});

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
});

const q = <T extends HTMLElement>(selector: string) => document.querySelector(selector) as T;
const flush = () => new Promise((r) => setTimeout(r, 0));

function item(state: SignatureState, over: Partial<SignatureItem> = {}): SignatureItem {
  return {
    index: 0,
    state,
    reason: "no_trust_list",
    certificateTime: "valid",
    subFilter: "adbe.pkcs7.detached",
    byteRange: [0, 100, 300, 50],
    fileLen: 350,
    docMdp: 0,
    certificates: 1,
    signer: "Test Signer",
    organisation: "Test Organisation",
    issuer: "Test CA",
    serial: "12345678",
    fingerprint: "ab".repeat(32),
    validFrom: "2025-01-01 00:00 UTC",
    validTo: "2035-01-01 00:00 UTC",
    key: "ECDSA P-256",
    digest: "SHA-256",
    claimedTime: "2026-06-15 12:00:00 +02:00",
    statedReason: "I approve this document",
    statedLocation: "Oslo",
    trustAnchor: null,
    ...over,
  };
}

function report(items: SignatureItem[], over: Partial<SignaturesInfo> = {}): SignaturesInfo {
  return { total: items.length, unlisted: 0, trustAnchors: 0, items, ...over };
}

function make(answer: SignaturesInfo | Error | ((doc: number) => SignaturesInfo), delayMs = 0) {
  const getSignatures = vi.fn((doc: number) =>
    answer instanceof Error
      ? Promise.reject(answer)
      : Promise.resolve(typeof answer === "function" ? answer(doc) : answer),
  );
  const panel = new SignaturePanel(document, { api: { getSignatures }, delayMs });
  return { panel, getSignatures };
}

const badge = () => q<HTMLButtonElement>("#signature-badge");
const panelEl = () => q<HTMLElement>("#signature-panel");

describe("the signature badge", () => {
  it("is not there for a document without signatures", async () => {
    const { panel, getSignatures } = make(report([]));
    panel.setDocument(1);
    await flush();
    expect(getSignatures).toHaveBeenCalledOnce();
    expect(badge().hidden).toBe(true);
    expect(panel.summary).toBeNull();
  });

  it("sits in the toolbar before the print button", () => {
    make(report([]));
    expect(badge().nextElementSibling?.id).toBe("print-button");
  });

  it("says that an intact signature has a signer that is not verified, and does not tick it", async () => {
    const { panel } = make(report([item("unknown_signer")]));
    panel.setDocument(1);
    await flush();
    expect(badge().hidden).toBe(false);
    expect(badge().textContent).toBe("Signature intact; signer not verified");
    expect(badge().classList.contains("sig-unknown_signer")).toBe(true);
    expect(badge().classList.contains("sig-trusted")).toBe(false);
    // The mark of a trusted signature is a tick; nothing else has one.
    const tick = "M8.25 12.25l2.6 2.6";
    expect(badge().innerHTML).not.toContain(tick);
  });

  it("ticks a trusted signature, and only that", async () => {
    const { panel } = make(report([item("trusted", { reason: "verified", trustAnchor: "Test Root" })], { trustAnchors: 1 }));
    panel.setDocument(1);
    await flush();
    expect(badge().textContent).toBe("Signature intact; signer trusted");
    expect(badge().classList.contains("sig-trusted")).toBe(true);
    expect(badge().innerHTML).toContain("M8.25 12.25l2.6 2.6");
  });

  it.each([
    ["changed", "Changed after signing"],
    ["broken", "Signature broken"],
    ["unsupported", "Signature not checked"],
  ] as const)("shows %s in words", async (state, label) => {
    const { panel } = make(report([item(state, { reason: "data_after_signature" })]));
    panel.setDocument(1);
    await flush();
    expect(badge().textContent).toBe(label);
    expect(badge().classList.contains(`sig-${state}`)).toBe(true);
  });

  it("shows the worst of several signatures", async () => {
    const { panel } = make(report([item("trusted"), item("changed", { index: 1 }), item("unknown_signer", { index: 2 })]));
    panel.setDocument(1);
    await flush();
    expect(badge().textContent).toBe("Changed after signing");
  });

  it("shows a document whose signatures are hidden", async () => {
    const { panel } = make(report([], { unlisted: 1 }));
    panel.setDocument(1);
    await flush();
    expect(badge().textContent).toBe("Possible signature marks");
    badge().click();
    expect(panelEl().textContent).toContain("looks like a signature");
    expect(panelEl().textContent).toContain("not listed as one");
  });

  it("says the signatures could not be checked when the check fails, and can try again", async () => {
    let fail = true;
    const getSignatures = vi.fn(() => (fail ? Promise.reject(new Error("gone")) : Promise.resolve(report([item("unknown_signer")]))));
    const panel = new SignaturePanel(document, { api: { getSignatures }, delayMs: 0 });
    panel.setDocument(1);
    await flush();
    expect(badge().textContent).toBe("Signatures not checked");
    badge().click();
    expect(panelEl().textContent).toContain("could not be checked just now");
    fail = false;
    panelEl().querySelector<HTMLButtonElement>("button.btn:not(.btn-icon)")?.click();
    await flush();
    expect(getSignatures).toHaveBeenCalledTimes(2);
    expect(badge().textContent).toBe("Signature intact; signer not verified");
  });

  it("makes no request without the call, and is quiet", async () => {
    const panel = new SignaturePanel(document, { api: {}, delayMs: 0 });
    panel.setDocument(1);
    await flush();
    expect(badge().hidden).toBe(true);
  });
});

describe("when the signatures are asked for", () => {
  it("waits for the document to rest, asks once, and follows the document in front", async () => {
    vi.useFakeTimers();
    const { panel, getSignatures } = make((doc) => report(doc === 1 ? [item("changed")] : [item("unknown_signer")]), 1200);
    panel.setDocument(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(getSignatures).not.toHaveBeenCalled();
    expect(badge().hidden).toBe(true);
    await vi.advanceTimersByTimeAsync(300);
    expect(getSignatures).toHaveBeenCalledWith(1);
    expect(badge().textContent).toBe("Changed after signing");

    panel.setDocument(2);
    // The other document has no badge until its answer is in.
    expect(badge().hidden).toBe(true);
    await vi.advanceTimersByTimeAsync(1300);
    expect(badge().textContent).toBe("Signature intact; signer not verified");

    // Back to the first: what is known is shown at once, and nothing is asked again.
    panel.setDocument(1);
    expect(badge().textContent).toBe("Changed after signing");
    await vi.advanceTimersByTimeAsync(5000);
    expect(getSignatures).toHaveBeenCalledTimes(2);
  });

  it("does not ask for a document that was closed while it waited", async () => {
    vi.useFakeTimers();
    const { panel, getSignatures } = make(report([item("unknown_signer")]), 1200);
    panel.setDocument(1);
    panel.forget(1);
    await vi.advanceTimersByTimeAsync(3000);
    expect(getSignatures).not.toHaveBeenCalled();
  });

  it("drops an answer that arrives after the document was closed", async () => {
    const releases = new Map<number, (info: SignaturesInfo) => void>();
    const getSignatures = vi.fn((doc: number) => new Promise<SignaturesInfo>((resolve) => releases.set(doc, resolve)));
    const panel = new SignaturePanel(document, { api: { getSignatures }, delayMs: 0 });
    panel.setDocument(1);
    panel.forget(1);
    panel.setDocument(2);
    // The answer for the closed document comes in; the one in front has none yet.
    releases.get(1)?.(report([item("broken")]));
    await flush();
    expect(badge().hidden).toBe(true);
    releases.get(2)?.(report([item("changed")]));
    await flush();
    expect(badge().textContent).toBe("Changed after signing");
  });
});

describe("the signature panel", () => {
  async function opened(over: Partial<SignatureItem> = {}, info: Partial<SignaturesInfo> = {}) {
    const made = make(report([item("unknown_signer", over)], info));
    made.panel.setDocument(1);
    await flush();
    badge().click();
    return made;
  }

  it("opens from the badge, closes with Escape and gives the focus back", async () => {
    const { panel } = await opened();
    expect(panelEl().hidden).toBe(false);
    expect(badge().getAttribute("aria-expanded")).toBe("true");
    expect(panelEl().getAttribute("role")).toBe("dialog");
    expect(document.activeElement).toBe(panelEl().querySelector("button"));
    panelEl().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(panelEl().hidden).toBe(true);
    expect(badge().getAttribute("aria-expanded")).toBe("false");
    expect(document.activeElement).toBe(badge());
    expect(panel.isOpen).toBe(false);
    // The badge opens it again, and the close button closes it.
    badge().click();
    expect(panelEl().hidden).toBe(false);
    panelEl().querySelector<HTMLButtonElement>(".sig-head button")?.click();
    expect(panelEl().hidden).toBe(true);
  });

  it("closes when there is no document, and when the document is closed", async () => {
    const { panel } = await opened();
    panel.setDocument(null);
    expect(panelEl().hidden).toBe(true);
    expect(badge().hidden).toBe(true);
    badge().click();
    expect(panelEl().hidden).toBe(true);
    panel.setDocument(1);
    badge().click();
    expect(panelEl().hidden).toBe(false);
    panel.forget(1);
    expect(panelEl().hidden).toBe(true);
  });

  it("says that a picture of a signature is only a picture", async () => {
    await opened();
    expect(panelEl().textContent).toContain("A picture of a signature on a page is only a picture");
  });

  it("shows the certificate as text and the claims of the file apart, as not checked", async () => {
    await opened();
    const text = panelEl().textContent ?? "";
    expect(text).toContain("Signature intact; signer not verified");
    expect(text).toContain("ships no trust list");
    expect(text).toContain("Certificate of the signer");
    expect(text).toContain("Test Signer");
    expect(text).toContain("Test Organisation");
    expect(text).toContain("Issued by");
    expect(text).toContain("2025-01-01 00:00 UTC");
    expect(text).toContain("SHA-256 fingerprint");
    expect(text).toContain("What the file says (not checked)");
    expect(text).toContain("I approve this document");
    expect(text).toContain("Oslo");
    expect(text).toContain("0–100 and 300–350 of 350");
    // The claims are after the certificate and in their own list.
    const lists = [...panelEl().querySelectorAll("dl")];
    expect(lists[0].textContent).toContain("Test Signer");
    expect(lists[0].textContent).not.toContain("I approve");
    expect(lists[1].textContent).toContain("I approve this document");
    expect(lists[1].textContent).not.toContain("Test Signer");
  });

  it("says what is not checked", async () => {
    await opened();
    const text = panelEl().textContent ?? "";
    expect(text).toContain("What this does not check");
    expect(text).toMatch(/revoked/);
    expect(text).toMatch(/Time stamps/);
    expect(text).toMatch(/Long-term validation/);
    expect(text).toContain("This version ships no list of trusted signers.");
  });

  it("explains a change after signing in words that do not blame an edit", async () => {
    const made = make(report([item("changed", { reason: "data_after_signature" })]));
    made.panel.setDocument(1);
    await flush();
    badge().click();
    const text = panelEl().textContent ?? "";
    expect(text).toContain("Changed after signing");
    expect(text).toContain("A later signature, a filled-in form or any other edit");
  });

  it("warns that a certificate has expired", async () => {
    await opened({ certificateTime: "expired" });
    expect(panelEl().textContent).toContain("has expired by this computer's clock");
  });

  it("names the signer when the trust list has the certificate", async () => {
    const made = make(
      report([item("trusted", { reason: "verified", trustAnchor: "Test Root" })], { trustAnchors: 1 }),
    );
    made.panel.setDocument(1);
    await flush();
    badge().click();
    const text = panelEl().textContent ?? "";
    expect(text).toContain("On the trust list as");
    expect(text).toContain("Test Root");
    expect(text).toContain("The trust list of this version holds 1 certificate.");
    expect(text).not.toContain("ships no trust list");
  });

  it("mentions signatures that are listed but not described", async () => {
    await opened({}, { total: 70 });
    expect(panelEl().textContent).toContain("69 more signatures are listed in the file but not described here.");
  });

  it("shows text from the file as text, never as markup, and the claims cannot change the state", async () => {
    const hostile = '<img src=x onerror="alert(1)"> Trusted Bank AS';
    const made = make(
      report([
        item("broken", {
          reason: "signature_invalid",
          signer: hostile,
          organisation: "<script>alert(2)</script>",
          statedReason: "<b>VERIFIED</b> by Some Reader - signature valid",
          statedLocation: "javascript:alert(3)",
          claimedTime: "<i>now</i>",
          subFilter: "<u>sub</u>",
        }),
      ]),
    );
    made.panel.setDocument(1);
    await flush();
    // The badge and the state in the panel are the words of the window.
    expect(badge().textContent).toBe("Signature broken");
    badge().click();
    expect(panelEl().querySelector("img, script, b, i, u, a")).toBeNull();
    const text = panelEl().textContent ?? "";
    expect(text).toContain(hostile);
    expect(text).toContain("<b>VERIFIED</b> by Some Reader - signature valid");
    expect(text).toContain("Signature broken");
    expect(panelEl().querySelector(".sig-state strong")?.textContent).toBe("Signature broken");
    // Every element in the panel is one that this file made.
    const tags = new Set([...panelEl().querySelectorAll("*")].map((e) => e.tagName.toLowerCase()));
    for (const tag of tags) {
      expect(["aside", "div", "h2", "h3", "button", "svg", "use", "path", "circle", "p", "ol", "li", "strong", "dl", "dt", "dd", "details", "summary", "section", "ul"]).toContain(tag);
    }
  });

  it("does not let a state it does not know look intact", async () => {
    const made = make(report([item("splendid" as SignatureState, { reason: "made_up" })]));
    made.panel.setDocument(1);
    await flush();
    expect(badge().textContent).toBe("Signature not checked");
    badge().click();
    expect(panelEl().textContent).toContain("No reason is given for this state.");
    expect(panelEl().textContent).not.toContain("Signature intact");
  });

  it("describes a byte range that is not the usual four numbers", () => {
    expect(describeRange([0, 10, 20, 30], 50)).toBe("0–10 and 20–50 of 50");
    expect(describeRange([1, 2, 3], 9)).toBe("1 2 3");
    expect(describeRange([], 9)).toBe("");
  });
});

describe("a verdict is only for the bytes it was made from", () => {
  const STALE = "Not checked for this version";

  it("is dropped by invalidate, and what was intact is no longer shown as intact", async () => {
    const { panel, getSignatures } = make(report([item("unknown_signer")]));
    panel.setDocument(1);
    await flush();
    expect(badge().textContent).toBe("Signature intact; signer not verified");
    panel.invalidate(1);
    expect(badge().hidden).toBe(false);
    expect(badge().textContent).toBe(STALE);
    expect(badge().classList.contains("sig-stale")).toBe(true);
    expect(badge().classList.contains("sig-unknown_signer")).toBe(false);
    expect(badge().title).toContain(STALE);
    expect(panel.summary).toBeNull();
    // Showing the tab again asks nothing.
    panel.setDocument(null);
    panel.setDocument(1);
    await flush();
    expect(getSignatures).toHaveBeenCalledOnce();
    expect(badge().textContent).toBe(STALE);
  });

  it("makes an open panel say so, without a way to check again", async () => {
    const { panel } = make(report([item("unknown_signer")]));
    panel.setDocument(1);
    await flush();
    badge().click();
    expect(panelEl().textContent).toContain("Signature intact");
    panel.invalidate(1);
    const text = panelEl().textContent ?? "";
    expect(text).not.toContain("Signature intact");
    expect(text).not.toContain("Test Signer");
    expect(text).toContain("has not been checked");
    // The way to a check is one that exists: the tab is closed and the file opened again.
    expect(text).toContain("close this tab and open the file again");
    expect(text).not.toContain("Open the file again to check it");
    expect(text).toContain("Do not rely");
    expect(panelEl().querySelector("button.btn:not(.btn-icon)")).toBeNull();
    expect(panelEl().hidden).toBe(false);
  });

  it("says in other words that the file on the disk changed, when the engine found it", async () => {
    const { panel } = make(report([item("unknown_signer")]));
    panel.setDocument(1);
    await flush();
    badge().click();
    panel.invalidate(1, "file_changed");
    expect(badge().textContent).toBe(STALE);
    const text = panelEl().textContent ?? "";
    expect(text).not.toContain("Signature intact");
    expect(text).toContain("The file on the disk has been changed");
    expect(text).toContain("Do not rely");
    // The first reason stays: a later one does not rewrite it.
    panel.invalidate(1);
    expect(panelEl().textContent).toContain("The file on the disk has been changed");
  });

  it("covers a check that was still on its way: the answer only says whether to speak", async () => {
    const releases: ((info: SignaturesInfo) => void)[] = [];
    const getSignatures = vi.fn(() => new Promise<SignaturesInfo>((resolve) => releases.push(resolve)));
    const panel = new SignaturePanel(document, { api: { getSignatures }, delayMs: 0 });
    panel.setDocument(1);
    panel.invalidate(1);
    expect(badge().hidden).toBe(true);
    releases[0](report([item("unknown_signer")]));
    await flush();
    // It had signatures, so the tab says that this version was not checked; it never says intact.
    expect(badge().textContent).toBe(STALE);
    expect(panel.summary).toBeNull();

    // A document without signatures stays without a badge.
    panel.setDocument(2);
    panel.invalidate(2);
    releases[1](report([]));
    await flush();
    expect(badge().hidden).toBe(true);
  });

  it("asks once for a tab that was invalidated before the wait was over, so a signed document keeps a badge", async () => {
    vi.useFakeTimers();
    const { panel, getSignatures } = make(report([item("unknown_signer")]), 1200);
    panel.setDocument(1);
    panel.invalidate(1);
    await vi.advanceTimersByTimeAsync(3000);
    // Asked at once (the timer is not left to run), and once only.
    expect(getSignatures).toHaveBeenCalledOnce();
    expect(badge().textContent).toBe(STALE);
    expect(panel.summary).toBeNull();
  });

  it("keeps no badge for an invalidated tab of a document that is not signed", async () => {
    vi.useFakeTimers();
    const { panel, getSignatures } = make(report([]), 1200);
    panel.setDocument(1);
    panel.invalidate(1);
    await vi.advanceTimersByTimeAsync(3000);
    expect(getSignatures).toHaveBeenCalledOnce();
    expect(badge().hidden).toBe(true);
  });

  it("says that this version was not checked when the answer that was on its way fails", async () => {
    const reject: ((e: Error) => void)[] = [];
    const getSignatures = vi.fn(() => new Promise<SignaturesInfo>((_, no) => reject.push(no)));
    const panel = new SignaturePanel(document, { api: { getSignatures }, delayMs: 0 });
    panel.setDocument(1);
    panel.invalidate(1);
    expect(badge().hidden).toBe(true);
    reject[0](new Error("the document was saved"));
    await flush();
    // Whether it is signed is not known, and a changed document may well be a signed one.
    expect(badge().hidden).toBe(false);
    expect(badge().textContent).toBe(STALE);
  });

  it("covers a check that failed, and does not offer to try again", async () => {
    const { panel } = make(new Error("gone"));
    panel.setDocument(1);
    await flush();
    expect(badge().textContent).toBe("Signatures not checked");
    panel.invalidate(1);
    expect(badge().textContent).toBe(STALE);
    badge().click();
    expect(panelEl().querySelector("button.btn:not(.btn-icon)")).toBeNull();
  });

  it("leaves other documents alone, and stays dropped when asked twice", async () => {
    const { panel } = make(report([item("changed")]));
    panel.setDocument(1);
    await flush();
    panel.setDocument(2);
    await flush();
    panel.invalidate(2);
    panel.invalidate(2);
    expect(badge().textContent).toBe(STALE);
    panel.setDocument(1);
    expect(badge().textContent).toBe("Changed after signing");
  });

  it("is told for a document that was never asked about", () => {
    const { panel, getSignatures } = make(report([item("unknown_signer")]));
    panel.invalidate(5);
    panel.setDocument(5);
    expect(getSignatures).not.toHaveBeenCalled();
    expect(badge().hidden).toBe(true);
  });
});

describe("what the panel says about the name of a signer", () => {
  function freshPage(): void {
    const html = readFileSync("index.html", "utf8");
    const body = /<body>([\s\S]*)<\/body>/.exec(html)?.[1] ?? "";
    document.body.innerHTML = body.replace(/<script[\s\S]*?<\/script>/g, "");
  }

  it("says that the name is not proof for every state except a trusted one", async () => {
    for (const state of ["unknown_signer", "changed", "broken", "unsupported"] as const) {
      freshPage();
      const made = make(report([item(state)]));
      made.panel.setDocument(1);
      await flush();
      badge().click();
      expect(panelEl().textContent, state).toContain("not proof of who holds the key");
    }
    freshPage();
    const made = make(report([item("trusted", { reason: "verified", trustAnchor: "Test Root" })], { trustAnchors: 1 }));
    made.panel.setDocument(1);
    await flush();
    badge().click();
    expect(panelEl().textContent).not.toContain("not proof of who holds the key");
  });

  it("says in words what a level of change allows", () => {
    expect(docMdpText(1)).toContain("none");
    expect(docMdpText(2)).toContain("forms");
    expect(docMdpText(3)).toContain("annotations");
  });
});

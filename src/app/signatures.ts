// The signature panel: a calm badge in the toolbar for a document that is signed, and a panel with
// the details.
//
// The engine checks the signatures. This file only shows what it found. Two rules hold everywhere
// in it:
//  - The state of a signature and the reason for it are worded here (lib/signatures.ts), from a
//    code. Nothing a PDF contains can change those words.
//  - Text from the file (the name in the certificate, the reason and the place the signer wrote)
//    is put on the page with textContent, in its own field, labelled as what it is. No HTML is
//    ever made from it.
// A picture of a signature on a page of the PDF is only a picture; this panel is the only place
// that reports on a signature, and it says so.
//
// A result is about the bytes it was computed from, and only about those. The engine checks the
// file the document was opened from. As soon as the tab holds anything else (a save, a copy saved
// under another name, an edit in memory), `invalidate` drops the result and the tab shows
// "Not checked for this version" until the tab is closed and the file opened again. A result is
// never kept, shown or asked for again for such a tab. The same goes when the engine finds that the
// file it reads from has other bytes than the ones that were checked: the window is told, and the
// result is dropped at once.

import {
  NOT_CHECKED,
  STALE_LABEL,
  STALE_TEXT,
  STATE_LABEL,
  STATE_MEANING,
  type StaleReason,
  certificateTimeText,
  knownState,
  omittedText,
  reasonText,
  summarise,
  unlistedText,
  type SignatureItem,
  type SignatureState,
  type SignaturesInfo,
  type Summary,
} from "../lib/signatures";

/** How long after a document is shown its signatures are asked for. The check reads the whole file in the document's engine, so it waits until the first page is up. */
export const DEFAULT_SIGNATURES_DELAY_MS = 1200;

export interface SignatureApi {
  getSignatures(doc: number): Promise<SignaturesInfo>;
}

type Entry =
  | { kind: "loading" }
  | { kind: "error" }
  | { kind: "ready"; info: SignaturesInfo; summary: Summary | null }
  /**
   * The tab no longer holds the bytes that were (or were being) checked. `shown` says whether the
   * badge is there to say so: it is for a document that had signatures, and `null` while the answer
   * that tells is still on its way (it is used for that and for nothing else).
   */
  | { kind: "stale"; shown: boolean | null; why: StaleReason };

const SVG = "http://www.w3.org/2000/svg";

/** One small mark for each state. Only a trusted signature has a tick. */
const ICONS: Record<SignatureState | "marks" | "error" | "stale", string> = {
  trusted: '<circle cx="12" cy="12" r="8.25"/><path d="M8.25 12.25l2.6 2.6 4.9-5.5"/>',
  unknown_signer: '<circle cx="12" cy="12" r="8.25"/><path d="M9.75 9.75a2.35 2.35 0 1 1 3.4 2.1c-.75.4-1.15.8-1.15 1.65M12 16.5v.01"/>',
  changed: '<path d="M12 4.25l8.25 14.5H3.75z"/><path d="M12 10v4M12 16.75v.01"/>',
  broken: '<circle cx="12" cy="12" r="8.25"/><path d="M9 9l6 6M15 9l-6 6"/>',
  unsupported: '<circle cx="12" cy="12" r="8.25"/><path d="M8.5 12h7"/>',
  marks: '<circle cx="12" cy="12" r="8.25"/><path d="M12 7.75v5M12 16.25v.01"/>',
  error: '<circle cx="12" cy="12" r="8.25"/><path d="M8.5 12h7"/>',
  stale: '<circle cx="12" cy="12" r="8.25"/><path d="M8.5 12h7"/>',
};

function el<K extends keyof HTMLElementTagNameMap>(
  doc: Document,
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = doc.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function icon(doc: Document, key: keyof typeof ICONS): SVGSVGElement {
  const svg = doc.createElementNS(SVG, "svg");
  svg.setAttribute("class", "icon");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  // The paths are written in this file, never taken from a document.
  svg.innerHTML = ICONS[key];
  return svg;
}

/** Adds `label` and `value` as one row of a description list, when there is a value. */
function fact(list: HTMLElement, label: string, value: string | null | undefined, mono = false): void {
  if (value === null || value === undefined || value === "") return;
  const doc = list.ownerDocument;
  list.append(el(doc, "dt", undefined, label), el(doc, "dd", mono ? "sig-mono" : undefined, value));
}

/** What the level of change a signer allows (DocMDP) permits, in words. */
export function docMdpText(level: number): string {
  switch (level) {
    case 1:
      return "none (level 1)";
    case 2:
      return "filling in forms and signing (level 2)";
    case 3:
      return "filling in forms, signing and annotations (level 3)";
    default:
      return `level ${level}`;
  }
}

function formatBytes(n: number): string {
  return `${n.toLocaleString("en-US")} bytes`;
}

/** The byte range as the file gives it, in a form a person can read. */
export function describeRange(range: number[], fileLen: number): string {
  if (range.length === 4) {
    const [a, b, c, d] = range;
    return `${a}–${a + b} and ${c}–${c + d} of ${fileLen.toLocaleString("en-US")}`;
  }
  return range.join(" ");
}

export class SignaturePanel {
  private readonly entries = new Map<number, Entry>();
  private current: number | null = null;
  private readonly badge: HTMLButtonElement;
  private readonly badgeLabel: HTMLElement;
  private readonly panel: HTMLElement;
  private readonly body: HTMLElement;
  private readonly closeButton: HTMLButtonElement;
  private open = false;
  private readonly timers = new Map<number, ReturnType<typeof setTimeout>>();

  constructor(
    private readonly root: Document,
    private readonly deps: { api: Partial<SignatureApi>; delayMs?: number },
  ) {
    const doc = root;
    this.badge = el(doc, "button", "pill sig-badge");
    this.badge.type = "button";
    this.badge.id = "signature-badge";
    this.badge.hidden = true;
    this.badge.setAttribute("aria-haspopup", "dialog");
    this.badge.setAttribute("aria-expanded", "false");
    this.badge.setAttribute("aria-controls", "signature-panel");
    this.badgeLabel = el(doc, "span");
    this.badge.append(this.badgeLabel);
    const print = doc.getElementById("print-button");
    if (print?.parentElement) print.parentElement.insertBefore(this.badge, print);
    else doc.getElementById("toolbar")?.append(this.badge);

    this.panel = el(doc, "aside", "sig-panel");
    this.panel.id = "signature-panel";
    this.panel.setAttribute("role", "dialog");
    this.panel.setAttribute("aria-modal", "false");
    this.panel.setAttribute("aria-labelledby", "signature-panel-title");
    this.panel.hidden = true;
    const head = el(doc, "div", "sig-head");
    const title = el(doc, "h2", "sig-title", "Digital signatures");
    title.id = "signature-panel-title";
    this.closeButton = el(doc, "button", "btn btn-icon btn-plain");
    this.closeButton.type = "button";
    this.closeButton.title = "Close (Esc)";
    this.closeButton.setAttribute("aria-label", "Close the signature panel");
    const x = doc.createElementNS(SVG, "svg");
    x.setAttribute("class", "icon");
    x.setAttribute("aria-hidden", "true");
    const use = doc.createElementNS(SVG, "use");
    use.setAttribute("href", "#i-x");
    x.append(use);
    this.closeButton.append(x);
    head.append(title, this.closeButton);
    this.body = el(doc, "div", "sig-body");
    this.panel.append(head, this.body);
    (doc.querySelector(".workspace") ?? doc.body).append(this.panel);

    this.badge.addEventListener("click", () => this.toggle());
    this.closeButton.addEventListener("click", () => this.close(true));
    this.panel.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.close(true);
      }
    });
  }

  /** The document in front changed (or there is none). */
  setDocument(doc: number | null): void {
    this.current = doc;
    if (doc === null) {
      this.close(false);
      this.render();
      return;
    }
    if (!this.entries.has(doc) && this.deps.api.getSignatures) this.schedule(doc);
    this.render();
  }

  /**
   * The tab of this document no longer holds the file the signatures are checked on: it was saved
   * (in place or as a copy) or changed in memory. What was found is dropped and never shown again
   * for this tab; the badge of a document that had signatures says that this version was not
   * checked. Asking the engine again would not help: it goes on reading the file it opened.
   */
  invalidate(doc: number, why: StaleReason = "changed"): void {
    // A wait that is not over means the engine was not asked yet.
    const notAsked = this.timers.has(doc);
    clearTimeout(this.timers.get(doc));
    this.timers.delete(doc);
    const entry = this.entries.get(doc);
    let shown: boolean | null;
    switch (entry?.kind) {
      case "stale":
        return;
      case "loading":
        // The answer on its way (or, if the wait before asking is not over, the one that will be
        // asked for) is about the old file. It only tells whether to say anything.
        shown = null;
        break;
      case "ready":
        shown = entry.summary !== null;
        break;
      case "error":
        shown = true;
        break;
      default:
        // Never asked, and now it will not be: nothing is known about this version.
        shown = false;
    }
    this.entries.set(doc, { kind: "stale", shown, why });
    if (this.current === doc) this.render();
    // Whether the document is signed at all is still not known: ask now, and let the answer decide
    // only whether the badge is there. Without this a signed document would end with no badge.
    if (notAsked && entry?.kind === "loading") this.ask(doc, true);
  }

  /** A document was closed: what is known about it goes. */
  forget(doc: number): void {
    clearTimeout(this.timers.get(doc));
    this.timers.delete(doc);
    this.entries.delete(doc);
    if (this.current === doc) {
      this.current = null;
      this.close(false);
      this.render();
    }
  }

  /** The state of the document in front, for tests and for other parts of the window. */
  get summary(): Summary | null {
    const entry = this.current === null ? undefined : this.entries.get(this.current);
    return entry?.kind === "ready" ? entry.summary : null;
  }

  get isOpen(): boolean {
    return this.open;
  }

  /** Opens or closes the panel, as the badge does. */
  toggle(): void {
    if (this.open) this.close(true);
    else this.show();
  }

  /** Asks for the signatures of a document after a short rest, once. */
  private schedule(doc: number): void {
    const delay = this.deps.delayMs ?? DEFAULT_SIGNATURES_DELAY_MS;
    this.entries.set(doc, { kind: "loading" });
    if (delay <= 0) {
      this.ask(doc);
      return;
    }
    this.timers.set(
      doc,
      setTimeout(() => {
        this.timers.delete(doc);
        if (this.entries.get(doc)?.kind === "loading") this.ask(doc);
      }, delay),
    );
  }

  /** `forStale`: the tab no longer holds the checked bytes, and the answer only tells whether to say so. */
  private ask(doc: number, forStale = false): void {
    const getSignatures = this.deps.api.getSignatures;
    if (!getSignatures) return;
    if (!forStale) this.entries.set(doc, { kind: "loading" });
    getSignatures.call(this.deps.api, doc).then(
      (info) => {
        // The document may be gone by now, or its tab may hold other bytes than the ones checked.
        const now = this.entries.get(doc);
        if (now?.kind === "stale" && now.shown === null) {
          this.entries.set(doc, { kind: "stale", shown: summarise(info) !== null, why: now.why });
          if (this.current === doc) this.render();
          return;
        }
        if (now?.kind !== "loading") return;
        this.entries.set(doc, { kind: "ready", info, summary: summarise(info) });
        if (this.current === doc) this.render();
      },
      () => {
        const now = this.entries.get(doc);
        if (now?.kind === "stale" && now.shown === null) {
          // The engine could not say whether the document is signed. A document that was changed
          // may well be one that was, so the badge says that this version was not checked.
          this.entries.set(doc, { kind: "stale", shown: true, why: now.why });
          if (this.current === doc) this.render();
          return;
        }
        if (now?.kind !== "loading") return;
        this.entries.set(doc, { kind: "error" });
        if (this.current === doc) this.render();
      },
    );
  }

  private show(): void {
    if (this.current === null) return;
    this.open = true;
    this.panel.hidden = false;
    this.badge.setAttribute("aria-expanded", "true");
    this.render();
    this.closeButton.focus({ preventScroll: true });
  }

  private close(returnFocus: boolean): void {
    if (!this.open) return;
    this.open = false;
    this.panel.hidden = true;
    this.badge.setAttribute("aria-expanded", "false");
    if (returnFocus && !this.badge.hidden) this.badge.focus({ preventScroll: true });
  }

  private render(): void {
    const entry = this.current === null ? undefined : this.entries.get(this.current);
    // The badge: only for a document that has signatures (or marks of them), or whose check failed.
    let kind: SignatureState | "marks" | "error" | "stale" | null = null;
    let label = "";
    if (entry?.kind === "ready" && entry.summary) {
      kind = entry.summary.kind;
      label = entry.summary.label;
    } else if (entry?.kind === "stale" && entry.shown === true) {
      kind = "stale";
      label = STALE_LABEL;
    } else if (entry?.kind === "error") {
      kind = "error";
      label = "Signatures not checked";
    }
    this.badge.hidden = kind === null;
    this.badge.className = kind === null ? "pill sig-badge" : `pill sig-badge sig-${kind}`;
    this.badge.replaceChildren();
    if (kind !== null) {
      this.badge.append(icon(this.root, kind), this.badgeLabel);
      this.badgeLabel.textContent = label;
      // The whole label is in the title: a narrow window may cut the badge short.
      this.badge.title = `${label}. Show the digital signatures of this document.`;
    }
    if (kind === null && this.open) this.close(false);
    if (this.open) this.renderBody(entry);
  }

  private renderBody(entry: Entry | undefined): void {
    const doc = this.root;
    this.body.replaceChildren();
    if (!entry || entry.kind === "loading") {
      this.body.append(el(doc, "p", "sig-note", "Checking the signatures…"));
      return;
    }
    if (entry.kind === "stale") {
      this.body.append(
        el(doc, "p", "sig-lead", "Only this panel reports on a signature. A picture of a signature on a page is only a picture."),
        el(doc, "p", "sig-alert", STALE_TEXT[entry.why]),
      );
      return;
    }
    if (entry.kind === "error") {
      this.body.append(
        el(doc, "p", "sig-note", "The signatures of this document could not be checked just now. This says nothing about whether it is signed. If it is, do not rely on its signature."),
      );
      const retry = el(doc, "button", "btn", "Check again");
      retry.type = "button";
      retry.addEventListener("click", () => {
        if (this.current === null) return;
        this.entries.delete(this.current);
        this.ask(this.current);
        this.render();
      });
      this.body.append(retry);
      return;
    }
    const { info } = entry;
    this.body.append(
      el(doc, "p", "sig-lead", "Only this panel reports on a signature. A picture of a signature on a page is only a picture."),
    );
    if (info.unlisted > 0) this.body.append(el(doc, "p", "sig-alert", unlistedText(info.unlisted)));
    const omitted = omittedText(info.total, info.items.length);
    if (omitted) this.body.append(el(doc, "p", "sig-note", omitted));
    if (info.items.length > 0) {
      const list = el(doc, "ol", "sig-list");
      for (const item of info.items) list.append(this.renderItem(item, info));
      this.body.append(list);
    }
    this.body.append(this.renderNotChecked(info));
  }

  private renderItem(item: SignatureItem, info: SignaturesInfo): HTMLElement {
    const doc = this.root;
    const state = knownState(item.state);
    const li = el(doc, "li", `sig-item sig-${state}`);
    const head = el(doc, "div", "sig-state");
    head.append(icon(doc, state), el(doc, "strong", undefined, STATE_LABEL[state]));
    li.append(head, el(doc, "p", "sig-meaning", STATE_MEANING[state]), el(doc, "p", "sig-why", reasonText(item.reason)));
    if (state !== "trusted") {
      // The name below is what a certificate says. Unless it led to the trust list, nothing ties it to a person.
      li.append(
        el(
          doc,
          "p",
          "sig-why",
          info.trustAnchors === 0
            ? "No signer is verified by this version; the name below is what the certificate says, not proof of who holds the key."
            : "The name below is what the certificate says, not proof of who holds the key.",
        ),
      );
    }
    const time = certificateTimeText(item.certificateTime);
    if (time) li.append(el(doc, "p", "sig-why", time));

    const certificate = el(doc, "dl", "sig-facts");
    fact(certificate, "Signer", item.signer);
    fact(certificate, "Organisation", item.organisation);
    fact(certificate, "Issued by", item.issuer);
    fact(certificate, "Valid from", item.validFrom);
    fact(certificate, "Valid until", item.validTo);
    fact(certificate, "Key", item.key);
    fact(certificate, "Digest", item.digest);
    fact(certificate, "Serial number", item.serial, true);
    fact(certificate, "SHA-256 fingerprint", item.fingerprint, true);
    fact(certificate, "On the trust list as", item.trustAnchor);
    if (certificate.childElementCount > 0) {
      li.append(el(doc, "h3", "sig-sub", "Certificate of the signer"), certificate);
    }

    const claims = el(doc, "dl", "sig-facts");
    fact(claims, "Time", item.claimedTime);
    fact(claims, "Reason", item.statedReason);
    fact(claims, "Place", item.statedLocation);
    if (claims.childElementCount > 0) {
      li.append(
        el(doc, "h3", "sig-sub", "What the file says (not checked)"),
        claims,
      );
    }

    const technical = el(doc, "details", "sig-tech");
    technical.append(el(doc, "summary", undefined, "Technical details"));
    const tech = el(doc, "dl", "sig-facts");
    fact(tech, "Form", item.subFilter);
    fact(tech, "Covers", describeRange(item.byteRange, item.fileLen));
    fact(tech, "File size", formatBytes(item.fileLen));
    fact(tech, "Certificates in the signature", String(item.certificates));
    if (item.docMdp > 0) fact(tech, "Changes allowed by the signer", docMdpText(item.docMdp));
    technical.append(tech);
    li.append(technical);
    return li;
  }

  private renderNotChecked(info: SignaturesInfo): HTMLElement {
    const doc = this.root;
    const section = el(doc, "section", "sig-limits");
    section.append(el(doc, "h3", "sig-sub", "What this does not check"));
    const list = el(doc, "ul");
    for (const line of NOT_CHECKED) list.append(el(doc, "li", undefined, line));
    section.append(list);
    section.append(
      el(
        doc,
        "p",
        "sig-note",
        info.trustAnchors === 0
          ? "This version ships no list of trusted signers."
          : `The trust list of this version holds ${info.trustAnchors} certificate${info.trustAnchors === 1 ? "" : "s"}.`,
      ),
    );
    return section;
  }
}

// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { CERT_WORDS, type CertificateInfo, type SignField, type SignedFile } from "../lib/certsign";
import { askCertificateSignature, type CertDialogApi } from "./certsign";

let layer: HTMLElement;

beforeEach(() => {
  document.body.replaceChildren();
  layer = document.createElement("div");
  layer.hidden = true;
  document.body.append(layer);
  HTMLCanvasElement.prototype.getContext = vi.fn(() => null) as never;
  HTMLElement.prototype.setPointerCapture = vi.fn();
  HTMLElement.prototype.releasePointerCapture = vi.fn();
});

const certificate: CertificateInfo = {
  name: "Anna Alvorlig",
  issuer: "Anna Alvorlig",
  selfSigned: true,
  serial: "01",
  fingerprint: "ab".repeat(32),
  notBefore: 1_577_836_800,
  notAfter: 1_600_000_000,
  now: 1_781_524_800,
  key: "ECDSA P-256",
  maySign: true,
  chain: 1,
};

const signed: SignedFile = { path: "/d/a (signed).pdf", name: "a (signed).pdf", field: "Signature1", signer: "Anna Alvorlig", fingerprint: "ab".repeat(32) };

function fake(over: Partial<CertDialogApi> = {}) {
  const calls: string[] = [];
  const api: CertDialogApi = {
    signPickCertificate: vi.fn(() => {
      calls.push("pick");
      return Promise.resolve("anna.p12");
    }),
    signLoadCertificate: vi.fn((password: string) => {
      calls.push(`load ${password}`);
      return Promise.resolve(certificate);
    }),
    signForgetCertificate: vi.fn(() => {
      calls.push("forget");
      return Promise.resolve();
    }),
    signSetImageMark: vi.fn(() => Promise.resolve()),
    signListFields: vi.fn((): Promise<SignField[]> => Promise.resolve([])),
    signDocument: vi.fn((_doc, args) => {
      calls.push(`sign ${JSON.stringify(args.place)} ${args.name}|${args.reason}|${args.location}`);
      return Promise.resolve(signed);
    }),
    render: vi.fn(() => Promise.resolve(new ArrayBuffer(0))),
    ...over,
  };
  return { api, calls };
}

const flush = () => new Promise((r) => setTimeout(r, 10));

function open(api: CertDialogApi, extra: Partial<Parameters<typeof askCertificateSignature>[0]> = {}) {
  const answer = askCertificateSignature({
    layer,
    api,
    doc: 7,
    fileName: "a.pdf",
    pages: [{ width: 612, height: 792 }, { width: 612, height: 792 }],
    page: 0,
    pickPicture: () => Promise.resolve(null),
    ...extra,
  });
  const form = layer.querySelector("form") as HTMLFormElement;
  const q = <T extends Element>(sel: string) => form.querySelector(sel) as T;
  return { answer, form, q };
}

const button = (root: ParentNode, label: string) =>
  [...root.querySelectorAll("button")].find((b) => b.textContent === label) as HTMLButtonElement;

async function openCertificate(form: HTMLFormElement, password = "secret") {
  button(form, CERT_WORDS.chooseFile).click();
  await flush();
  const input = form.querySelector('input[type="password"]') as HTMLInputElement;
  input.value = password;
  input.dispatchEvent(new Event("input"));
  button(form, CERT_WORDS.open).click();
  await flush();
}

function pointer(type: string, x: number, y: number): PointerEvent {
  const e = new MouseEvent(type, { clientX: x, clientY: y, button: 0, bubbles: true, cancelable: true }) as unknown as Record<string, unknown>;
  e.pointerId = 1;
  return e as unknown as PointerEvent;
}

describe("the box for signing with a certificate", () => {
  it("says what it does, with no promise, and starts with nothing to sign with", () => {
    const { form } = open(fake().api);
    expect(form.getAttribute("role")).toBe("dialog");
    expect(form.getAttribute("aria-modal")).toBe("true");
    expect(form.textContent).toContain(CERT_WORDS.intro);
    expect((button(form, CERT_WORDS.sign) as HTMLButtonElement).disabled).toBe(true);
    // The password field waits for a file.
    expect((form.querySelector('input[type="password"]') as HTMLInputElement).disabled).toBe(true);
    expect(layer.hidden).toBe(false);
  });

  it("opens the file with the password, shows the certificate as text, and clears the password", async () => {
    const { api, calls } = fake();
    const { form } = open(api);
    await openCertificate(form);
    expect(calls).toEqual(["pick", "load secret"]);
    const password = form.querySelector('input[type="password"]') as HTMLInputElement;
    expect(password.value).toBe("");
    expect(password.disabled).toBe(true);
    const summary = form.querySelector(".certsign-summary") as HTMLElement;
    expect(summary.hidden).toBe(false);
    expect(summary.textContent).toContain("Anna Alvorlig");
    expect(summary.textContent).toContain("ECDSA P-256");
    expect(summary.textContent).toContain("2020-01-01 to 2020-09-13");
    // Self-made and expired: one line each.
    const lines = [...form.querySelectorAll(".certsign-warnings li")].map((li) => li.textContent);
    expect(lines).toEqual([CERT_WORDS.selfMade, CERT_WORDS.expired]);
    // What the signature says is filled in from the certificate; the clock and the rule are said.
    expect((form.querySelectorAll('input[type="text"]')[0] as HTMLInputElement).value).toBe("Anna Alvorlig");
    expect(form.textContent).toContain(CERT_WORDS.clock);
    expect(form.textContent).toContain(CERT_WORDS.afterSigning);
    expect(form.textContent).toContain(CERT_WORDS.fileBeing("a.pdf", 2));
  });

  it("never puts markup from the certificate on the page", async () => {
    const { api } = fake({
      signLoadCertificate: vi.fn(() => Promise.resolve({ ...certificate, name: "<img src=x onerror=alert(1)>", issuer: "<b>x</b>" })),
    });
    const { form } = open(api);
    await openCertificate(form);
    expect(form.querySelector("img")).toBeNull();
    expect(form.querySelector(".certsign-summary b")).toBeNull();
    expect(form.querySelector(".certsign-summary")?.textContent).toContain("<img src=x onerror=alert(1)>");
  });

  it("says what went wrong with the password and lets the person try again", async () => {
    let tries = 0;
    const { api } = fake({
      signLoadCertificate: vi.fn(() => {
        tries += 1;
        return tries === 1 ? Promise.reject({ code: "signer_wrong_password", message: "" }) : Promise.resolve(certificate);
      }),
    });
    const { form } = open(api);
    await openCertificate(form, "nope");
    const status = form.querySelector(".certsign-status") as HTMLElement;
    expect(status.hidden).toBe(false);
    expect(status.textContent).toContain("That password is not right");
    const password = form.querySelector('input[type="password"]') as HTMLInputElement;
    expect(password.disabled).toBe(false);
    password.value = "right";
    button(form, CERT_WORDS.open).click();
    await flush();
    expect((form.querySelector(".certsign-summary") as HTMLElement).hidden).toBe(false);
  });

  it("explains a key that is not supported and does not offer the next step", async () => {
    const { api } = fake({ signLoadCertificate: vi.fn(() => Promise.reject({ code: "signer_key_unsupported", message: "Ed25519" })) });
    const { form } = open(api);
    await openCertificate(form);
    expect((form.querySelector(".certsign-status") as HTMLElement).textContent).toContain("This kind of key is not supported");
    expect((form.querySelector(".certsign-place") as HTMLElement).hidden).toBe(true);
    expect((button(form, CERT_WORDS.sign) as HTMLButtonElement).disabled).toBe(true);
  });

  it("signs with no box when that is chosen, and hands back the file", async () => {
    const { api, calls } = fake();
    const { answer, form } = open(api);
    await openCertificate(form);
    const none = form.querySelector('input[type="radio"][value="none"]') as HTMLInputElement;
    none.checked = true;
    none.dispatchEvent(new Event("change"));
    const [name, reason, place] = [...form.querySelectorAll('input[type="text"]')] as HTMLInputElement[];
    reason.value = "I have read this";
    place.value = "Oslo";
    name.dispatchEvent(new Event("input"));
    expect((button(form, CERT_WORDS.sign) as HTMLButtonElement).disabled).toBe(false);
    form.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(await answer).toEqual(signed);
    expect(calls.at(-1)).toBe('sign {"kind":"invisible"} Anna Alvorlig|I have read this|Oslo');
    expect(layer.querySelector("form")).toBeNull();
    // The certificate is not let go by the window: the app process did that when it signed.
    expect(calls).not.toContain("forget");
  });

  it("needs a box before a box can be signed, and takes a click on the page as a box of the usual size", async () => {
    const { api } = fake();
    const { answer, form } = open(api);
    await openCertificate(form);
    expect((button(form, CERT_WORDS.sign) as HTMLButtonElement).disabled).toBe(true);
    const canvas = form.querySelector("canvas.certsign-preview") as HTMLCanvasElement;
    canvas.getBoundingClientRect = () => ({ left: 0, top: 0, width: 306, height: 396, right: 306, bottom: 396, x: 0, y: 0 }) as DOMRect;
    canvas.dispatchEvent(pointer("pointerdown", 153, 198));
    canvas.dispatchEvent(pointer("pointerup", 153, 198));
    expect((button(form, CERT_WORDS.sign) as HTMLButtonElement).disabled).toBe(false);
    form.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    await answer;
    const place = (api.signDocument as ReturnType<typeof vi.fn>).mock.calls[0][1].place as { kind: string; page: number; rect: number[] };
    expect(place.kind).toBe("rect");
    expect(place.page).toBe(0);
    expect(place.rect).toEqual([306 - 110, 396 - 35, 306 + 110, 396 + 35]);
  });

  it("offers the signature fields of the file and signs in the one that was chosen", async () => {
    const { api, calls } = fake({
      signListFields: vi.fn(() => Promise.resolve([{ page: 1, rect: [10, 10, 200, 60] as [number, number, number, number], name: "approval" }])),
    });
    const { answer, form } = open(api);
    await openCertificate(form);
    const radio = form.querySelector('input[type="radio"][value="field"]') as HTMLInputElement;
    expect(radio.disabled).toBe(false);
    radio.checked = true;
    radio.dispatchEvent(new Event("change"));
    expect((form.querySelector("select") as HTMLSelectElement).value).toBe("approval");
    form.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    await answer;
    expect(calls.at(-1)).toContain('{"kind":"field","page":1,"name":"approval"}');
  });

  it("turns the field choice off when the file has none", async () => {
    const { form } = open(fake().api);
    await openCertificate(form);
    const radio = form.querySelector('input[type="radio"][value="field"]') as HTMLInputElement;
    expect(radio.disabled).toBe(true);
    expect(radio.parentElement?.title).toBe(CERT_WORDS.noFields);
  });

  it("hands an imported picture over as pixels before the signing, and a drawn one in the request", async () => {
    const { api, calls } = fake();
    const picture = { kind: "image" as const, width: 2, height: 1, rgba: new Uint8ClampedArray(8), color: [0, 0, 0, 255] as [number, number, number, number], aspect: 2 };
    const { answer, form } = open(api, { pickPicture: () => Promise.resolve(picture) });
    await openCertificate(form);
    button(form, CERT_WORDS.pictureAdd).click();
    await flush();
    expect(form.hidden).toBe(false);
    expect(button(form, CERT_WORDS.pictureChange)).toBeTruthy();
    const none = form.querySelector('input[type="radio"][value="none"]') as HTMLInputElement;
    none.checked = true;
    none.dispatchEvent(new Event("change"));
    form.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    await answer;
    expect(api.signSetImageMark).toHaveBeenCalledWith(2, 1, picture.rgba);
    const [, args] = (api.signDocument as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(args.mark).toEqual({ kind: "image" });
    expect(calls.at(-1)).toContain("sign");
  });

  it("gives a refusal in words and stays open for another try", async () => {
    const { api } = fake({ signDocument: vi.fn(() => Promise.reject({ code: "sign_already_signed", message: "" })) });
    const { form } = open(api);
    await openCertificate(form);
    const none = form.querySelector('input[type="radio"][value="none"]') as HTMLInputElement;
    none.checked = true;
    none.dispatchEvent(new Event("change"));
    form.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(layer.querySelector("form")).toBe(form);
    expect((form.querySelector(".certsign-status") as HTMLElement).textContent).toContain("already has signatures");
    expect((button(form, CERT_WORDS.sign) as HTMLButtonElement).disabled).toBe(false);
  });

  it("stays open when the person cancels the box for the name of the copy", async () => {
    const { api } = fake({ signDocument: vi.fn(() => Promise.resolve(null)) });
    const { form } = open(api);
    await openCertificate(form);
    const none = form.querySelector('input[type="radio"][value="none"]') as HTMLInputElement;
    none.checked = true;
    none.dispatchEvent(new Event("change"));
    form.dispatchEvent(new Event("submit", { cancelable: true }));
    await flush();
    expect(layer.querySelector("form")).toBe(form);
    expect((form.querySelector(".certsign-status") as HTMLElement).hidden).toBe(true);
  });

  it("lets the key go when the person cancels or presses Escape", async () => {
    const first = fake();
    const one = open(first.api);
    await openCertificate(one.form);
    button(one.form, CERT_WORDS.cancel).click();
    expect(await one.answer).toBeNull();
    expect(first.calls).toContain("forget");
    expect(layer.querySelector("form")).toBeNull();

    const second = fake();
    const two = open(second.api);
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(await two.answer).toBeNull();
    expect(second.calls).toContain("forget");
  });
});

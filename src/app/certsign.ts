// The box for signing a document with a certificate.
//
// Three things happen in it: the person opens a certificate file (a file the app process asks for
// with the dialog of the system, and a password that goes to the signer and is not kept), says where
// the signature shows and what it says, and signs. The window never sees the key or the file. What
// it shows about the certificate comes from the file and is shown as text.
//
// The words are in lib/certsign.ts. Nothing here promises what a signature is worth.

import type { CertSignApi } from "../lib/api";
import { describeError, toAppError } from "../lib/errors";
import {
  CERT_WORDS,
  boxAround,
  boxBetween,
  certificateWarnings,
  cleanText,
  describeSignError,
  periodText,
  type CertificateInfo,
  type SignField,
  type SignMark,
  type SignPlace,
  type SignedFile,
} from "../lib/certsign";
import type { Chosen } from "../lib/signpicture";
import type { PageSizePt } from "../viewer/layout";

export interface CertDialogApi extends CertSignApi {
  render(doc: number, page: number, width: number, height: number): Promise<ArrayBuffer>;
}

export interface CertDialogRequest {
  /** The layer of the message boxes. */
  layer: HTMLElement;
  api: CertDialogApi;
  doc: number;
  fileName: string;
  pages: PageSizePt[];
  /** The page the person is looking at; the box starts there. */
  page: number;
  /** Opens the box for a picture of the signature; resolves with the picture, or null. */
  pickPicture(): Promise<Chosen | null>;
}

type Where = "box" | "field" | "none";

/** How wide the preview of the page is, in CSS pixels. */
const PREVIEW_WIDTH = 260;

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

let counter = 0;

/** The mark the engine takes for a picture the person chose. Pixels of an image go in a call of their own. */
function markOf(picture: Chosen | null): SignMark {
  if (!picture) return { kind: "none" };
  if (picture.kind === "draw") {
    return { kind: "draw", width: picture.width, height: picture.height, line: picture.line, strokes: picture.strokes };
  }
  if (picture.kind === "type") return { kind: "type", font: picture.font, text: picture.text };
  return { kind: "image" };
}

export function askCertificateSignature(request: CertDialogRequest): Promise<SignedFile | null> {
  return new Promise((resolve) => {
    const { api, pages } = request;
    const previousFocus = document.activeElement as HTMLElement | null;
    const id = `certsign-${++counter}`;
    let certificate: CertificateInfo | null = null;
    let where: Where = "box";
    let page = Math.min(Math.max(request.page, 0), Math.max(0, pages.length - 1));
    let box: [number, number, number, number] | null = null;
    let fields: SignField[] = [];
    let fieldName = "";
    let picture: Chosen | null = null;
    let busy = false;
    let closed = false;

    const form = el("form", "sheet notice certsign");
    form.noValidate = true;
    form.setAttribute("role", "dialog");
    form.setAttribute("aria-modal", "true");
    form.setAttribute("aria-labelledby", `${id}-title`);
    form.setAttribute("aria-describedby", `${id}-intro`);
    const title = el("h2", "sheet-title", CERT_WORDS.title);
    title.id = `${id}-title`;
    const intro = el("p", "sheet-text", CERT_WORDS.intro);
    intro.id = `${id}-intro`;

    // ---- the certificate file ------------------------------------------------------------
    const fileSection = el("section", "certsign-section");
    const chooseRow = el("div", "certsign-row");
    const choose = el("button", "btn btn-secondary", CERT_WORDS.chooseFile);
    choose.type = "button";
    choose.id = `${id}-choose`;
    const fileName = el("span", "certsign-file muted", CERT_WORDS.fileHint);
    chooseRow.append(choose, fileName);
    const passwordLabel = el("label", "certsign-field");
    const passwordText = el("span", undefined, CERT_WORDS.password);
    const password = el("input", "input");
    password.type = "password";
    password.id = `${id}-password`;
    password.autocomplete = "off";
    password.spellcheck = false;
    password.disabled = true;
    password.setAttribute("aria-describedby", `${id}-password-hint`);
    passwordLabel.append(passwordText, password);
    const passwordHint = el("p", "muted certsign-hint", `${CERT_WORDS.passwordHint} ${CERT_WORDS.notKept}`);
    passwordHint.id = `${id}-password-hint`;
    const openRow = el("div", "certsign-row");
    const open = el("button", "btn btn-secondary", CERT_WORDS.open);
    open.type = "button";
    open.id = `${id}-open`;
    open.disabled = true;
    openRow.append(open);
    const summary = el("dl", "certsign-summary");
    summary.hidden = true;
    const warnings = el("ul", "certsign-warnings");
    warnings.hidden = true;
    fileSection.append(chooseRow, passwordLabel, passwordHint, openRow, summary, warnings);

    // ---- where the signature shows --------------------------------------------------------
    const placeSection = el("fieldset", "certsign-section certsign-place");
    placeSection.hidden = true;
    const placeLegend = el("legend", "certsign-legend", CERT_WORDS.placeTitle);
    placeSection.append(placeLegend);
    const radios = new Map<Where, HTMLInputElement>();
    const radioFor = (value: Where, label: string): HTMLLabelElement => {
      const wrap = el("label", "certsign-radio");
      const input = el("input");
      input.type = "radio";
      input.name = `${id}-where`;
      input.value = value;
      input.checked = value === where;
      input.addEventListener("change", () => {
        where = value;
        sync();
      });
      radios.set(value, input);
      wrap.append(input, el("span", undefined, label));
      return wrap;
    };
    const boxPanel = el("div", "certsign-panel");
    const pageRow = el("div", "certsign-row");
    const pageLabel = el("label", "certsign-field certsign-page");
    const pageInput = el("input", "input");
    pageInput.type = "number";
    pageInput.min = "1";
    pageInput.max = String(Math.max(1, pages.length));
    pageInput.value = String(page + 1);
    pageLabel.append(el("span", undefined, CERT_WORDS.pageLabel), pageInput);
    const boxHint = el("span", "muted certsign-hint", CERT_WORDS.placeBoxHint);
    pageRow.append(pageLabel, boxHint);
    const stage = el("div", "certsign-stage");
    const canvas = el("canvas", "certsign-preview");
    canvas.setAttribute("role", "img");
    canvas.setAttribute("aria-label", "The page. Drag on it to draw the box for the signature.");
    const marker = el("div", "certsign-box");
    marker.hidden = true;
    const previewNote = el("p", "muted certsign-hint", "");
    previewNote.hidden = true;
    stage.append(canvas, marker);
    boxPanel.append(pageRow, stage, previewNote);
    const fieldPanel = el("div", "certsign-panel");
    const fieldSelect = el("select", "select");
    fieldSelect.setAttribute("aria-label", CERT_WORDS.placeFieldHint);
    fieldPanel.append(fieldSelect);
    placeSection.append(
      radioFor("box", CERT_WORDS.placeBox),
      boxPanel,
      radioFor("field", CERT_WORDS.placeField),
      fieldPanel,
      radioFor("none", CERT_WORDS.placeNone),
    );

    // ---- what it says ----------------------------------------------------------------------
    const textSection = el("fieldset", "certsign-section");
    textSection.hidden = true;
    textSection.append(el("legend", "certsign-legend", CERT_WORDS.textsTitle));
    const textField = (label: string): HTMLInputElement => {
      const wrap = el("label", "certsign-field");
      const input = el("input", "input");
      input.type = "text";
      input.maxLength = 80;
      wrap.append(el("span", undefined, label), input);
      textSection.append(wrap);
      return input;
    };
    textSection.append(el("p", "muted certsign-hint", CERT_WORDS.signerName));
    const reasonInput = textField(CERT_WORDS.reason);
    const placeInput = textField(CERT_WORDS.location);
    const pictureRow = el("div", "certsign-row");
    const pictureButton = el("button", "btn btn-secondary", CERT_WORDS.pictureAdd);
    pictureButton.type = "button";
    const pictureRemove = el("button", "btn btn-secondary", CERT_WORDS.pictureRemove);
    pictureRemove.type = "button";
    pictureRemove.hidden = true;
    pictureRow.append(el("span", "certsign-legend", CERT_WORDS.pictureTitle), pictureButton, pictureRemove);
    const pictureNote = el("p", "muted certsign-hint", CERT_WORDS.pictureNote);
    textSection.append(pictureRow, pictureNote);

    const facts = el("div", "certsign-facts");
    facts.hidden = true;
    facts.append(
      el("p", "sheet-text", CERT_WORDS.fileBeing(request.fileName, pages.length)),
      el("p", "sheet-text", CERT_WORDS.clock),
      el("p", "sheet-text", CERT_WORDS.afterSigning),
    );
    const status = el("p", "certsign-status");
    status.setAttribute("role", "alert");
    status.hidden = true;

    const buttons = el("div", "sheet-buttons");
    const cancel = el("button", "btn btn-secondary", CERT_WORDS.cancel);
    cancel.type = "button";
    const sign = el("button", "btn btn-primary", CERT_WORDS.sign);
    sign.type = "submit";
    sign.disabled = true;
    buttons.append(cancel, sign);
    form.append(title, intro, fileSection, placeSection, textSection, facts, status, buttons);

    // ---- helpers ----------------------------------------------------------------------------
    const say = (text: string) => {
      status.textContent = text;
      status.hidden = text === "";
    };
    const finish = (result: SignedFile | null) => {
      if (closed) return;
      closed = true;
      password.value = "";
      document.removeEventListener("keydown", onKey, true);
      form.remove();
      if (request.layer.childElementCount === 0) request.layer.hidden = true;
      if (!result) void api.signForgetCertificate().catch(() => undefined);
      previousFocus?.focus?.();
      resolve(result);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy && !form.hidden) {
        e.preventDefault();
        e.stopPropagation();
        finish(null);
      }
    };
    document.addEventListener("keydown", onKey, true);

    const place = (): SignPlace | null => {
      if (where === "none") return { kind: "invisible" };
      if (where === "box") return box ? { kind: "rect", page, rect: box } : null;
      const f = fields.find((x) => x.name === fieldName);
      return f ? { kind: "field", page: f.page, name: f.name } : null;
    };
    function sync() {
      const loaded = certificate !== null;
      placeSection.hidden = !loaded;
      textSection.hidden = !loaded;
      facts.hidden = !loaded;
      boxPanel.hidden = where !== "box";
      fieldPanel.hidden = where !== "field";
      const fieldsRadio = radios.get("field");
      if (fieldsRadio) {
        fieldsRadio.disabled = fields.length === 0;
        const wrap = fieldsRadio.parentElement as HTMLElement;
        wrap.title = fields.length === 0 ? CERT_WORDS.noFields : "";
      }
      pictureButton.textContent = picture ? CERT_WORDS.pictureChange : CERT_WORDS.pictureAdd;
      pictureRemove.hidden = picture === null;
      const ready = loaded && !busy && place() !== null;
      sign.disabled = !ready;
      choose.disabled = busy;
      open.disabled = busy || password.disabled;
      if (loaded && where === "box" && box === null) previewNote.textContent = CERT_WORDS.choosePageFirst;
      previewNote.hidden = !(loaded && where === "box" && box === null) && previewNote.dataset.failed !== "1";
    }

    // ---- the preview of the page ----------------------------------------------------------
    let previewToken = 0;
    const showMarker = () => {
      const size = pages[page];
      const shown = where === "box" && box !== null && size !== undefined;
      marker.hidden = !shown;
      if (!shown || !box) return;
      marker.style.left = `${(box[0] / size.width) * 100}%`;
      marker.style.top = `${(box[1] / size.height) * 100}%`;
      marker.style.width = `${((box[2] - box[0]) / size.width) * 100}%`;
      marker.style.height = `${((box[3] - box[1]) / size.height) * 100}%`;
    };
    const drawPreview = async () => {
      const size = pages[page];
      if (!size || size.width <= 0) return;
      const token = ++previewToken;
      const width = Math.round(PREVIEW_WIDTH * Math.min(2, window.devicePixelRatio || 1));
      const height = Math.max(1, Math.round((width * size.height) / size.width));
      canvas.style.aspectRatio = `${size.width} / ${size.height}`;
      previewNote.dataset.failed = "";
      try {
        const buffer = await api.render(request.doc, page, width, height);
        if (token !== previewToken || closed) return;
        if (buffer.byteLength !== width * height * 4) throw new Error("size");
        canvas.width = width;
        canvas.height = height;
        canvas.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(buffer), width, height), 0, 0);
      } catch {
        if (token !== previewToken || closed) return;
        previewNote.textContent = CERT_WORDS.previewFailed;
        previewNote.dataset.failed = "1";
      }
      sync();
      showMarker();
    };
    pageInput.addEventListener("change", () => {
      const wanted = Math.round(Number(pageInput.value)) - 1;
      page = Math.min(Math.max(Number.isFinite(wanted) ? wanted : page, 0), Math.max(0, pages.length - 1));
      pageInput.value = String(page + 1);
      box = null;
      sync();
      void drawPreview();
    });
    const toPoint = (e: PointerEvent): [number, number] => {
      const rect = canvas.getBoundingClientRect();
      const size = pages[page];
      return [
        ((e.clientX - rect.left) / (rect.width || 1)) * size.width,
        ((e.clientY - rect.top) / (rect.height || 1)) * size.height,
      ];
    };
    let drag: { id: number; from: [number, number] } | null = null;
    canvas.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || !pages[page]) return;
      e.preventDefault();
      try {
        canvas.setPointerCapture(e.pointerId);
      } catch {
        // The pointer is followed while it stays over the page.
      }
      drag = { id: e.pointerId, from: toPoint(e) };
    });
    canvas.addEventListener("pointermove", (e) => {
      if (!drag || drag.id !== e.pointerId) return;
      const size = pages[page];
      const next = boxBetween(drag.from, toPoint(e), size);
      if (next) {
        box = next;
        showMarker();
      }
    });
    const endDrag = (e: PointerEvent, keep: boolean) => {
      if (!drag || drag.id !== e.pointerId) return;
      const size = pages[page];
      const from = drag.from;
      drag = null;
      if (!keep) return;
      const to = toPoint(e);
      // A click that did not move gives a box of the usual size around the point.
      const moved = Math.hypot(to[0] - from[0], to[1] - from[1]) > 6;
      box = moved ? (boxBetween(from, to, size) ?? box) : boxAround(to[0], to[1], size);
      showMarker();
      sync();
    };
    canvas.addEventListener("pointerup", (e) => endDrag(e, true));
    canvas.addEventListener("pointercancel", (e) => endDrag(e, false));

    // ---- opening the file --------------------------------------------------------------------
    choose.addEventListener("click", () => {
      say("");
      void api
        .signPickCertificate()
        .then((name) => {
          if (closed || name === null) return;
          fileName.textContent = name;
          fileName.classList.remove("muted");
          certificate = null;
          summary.hidden = true;
          warnings.hidden = true;
          password.disabled = false;
          password.value = "";
          password.focus();
          sync();
        })
        .catch((e: unknown) => {
          const told = describeSignError(toAppError(e));
          say(`${told.title}. ${told.detail}`);
        });
    });
    password.addEventListener("input", () => sync());
    password.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        if (!open.disabled) open.click();
      }
    });
    const showCertificate = (c: CertificateInfo) => {
      summary.replaceChildren();
      const add = (term: string, value: string) => {
        summary.append(el("dt", undefined, term), el("dd", undefined, value));
      };
      add(CERT_WORDS.nameLabel, c.name);
      add(CERT_WORDS.issuerLabel, c.issuer);
      add(CERT_WORDS.periodLabel, periodText(c));
      add(CERT_WORDS.keyLabel, c.key);
      add(CERT_WORDS.chainLabel, String(c.chain));
      summary.hidden = false;
      warnings.replaceChildren(...certificateWarnings(c).map((line) => el("li", undefined, line)));
      warnings.hidden = warnings.childElementCount === 0;
    };
    open.addEventListener("click", () => {
      if (busy) return;
      busy = true;
      say(CERT_WORDS.openWorking);
      sync();
      const typed = password.value;
      void api
        .signLoadCertificate(request.doc, typed)
        .then(async (c) => {
          certificate = c;
          password.value = "";
          password.disabled = true;
          showCertificate(c);
          say("");
          try {
            fields = await api.signListFields(request.doc);
          } catch {
            fields = [];
          }
          fieldSelect.replaceChildren(
            ...fields.map((f) => {
              const o = el("option", undefined, `${f.name} (page ${f.page + 1})`);
              o.value = f.name;
              return o;
            }),
          );
          fieldName = fields[0]?.name ?? "";
          if (fields.length > 0 && !radios.get("box")?.checked) fieldSelect.value = fieldName;
          void drawPreview();
        })
        .catch((e: unknown) => {
          const error = toAppError(e);
          const told = error.code.startsWith("sign") ? describeSignError(error) : describeError(error);
          say(`${told.title}. ${told.detail}`);
          if (error.code !== "signer_wrong_password") password.disabled = true;
          if (error.code === "signer_wrong_password") password.select();
        })
        .finally(() => {
          busy = false;
          sync();
        });
    });
    fieldSelect.addEventListener("change", () => {
      fieldName = fieldSelect.value;
      sync();
    });

    // ---- the picture -------------------------------------------------------------------------
    pictureButton.addEventListener("click", () => {
      if (busy) return;
      form.hidden = true;
      void request
        .pickPicture()
        .then((chosen) => {
          if (chosen && !closed) picture = chosen;
        })
        .catch(() => undefined)
        .finally(() => {
          form.hidden = false;
          sync();
          pictureButton.focus();
        });
    });
    pictureRemove.addEventListener("click", () => {
      picture = null;
      sync();
    });

    // ---- signing -----------------------------------------------------------------------------
    cancel.addEventListener("click", () => {
      if (!busy) finish(null);
    });
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      const chosenPlace = place();
      if (busy || !certificate || !chosenPlace) return;
      busy = true;
      say(CERT_WORDS.working);
      sync();
      const run = async () => {
        if (picture && picture.kind === "image") {
          await api.signSetImageMark(picture.width, picture.height, picture.rgba);
        }
        return api.signDocument(request.doc, {
          place: chosenPlace,
          reason: cleanText(reasonInput.value),
          location: cleanText(placeInput.value),
          mark: markOf(picture),
          color: picture && picture.kind !== "image" ? picture.color : [20, 20, 22, 255],
        });
      };
      void run()
        .then((result) => {
          // The person cancelled the dialog for the name of the copy: nothing was signed, and they
          // may go on or give up.
          if (result === null) {
            say("");
            return;
          }
          finish(result);
        })
        .catch(async (e: unknown) => {
          const error = toAppError(e);
          const told = error.code.startsWith("sign") ? describeSignError(error, `"${request.fileName}"`) : describeError(error, request.fileName);
          say(`${told.title}. ${told.detail}`);
          // The signer ends after the one signature it makes, and lets the key go after some
          // minutes: when it is gone, the password is asked for again.
          const open = await api.signCertificateOpen(request.doc).catch(() => false);
          if (!open && !closed) {
            certificate = null;
            summary.hidden = true;
            warnings.hidden = true;
            password.disabled = false;
            password.value = "";
            say(`${status.textContent ?? ""} ${CERT_WORDS.keyLetGo}`.trim());
          }
        })
        .finally(() => {
          busy = false;
          if (!closed) sync();
        });
    });

    request.layer.append(form);
    request.layer.hidden = false;
    sync();
    choose.focus();
  });
}

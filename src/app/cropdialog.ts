// The box for cropping pages: a picture of one page with a frame on it that can be dragged by its
// edges and corners, the four margins as numbers, and which pages to crop. The words say what a
// crop does and does not do: it hides the edges, it removes nothing.

import {
  MIN_CROP_SIDE,
  TOOL_NOTES,
  boxOfMargins,
  clampMargins,
  dragBox,
  marginsOfBox,
  type Box,
  type Handle,
  type Margins,
} from "../lib/pages";
import type { PageSizePt } from "../viewer/layout";

export interface CropScope {
  id: string;
  label: string;
  /** The pages (counted from 0) this choice crops. */
  pages: number[];
}

export interface CropRequest {
  /** The layer of the message boxes. */
  layer: HTMLElement;
  name: string;
  /** The page shown in the picture (counted from 0) and its size as it is drawn, in points. */
  page: number;
  size: PageSizePt;
  /** Which pages the crop may apply to; the first is chosen to begin with. */
  scopes: CropScope[];
  /** Draws the page at a size, as raw RGBA. */
  render(width: number, height: number): Promise<ArrayBuffer>;
}

export interface CropAnswer {
  /** Left, top, right, bottom in points on the page as it is drawn. */
  margins: [number, number, number, number];
  pages: number[];
}

/** Longest side of the picture, in CSS pixels. */
const PREVIEW_SIDE = 340;
const MM_PER_POINT = 25.4 / 72;
/** How far the frame starts in from each edge, in points (half an inch). */
const START_MARGIN = 36;

const HANDLES: Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

function mm(points: number): string {
  return (points * MM_PER_POINT).toFixed(1);
}

export function askCrop(request: CropRequest): Promise<CropAnswer | null> {
  return new Promise((resolve) => {
    const { size } = request;
    const previousFocus = document.activeElement as HTMLElement | null;
    const scale = Math.min(PREVIEW_SIDE / size.width, PREVIEW_SIDE / size.height);
    const pw = Math.max(40, Math.round(size.width * scale));
    const ph = Math.max(40, Math.round(size.height * scale));

    let margins: Margins = clampMargins(
      { left: START_MARGIN, top: START_MARGIN, right: START_MARGIN, bottom: START_MARGIN },
      size.width,
      size.height,
    );

    const form = document.createElement("form");
    form.className = "sheet notice crop-sheet";
    form.noValidate = true;
    form.setAttribute("role", "dialog");
    form.setAttribute("aria-modal", "true");
    form.setAttribute("aria-labelledby", "crop-title");

    const title = document.createElement("h2");
    title.className = "sheet-title";
    title.id = "crop-title";
    title.textContent = "Crop pages";
    const note = document.createElement("p");
    note.className = "sheet-text crop-note";
    note.id = "crop-note";
    note.textContent = TOOL_NOTES.crop;
    form.setAttribute("aria-describedby", "crop-note");

    // The picture, with the frame over it.
    const stage = document.createElement("div");
    stage.className = "crop-stage";
    stage.style.width = `${pw}px`;
    stage.style.height = `${ph}px`;
    const canvas = document.createElement("canvas");
    canvas.className = "crop-canvas";
    canvas.width = pw;
    canvas.height = ph;
    const frame = document.createElement("div");
    frame.className = "crop-box";
    frame.dataset.handle = "move";
    frame.setAttribute("aria-hidden", "true");
    for (const h of HANDLES) {
      const el = document.createElement("span");
      el.className = "crop-handle";
      el.dataset.handle = h;
      frame.append(el);
    }
    stage.append(canvas, frame);

    // The margins as numbers, in millimetres.
    const fields = document.createElement("div");
    fields.className = "crop-fields";
    const inputs = new Map<keyof Margins, HTMLInputElement>();
    for (const [key, label] of [
      ["left", "Left"],
      ["top", "Top"],
      ["right", "Right"],
      ["bottom", "Bottom"],
    ] as const) {
      const wrap = document.createElement("label");
      wrap.className = "crop-field";
      const span = document.createElement("span");
      span.textContent = `${label} (mm)`;
      const input = document.createElement("input");
      input.type = "number";
      input.min = "0";
      input.step = "any";
      input.className = "field-input";
      input.id = `crop-${key}`;
      wrap.append(span, input);
      fields.append(wrap);
      inputs.set(key, input);
    }

    // Which pages.
    const scopeBox = document.createElement("fieldset");
    scopeBox.className = "crop-scopes";
    const legend = document.createElement("legend");
    legend.textContent = "Crop";
    scopeBox.append(legend);
    request.scopes.forEach((scope, i) => {
      const label = document.createElement("label");
      label.className = "crop-scope";
      const radio = document.createElement("input");
      radio.type = "radio";
      radio.name = "crop-scope";
      radio.value = scope.id;
      radio.checked = i === 0;
      const text = document.createElement("span");
      text.textContent = scope.label;
      label.append(radio, text);
      scopeBox.append(label);
    });

    const foot = document.createElement("div");
    foot.className = "sheet-foot";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "btn btn-secondary";
    cancel.textContent = "Cancel";
    const go = document.createElement("button");
    go.type = "submit";
    go.className = "btn btn-primary";
    go.textContent = "Crop";
    foot.append(cancel, go);

    form.append(title, note, stage, fields, scopeBox, foot);

    // ---- the frame and the numbers follow each other ---------------------------------------
    const draw = () => {
      const box = boxOfMargins(margins, size.width, size.height);
      frame.style.left = `${box.left * scale}px`;
      frame.style.top = `${box.top * scale}px`;
      frame.style.width = `${(box.right - box.left) * scale}px`;
      frame.style.height = `${(box.bottom - box.top) * scale}px`;
      for (const [key, input] of inputs) {
        if (document.activeElement !== input) input.value = mm(margins[key]);
      }
      go.disabled = margins.left + margins.top + margins.right + margins.bottom < 0.05;
    };
    for (const [key, input] of inputs) {
      input.addEventListener("input", () => {
        const points = Number(input.value) / MM_PER_POINT;
        if (!Number.isFinite(points)) return;
        margins = clampMargins({ ...margins, [key]: points }, size.width, size.height);
        draw();
      });
      input.addEventListener("blur", () => {
        input.value = mm(margins[key]);
      });
    }

    // Dragging a handle, or the whole frame.
    let drag: { handle: Handle; x: number; y: number; start: Box } | null = null;
    stage.addEventListener("pointerdown", (e) => {
      const target = (e.target as HTMLElement).closest<HTMLElement>("[data-handle]");
      if (!target || e.button !== 0) return;
      e.preventDefault();
      drag = {
        handle: target.dataset.handle as Handle,
        x: e.clientX,
        y: e.clientY,
        start: boxOfMargins(margins, size.width, size.height),
      };
      stage.setPointerCapture(e.pointerId);
    });
    stage.addEventListener("pointermove", (e) => {
      if (!drag) return;
      const dx = (e.clientX - drag.x) / scale;
      const dy = (e.clientY - drag.y) / scale;
      const box = dragBox(drag.start, drag.handle, dx, dy, size.width, size.height, MIN_CROP_SIDE);
      margins = clampMargins(marginsOfBox(box, size.width, size.height), size.width, size.height);
      draw();
    });
    const endDrag = () => {
      drag = null;
    };
    stage.addEventListener("pointerup", endDrag);
    stage.addEventListener("pointercancel", endDrag);

    // ---- finishing -------------------------------------------------------------------------
    const finish = (answer: CropAnswer | null) => {
      form.remove();
      request.layer.hidden = request.layer.childElementCount === 0;
      request.layer.removeEventListener("keydown", onKey);
      previousFocus?.focus?.();
      resolve(answer);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && request.layer.lastElementChild === form) {
        e.preventDefault();
        e.stopPropagation();
        finish(null);
      }
    };
    request.layer.addEventListener("keydown", onKey);
    cancel.addEventListener("click", () => finish(null));
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      if (go.disabled) return;
      const chosen = form.querySelector<HTMLInputElement>('input[name="crop-scope"]:checked')?.value;
      const scope = request.scopes.find((s) => s.id === chosen) ?? request.scopes[0];
      finish({
        margins: [margins.left, margins.top, margins.right, margins.bottom],
        pages: scope.pages,
      });
    });

    request.layer.append(form);
    request.layer.hidden = false;
    draw();
    inputs.get("left")?.focus();

    // The picture comes when the engine has drawn it; the frame works before that.
    void request
      .render(pw * 2, ph * 2)
      .then((rgba) => {
        if (!form.isConnected || rgba.byteLength !== pw * 2 * ph * 2 * 4) return;
        const big = document.createElement("canvas");
        big.width = pw * 2;
        big.height = ph * 2;
        big.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(rgba), pw * 2, ph * 2), 0, 0);
        const ctx = canvas.getContext("2d");
        ctx?.drawImage(big, 0, 0, pw, ph);
      })
      .catch(() => undefined);
  });
}

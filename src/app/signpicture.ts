// The box for making a picture of a signature: draw it, type it, or import an image of it, or pick
// one that was saved. The box says what a picture is and is not (PICTURE_WORDS). It makes the
// picture and hands it back; placing it on a page is the annotator's work.
//
// Everything typed or loaded here is data: a typed name is set in a text field and shown as text, a
// kept signature is drawn as lines, as text or as an image from a blob, never as markup.

import type { Rgba } from "../lib/annotations";
import {
  INK_COLORS,
  MAX_IMAGE_FILE_BYTES,
  MAX_NAME_CHARS,
  MAX_SAVED,
  MAX_SAVED_PNG_BYTES,
  MAX_STROKES,
  MAX_TEXT_CHARS,
  PEN_THICKNESS,
  PICTURE_WORDS,
  SIGNATURE_FONTS,
  base64Bytes,
  bodyOf,
  canBeginStroke,
  cleanName,
  cleanTypedText,
  drawnPicture,
  fitImageSize,
  fontInfo,
  fromBase64,
  hasInk,
  pictureOfSaved,
  strokeTotals,
  toBase64,
  typedOk,
  whiteToTransparent,
  type Chosen,
  type FontId,
  type Point,
  type SavedBody,
  type SavedPicture,
} from "../lib/signpicture";

/** The saved signatures, as the app process keeps them. */
export interface PictureStore {
  list(): Promise<SavedPicture[]>;
  add(name: string, body: SavedBody): Promise<SavedPicture>;
  rename(id: string, name: string): Promise<SavedPicture>;
  remove(id: string): Promise<void>;
}

/** An image read into pixels. */
export interface DecodedImage {
  width: number;
  height: number;
  rgba: Uint8ClampedArray;
}

/** What the box needs from outside: the saved signatures, and the reading and writing of images. */
export interface PictureTools {
  store: PictureStore | null;
  /** Reads an image file into pixels no bigger than the limits. Rejects when it is not a picture. */
  decode(blob: Blob): Promise<DecodedImage>;
  /** Writes pixels as a PNG. */
  encodePng(image: DecodedImage): Promise<Uint8Array>;
  /** The pixels scaled to a smaller size. */
  scale(image: DecodedImage, width: number, height: number): Promise<DecodedImage>;
  /** Width over height of a text set in a font, or null when it cannot be measured. */
  measure(text: string, font: FontId): Promise<number | null>;
}

export interface PictureRequest {
  /** The layer of the message boxes. */
  layer: HTMLElement;
  tools: PictureTools;
}

export interface PictureAnswer {
  picture: Chosen;
}

type Tab = "draw" | "type" | "image" | "saved";

const TABS: { id: Tab; label: string }[] = [
  { id: "draw", label: "Draw" },
  { id: "type", label: "Type" },
  { id: "image", label: "Image" },
  { id: "saved", label: "Saved" },
];

const SURFACE = { width: 520, height: 180 };
const SVG_NS = "http://www.w3.org/2000/svg";
const DEFAULT_THRESHOLD = 235;

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

export function askSignaturePicture(request: PictureRequest): Promise<PictureAnswer | null> {
  return new Promise((resolve) => {
    const { tools } = request;
    const previousFocus = document.activeElement as HTMLElement | null;
    const id = `sigpic-${++counter}`;
    let tab: Tab = "draw";
    let color: Rgba = INK_COLORS[0].rgba;
    let thickness = PEN_THICKNESS[1].width;
    // Drawn lines, in pixels of the drawing surface.
    const strokes: Point[][] = [];
    let drawing: { pointerId: number; points: Point[] } | null = null;
    let text = "";
    let font: FontId = SIGNATURE_FONTS[0].id;
    let image: DecodedImage | null = null;
    let see = false;
    let threshold = DEFAULT_THRESHOLD;
    let saved: SavedPicture[] = [];
    let showFull = false;
    let closed = false;
    const urls: string[] = [];

    const form = el("form", "sheet notice sigpic");
    form.noValidate = true;
    form.setAttribute("role", "dialog");
    form.setAttribute("aria-modal", "true");
    form.setAttribute("aria-labelledby", `${id}-title`);
    form.setAttribute("aria-describedby", `${id}-note`);

    const title = el("h2", "sheet-title", PICTURE_WORDS.tool);
    title.id = `${id}-title`;
    const note = el("p", "sheet-text sigpic-note", PICTURE_WORDS.notice);
    note.id = `${id}-note`;

    // ---- the tabs -------------------------------------------------------------------------
    const tabList = el("div", "segmented sigpic-tabs");
    tabList.setAttribute("role", "tablist");
    tabList.setAttribute("aria-label", "How to make the picture");
    const tabButtons = new Map<Tab, HTMLButtonElement>();
    const panels = new Map<Tab, HTMLElement>();
    for (const t of TABS) {
      const b = el("button", undefined, t.label);
      b.type = "button";
      b.id = `${id}-tab-${t.id}`;
      b.setAttribute("role", "tab");
      b.setAttribute("aria-controls", `${id}-panel-${t.id}`);
      b.addEventListener("click", () => show(t.id));
      tabButtons.set(t.id, b);
      tabList.append(b);
      const p = el("div", "sigpic-panel");
      p.id = `${id}-panel-${t.id}`;
      p.setAttribute("role", "tabpanel");
      p.setAttribute("aria-labelledby", b.id);
      panels.set(t.id, p);
    }
    tabList.addEventListener("keydown", (e) => {
      if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
      const at = TABS.findIndex((t) => t.id === tab);
      const next = TABS[(at + (e.key === "ArrowRight" ? 1 : TABS.length - 1)) % TABS.length].id;
      e.preventDefault();
      show(next);
      tabButtons.get(next)?.focus();
    });

    // ---- draw ------------------------------------------------------------------------------
    const drawPanel = panels.get("draw") as HTMLElement;
    const drawHint = el("p", "sigpic-hint muted", PICTURE_WORDS.drawHint);
    const canvas = el("canvas", "sigpic-surface");
    canvas.width = SURFACE.width;
    canvas.height = SURFACE.height;
    canvas.setAttribute("role", "img");
    canvas.setAttribute("aria-label", "Drawing area for your signature. To use the keyboard, type your name on the Type tab.");
    const drawRow = el("div", "sigpic-row");
    const thicknessGroup = el("div", "segmented sigpic-thickness");
    thicknessGroup.setAttribute("role", "group");
    thicknessGroup.setAttribute("aria-label", "Pen thickness");
    const thicknessButtons: HTMLButtonElement[] = [];
    for (const t of PEN_THICKNESS) {
      const b = el("button", undefined, t.label);
      b.type = "button";
      b.addEventListener("click", () => {
        thickness = t.width;
        sync();
      });
      thicknessButtons.push(b);
      thicknessGroup.append(b);
    }
    const undoLine = el("button", "btn btn-secondary", "Undo line");
    undoLine.type = "button";
    const clear = el("button", "btn btn-secondary", "Clear");
    clear.type = "button";
    const drawStatus = el("span", "sigpic-status muted");
    drawStatus.setAttribute("role", "status");
    drawRow.append(thicknessGroup, undoLine, clear, drawStatus);
    drawPanel.append(drawHint, canvas, drawRow);

    const paintSurface = () => {
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      ctx.strokeStyle = `rgb(${color[0]} ${color[1]} ${color[2]})`;
      ctx.lineWidth = thickness;
      const all = drawing ? [...strokes, drawing.points] : strokes;
      for (const s of all) {
        ctx.beginPath();
        s.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
        if (s.length === 1) ctx.lineTo(s[0][0] + 0.01, s[0][1]);
        ctx.stroke();
      }
    };
    const at = (e: PointerEvent): Point => {
      const box = canvas.getBoundingClientRect();
      const w = box.width || SURFACE.width;
      const h = box.height || SURFACE.height;
      return [((e.clientX - box.left) / w) * SURFACE.width, ((e.clientY - box.top) / h) * SURFACE.height];
    };
    canvas.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 || drawing) return;
      e.preventDefault();
      if (!canBeginStroke(strokes)) {
        showFull = true;
        drawStatus.textContent = PICTURE_WORDS.full;
        return;
      }
      try {
        canvas.setPointerCapture(e.pointerId);
      } catch {
        // The pointer is followed while it stays over the surface.
      }
      drawing = { pointerId: e.pointerId, points: [at(e)] };
      paintSurface();
    });
    canvas.addEventListener("pointermove", (e) => {
      if (!drawing || drawing.pointerId !== e.pointerId) return;
      const batch = typeof e.getCoalescedEvents === "function" ? e.getCoalescedEvents() : [];
      const room = 4_096 - strokeTotals(strokes).points - drawing.points.length;
      for (const c of (batch.length > 0 ? batch : [e]).slice(0, Math.max(0, room))) drawing.points.push(at(c));
      paintSurface();
    });
    const endStroke = (e: PointerEvent, keep: boolean) => {
      if (!drawing || drawing.pointerId !== e.pointerId) return;
      try {
        canvas.releasePointerCapture(e.pointerId);
      } catch {
        // Nothing was captured.
      }
      const done = drawing.points;
      drawing = null;
      if (keep && done.length > 0) strokes.push(done);
      paintSurface();
      sync();
    };
    canvas.addEventListener("pointerup", (e) => endStroke(e, true));
    canvas.addEventListener("pointercancel", (e) => endStroke(e, false));
    undoLine.addEventListener("click", () => {
      strokes.pop();
      paintSurface();
      sync();
    });
    clear.addEventListener("click", () => {
      strokes.length = 0;
      paintSurface();
      sync();
    });

    // ---- type ------------------------------------------------------------------------------
    const typePanel = panels.get("type") as HTMLElement;
    const typeHint = el("p", "sigpic-hint muted", PICTURE_WORDS.typeHint);
    const typeLabel = el("label", "sigpic-field");
    const typeLabelText = el("span", undefined, "Your name");
    const typeInput = el("input", "field-input sigpic-text");
    typeInput.type = "text";
    typeInput.autocomplete = "off";
    typeInput.spellcheck = false;
    typeInput.maxLength = MAX_TEXT_CHARS;
    typeLabel.append(typeLabelText, typeInput);
    const fontGroup = el("div", "sigpic-fonts");
    fontGroup.setAttribute("role", "radiogroup");
    fontGroup.setAttribute("aria-label", "Font");
    const fontButtons = new Map<FontId, HTMLButtonElement>();
    for (const f of SIGNATURE_FONTS) {
      const b = el("button", "sigpic-font");
      b.type = "button";
      b.setAttribute("role", "radio");
      b.setAttribute("aria-label", f.label);
      b.dataset.font = f.id;
      const sample = el("span", "sigpic-font-sample", "Signature");
      sample.style.fontFamily = `"${f.family}", cursive`;
      sample.setAttribute("aria-hidden", "true");
      b.append(sample, el("span", "sigpic-font-name muted", f.label));
      b.addEventListener("click", () => {
        font = f.id;
        sync();
      });
      fontButtons.set(f.id, b);
      fontGroup.append(b);
    }
    const typePreview = el("div", "sigpic-preview");
    typePreview.setAttribute("aria-hidden", "true");
    const typeStatus = el("span", "sigpic-status muted");
    typeStatus.setAttribute("role", "status");
    typePanel.append(typeHint, typeLabel, fontGroup, typePreview, typeStatus);
    typeInput.addEventListener("input", () => {
      text = typeInput.value;
      sync();
    });
    typeInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        if (!go.disabled) form.requestSubmit();
      }
    });

    // ---- image -----------------------------------------------------------------------------
    const imagePanel = panels.get("image") as HTMLElement;
    const imageHint = el("p", "sigpic-hint muted", PICTURE_WORDS.imageHint);
    const imageLabel = el("label", "sigpic-field");
    const imageLabelText = el("span", undefined, "Picture file");
    const fileInput = el("input", "sigpic-file");
    fileInput.type = "file";
    fileInput.accept = "image/png,image/jpeg,image/webp,image/bmp,image/gif";
    imageLabel.append(imageLabelText, fileInput);
    const seeLabel = el("label", "sigpic-check");
    const seeBox = el("input");
    seeBox.type = "checkbox";
    seeLabel.append(seeBox, el("span", undefined, "Make the white background see-through"));
    const thresholdLabel = el("label", "sigpic-field sigpic-threshold");
    const thresholdText = el("span", undefined, "How white counts as background");
    const thresholdInput = el("input");
    thresholdInput.type = "range";
    thresholdInput.min = "120";
    thresholdInput.max = "254";
    thresholdInput.step = "1";
    thresholdInput.value = String(DEFAULT_THRESHOLD);
    thresholdLabel.append(thresholdText, thresholdInput);
    const imagePreview = el("canvas", "sigpic-image");
    imagePreview.setAttribute("role", "img");
    imagePreview.setAttribute("aria-label", "The picture as it will be placed");
    const imageStatus = el("span", "sigpic-status muted");
    imageStatus.setAttribute("role", "status");
    imagePanel.append(imageHint, imageLabel, seeLabel, thresholdLabel, imagePreview, imageStatus);

    const processed = (): DecodedImage | null => {
      if (!image) return null;
      return see ? { ...image, rgba: whiteToTransparent(image.rgba, threshold) } : image;
    };
    const paintImage = () => {
      const shown = processed();
      imagePreview.hidden = !shown;
      if (!shown) return;
      imagePreview.width = shown.width;
      imagePreview.height = shown.height;
      const ctx = imagePreview.getContext("2d");
      if (!ctx) return;
      ctx.clearRect(0, 0, shown.width, shown.height);
      ctx.putImageData(new ImageData(new Uint8ClampedArray(shown.rgba), shown.width, shown.height), 0, 0);
    };
    fileInput.addEventListener("change", () => {
      const file = fileInput.files?.[0];
      image = null;
      imageStatus.textContent = "";
      sync();
      if (!file) return;
      if (file.size > MAX_IMAGE_FILE_BYTES) {
        imageStatus.textContent = PICTURE_WORDS.imageTooBig;
        return;
      }
      // A picture is a raster file. Drawings that can carry scripts or links are not read.
      if (/svg|xml|html|pdf/i.test(file.type) || /\.(svgz?|html?|xml|pdf)$/i.test(file.name)) {
        imageStatus.textContent = PICTURE_WORDS.imageFailed;
        return;
      }
      void tools
        .decode(file)
        .then((decoded) => {
          if (closed) return;
          const size = fitImageSize(decoded.width, decoded.height);
          if (!size || decoded.rgba.length !== decoded.width * decoded.height * 4) {
            imageStatus.textContent = PICTURE_WORDS.imageFailed;
            return;
          }
          image = decoded;
          imageStatus.textContent = `${decoded.width} by ${decoded.height} pixels.`;
          sync();
        })
        .catch(() => {
          if (!closed) imageStatus.textContent = PICTURE_WORDS.imageFailed;
        });
    });
    seeBox.addEventListener("change", () => {
      see = seeBox.checked;
      sync();
    });
    thresholdInput.addEventListener("input", () => {
      threshold = Number(thresholdInput.value);
      sync();
    });

    // ---- saved -----------------------------------------------------------------------------
    const savedPanel = panels.get("saved") as HTMLElement;
    const savedNote = el("p", "sigpic-hint muted", PICTURE_WORDS.saved);
    const savedList = el("ul", "sigpic-saved");
    const savedStatus = el("span", "sigpic-status muted");
    savedStatus.setAttribute("role", "status");
    savedPanel.append(savedNote, savedList, savedStatus);

    const sampleOf = (item: SavedPicture): HTMLElement => {
      const box = el("span", "sigpic-sample");
      box.setAttribute("aria-hidden", "true");
      if (item.kind === "draw") {
        const svg = document.createElementNS(SVG_NS, "svg");
        svg.setAttribute("viewBox", `0 0 ${item.width} ${item.height}`);
        svg.setAttribute("preserveAspectRatio", "xMidYMid meet");
        for (const s of item.strokes) {
          const line = document.createElementNS(SVG_NS, "polyline");
          const pts = s.length === 1 ? [s[0], [s[0][0] + 0.01, s[0][1]]] : s;
          line.setAttribute("points", pts.map((p) => `${p[0]},${p[1]}`).join(" "));
          line.setAttribute("fill", "none");
          line.setAttribute("stroke", "currentColor");
          line.setAttribute("stroke-width", String(item.line));
          line.setAttribute("stroke-linecap", "round");
          line.setAttribute("stroke-linejoin", "round");
          svg.append(line);
        }
        box.append(svg);
      } else if (item.kind === "type") {
        const family = SIGNATURE_FONTS.find((f) => f.id === item.font)?.family;
        const span = el("span", "sigpic-sample-text", item.text);
        if (family) span.style.fontFamily = `"${family}", cursive`;
        box.append(span);
      } else {
        try {
          const url = URL.createObjectURL(new Blob([fromBase64(item.png) as BlobPart], { type: "image/png" }));
          urls.push(url);
          const img = el("img", "sigpic-sample-image");
          img.alt = "";
          img.src = url;
          box.append(img);
        } catch {
          box.append(el("span", "muted", "Image"));
        }
      }
      return box;
    };

    const useSaved = async (item: SavedPicture) => {
      savedStatus.textContent = "";
      try {
        if (item.kind === "image") {
          const bytes = fromBase64(item.png);
          const decoded = await tools.decode(new Blob([bytes as BlobPart], { type: "image/png" }));
          if (closed) return;
          finish({
            picture: { kind: "image", width: decoded.width, height: decoded.height, rgba: decoded.rgba, color, aspect: decoded.width / decoded.height },
          });
          return;
        }
        const picture = pictureOfSaved(item);
        if (!picture) {
          savedStatus.textContent = "This saved signature cannot be used.";
          return;
        }
        const aspect =
          picture.kind === "draw" ? picture.width / picture.height : ((await tools.measure(picture.text, picture.font)) ?? Math.max(1, picture.text.length * 0.42));
        finish({ picture: { ...picture, color, aspect } });
      } catch {
        if (!closed) savedStatus.textContent = "This saved signature cannot be used.";
      }
    };

    const renderSaved = () => {
      savedList.replaceChildren();
      if (!tools.store) {
        savedList.append(el("li", "muted", "Saved signatures are not available."));
        return;
      }
      if (saved.length === 0) {
        savedList.append(el("li", "muted sigpic-empty", "No signatures are saved yet. Make one on another tab and tick Save this signature."));
        return;
      }
      for (const item of saved) {
        const row = el("li", "sigpic-saved-row");
        row.dataset.id = item.id;
        const name = el("span", "sigpic-saved-name", item.name);
        const use = el("button", "btn btn-primary", "Use");
        use.type = "button";
        use.setAttribute("aria-label", `Use ${item.name}`);
        use.addEventListener("click", () => void useSaved(item));
        const rename = el("button", "btn btn-secondary", "Rename");
        rename.type = "button";
        rename.setAttribute("aria-label", `Rename ${item.name}`);
        rename.addEventListener("click", () => startRename(item, row));
        const remove = el("button", "btn btn-secondary", "Delete");
        remove.type = "button";
        remove.setAttribute("aria-label", `Delete ${item.name}`);
        remove.addEventListener("click", () => void removeSaved(item));
        row.append(sampleOf(item), name, use, rename, remove);
        savedList.append(row);
      }
    };

    const startRename = (item: SavedPicture, row: HTMLElement) => {
      const input = el("input", "field-input sigpic-rename");
      input.type = "text";
      input.maxLength = MAX_NAME_CHARS;
      input.value = item.name;
      input.setAttribute("aria-label", "New name");
      const ok = el("button", "btn btn-primary", "Rename");
      ok.type = "button";
      const cancel = el("button", "btn btn-secondary", "Cancel");
      cancel.type = "button";
      const commit = async () => {
        const next = cleanName(input.value);
        if (!next) {
          savedStatus.textContent = "A name has one to sixty characters.";
          return;
        }
        try {
          const renamed = await tools.store?.rename(item.id, next);
          if (renamed) saved = saved.map((s) => (s.id === item.id ? renamed : s));
          savedStatus.textContent = "";
        } catch {
          savedStatus.textContent = "The name could not be saved.";
        }
        renderSaved();
      };
      ok.addEventListener("click", () => void commit());
      cancel.addEventListener("click", () => renderSaved());
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          e.stopPropagation();
          void commit();
        } else if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          renderSaved();
        }
      });
      row.replaceChildren(input, ok, cancel);
      input.focus();
      input.select();
    };

    const removeSaved = async (item: SavedPicture) => {
      try {
        await tools.store?.remove(item.id);
        saved = saved.filter((s) => s.id !== item.id);
        savedStatus.textContent = "";
      } catch {
        savedStatus.textContent = "The signature could not be deleted.";
      }
      renderSaved();
    };

    // ---- ink colour, saving, and the buttons -----------------------------------------------
    const colourGroup = el("div", "sigpic-colours");
    colourGroup.setAttribute("role", "radiogroup");
    colourGroup.setAttribute("aria-label", "Colour of the ink");
    const colourButtons: HTMLButtonElement[] = [];
    for (const c of INK_COLORS) {
      const b = el("button", `swatch sigpic-ink-${c.id}`);
      b.type = "button";
      b.setAttribute("role", "radio");
      b.setAttribute("aria-label", c.label);
      b.title = c.label;
      b.addEventListener("click", () => {
        color = c.rgba;
        paintSurface();
        sync();
      });
      colourButtons.push(b);
      colourGroup.append(b);
    }

    const keepRow = el("div", "sigpic-keep");
    const keepLabel = el("label", "sigpic-check");
    const keepBox = el("input");
    keepBox.type = "checkbox";
    keepLabel.append(keepBox, el("span", undefined, "Save this signature on this computer for later"));
    const keepName = el("input", "field-input sigpic-name");
    keepName.type = "text";
    keepName.maxLength = MAX_NAME_CHARS;
    keepName.placeholder = "Name";
    keepName.setAttribute("aria-label", "Name of the saved signature");
    const keepStatus = el("span", "sigpic-status muted");
    keepStatus.setAttribute("role", "status");
    keepRow.append(keepLabel, keepName, keepStatus);
    keepBox.addEventListener("change", sync);
    keepName.addEventListener("input", sync);

    const foot = el("div", "sheet-foot");
    const cancel = el("button", "btn btn-secondary", "Cancel");
    cancel.type = "button";
    const go = el("button", "btn btn-primary", "Use on the page");
    go.type = "submit";
    foot.append(cancel, go);

    form.append(title, note, tabList);
    for (const p of panels.values()) form.append(p);
    form.append(colourGroup, keepRow, foot);

    // ---- what is ready ---------------------------------------------------------------------
    const current = (): Chosen | null => {
      if (tab === "draw") {
        const picture = drawnPicture(strokes, thickness);
        return picture ? { ...picture, color, aspect: picture.width / picture.height } : null;
      }
      if (tab === "type") {
        const clean = cleanTypedText(text);
        return typedOk(clean) ? { kind: "type", font, text: clean, color, aspect: typedAspect } : null;
      }
      if (tab === "image") {
        const shown = processed();
        if (!shown || !hasInk(shown.rgba)) return null;
        return { kind: "image", width: shown.width, height: shown.height, rgba: shown.rgba, color, aspect: shown.width / shown.height };
      }
      return null;
    };

    // The width over height of the typed name, measured with the font once it is loaded.
    let typedAspect = 4;
    let measuring = 0;
    const measureTyped = () => {
      const clean = cleanTypedText(text);
      const want = ++measuring;
      if (!typedOk(clean)) return;
      void tools
        .measure(clean, font)
        .then((aspect) => {
          if (want === measuring && aspect && Number.isFinite(aspect) && aspect > 0) typedAspect = aspect;
        })
        .catch(() => undefined);
    };

    function show(next: Tab): void {
      tab = next;
      sync();
    }

    function sync(): void {
      for (const [t, b] of tabButtons) {
        b.setAttribute("aria-selected", String(t === tab));
        b.tabIndex = t === tab ? 0 : -1;
        const p = panels.get(t) as HTMLElement;
        p.hidden = t !== tab;
      }
      const makes = tab !== "saved";
      colourGroup.hidden = !(tab === "draw" || tab === "type");
      colourButtons.forEach((b, i) => b.setAttribute("aria-checked", String(INK_COLORS[i].rgba === color)));
      keepRow.hidden = !makes || !tools.store;
      keepName.disabled = !keepBox.checked;
      go.hidden = !makes;
      thicknessButtons.forEach((b, i) => b.setAttribute("aria-pressed", String(PEN_THICKNESS[i].width === thickness)));
      const totals = strokeTotals(strokes);
      undoLine.disabled = strokes.length === 0;
      clear.disabled = strokes.length === 0;
      if (canBeginStroke(strokes)) showFull = false;
      drawStatus.textContent = showFull ? PICTURE_WORDS.full : totals.strokes > 0 ? `${totals.strokes} of ${MAX_STROKES} lines.` : "";
      // Typed name.
      const clean = cleanTypedText(text);
      typePreview.textContent = clean;
      typePreview.style.fontFamily = `"${fontInfo(font).family}", cursive`;
      typeStatus.textContent = Array.from(text).length > MAX_TEXT_CHARS ? PICTURE_WORDS.tooLong : "";
      for (const [f, b] of fontButtons) b.setAttribute("aria-checked", String(f === font));
      measureTyped();
      // Image.
      seeBox.checked = see;
      thresholdLabel.hidden = !see;
      paintImage();
      go.disabled = current() === null;
      if (tab === "saved") renderSaved();
    }

    // ---- finishing -------------------------------------------------------------------------
    const finish = (answer: PictureAnswer | null) => {
      if (closed) return;
      closed = true;
      form.remove();
      request.layer.hidden = request.layer.childElementCount === 0;
      request.layer.removeEventListener("keydown", onKey);
      for (const url of urls) URL.revokeObjectURL(url);
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

    const keep = async (picture: Chosen): Promise<boolean> => {
      if (!keepBox.checked || !tools.store) return true;
      const name = cleanName(keepName.value);
      if (!name) {
        keepStatus.textContent = "Give the saved signature a name.";
        keepName.focus();
        return false;
      }
      if (saved.length >= MAX_SAVED) {
        keepStatus.textContent = PICTURE_WORDS.storeFull;
        return false;
      }
      try {
        let body: SavedBody;
        if (picture.kind === "image") {
          let work: DecodedImage = { width: picture.width, height: picture.height, rgba: picture.rgba };
          let png = await tools.encodePng(work);
          // A kept picture is small: a big one is made smaller until its file fits.
          let guard = 0;
          while (png.length > MAX_SAVED_PNG_BYTES && guard++ < 8) {
            const w = Math.max(1, Math.floor(work.width * 0.7));
            const h = Math.max(1, Math.floor(work.height * 0.7));
            work = await tools.scale(work, w, h);
            png = await tools.encodePng(work);
          }
          const text64 = toBase64(png);
          if (png.length > MAX_SAVED_PNG_BYTES || base64Bytes(text64) > MAX_SAVED_PNG_BYTES) {
            keepStatus.textContent = "This picture is too big to keep. Choose a smaller one.";
            return false;
          }
          body = { kind: "image", width: work.width, height: work.height, png: text64 };
        } else {
          body = bodyOf(picture);
        }
        const kept = await tools.store.add(name, body);
        saved = [...saved, kept];
        keepStatus.textContent = "";
        return true;
      } catch (error) {
        const code = (error as { code?: string } | null)?.code;
        keepStatus.textContent =
          code === "signature_store_full" ? PICTURE_WORDS.storeFull : "The signature could not be saved on this computer.";
        return false;
      }
    };

    form.addEventListener("submit", (e) => {
      e.preventDefault();
      const picture = current();
      if (!picture || go.disabled) return;
      go.disabled = true;
      void keep(picture).then((ok) => {
        if (closed) return;
        if (ok) finish({ picture });
        else go.disabled = current() === null;
      });
    });

    request.layer.append(form);
    request.layer.hidden = false;
    sync();
    paintSurface();
    if (tools.store) {
      void tools.store
        .list()
        .then((list) => {
          saved = list;
          if (!closed && tab === "saved") renderSaved();
        })
        .catch(() => undefined);
    }
    tabButtons.get("draw")?.focus();
  });
}

/** An image scaled to a smaller size, with the canvas of the window. */
export async function scaleImage(image: DecodedImage, width: number, height: number): Promise<DecodedImage> {
  const from = document.createElement("canvas");
  from.width = image.width;
  from.height = image.height;
  from.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(image.rgba), image.width, image.height), 0, 0);
  const to = document.createElement("canvas");
  to.width = width;
  to.height = height;
  const ctx = to.getContext("2d", { willReadFrequently: true });
  if (!ctx) throw new Error("no canvas");
  ctx.drawImage(from, 0, 0, width, height);
  return { width, height, rgba: ctx.getImageData(0, 0, width, height).data };
}

// ---- the tools of the real window --------------------------------------------------------------

/** Reads an image file into pixels with the window's own image decoder, no bigger than the limits. */
export async function decodeImage(blob: Blob): Promise<DecodedImage> {
  const bitmap = await createImageBitmap(blob);
  try {
    const size = fitImageSize(bitmap.width, bitmap.height);
    if (!size) throw new Error("not a picture");
    const canvas = document.createElement("canvas");
    canvas.width = size.width;
    canvas.height = size.height;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) throw new Error("no canvas");
    ctx.drawImage(bitmap, 0, 0, size.width, size.height);
    return { width: size.width, height: size.height, rgba: ctx.getImageData(0, 0, size.width, size.height).data };
  } finally {
    bitmap.close();
  }
}

/** Writes pixels as a PNG with the window's own encoder. */
export async function encodePng(image: DecodedImage): Promise<Uint8Array> {
  const canvas = document.createElement("canvas");
  canvas.width = image.width;
  canvas.height = image.height;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no canvas");
  ctx.putImageData(new ImageData(new Uint8ClampedArray(image.rgba), image.width, image.height), 0, 0);
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  if (!blob) throw new Error("no PNG");
  return new Uint8Array(await blob.arrayBuffer());
}

/** Width over height of a text set in one of the bundled fonts, measured on a canvas once the font is loaded. */
export async function measureTyped(text: string, font: FontId): Promise<number | null> {
  const family = fontInfo(font).family;
  try {
    await document.fonts.load(`100px "${family}"`, text);
  } catch {
    return null;
  }
  const ctx = document.createElement("canvas").getContext("2d");
  if (!ctx) return null;
  ctx.font = `100px "${family}"`;
  const width = ctx.measureText(text).width;
  // The line of script fonts is about as tall as the letters are high: the engine fits the picture
  // to the real outlines, so this only has to be near.
  return width > 0 ? width / 100 / 0.9 : null;
}

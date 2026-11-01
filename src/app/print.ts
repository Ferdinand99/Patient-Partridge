// Printing.
//
// The pages to print are drawn by the engine at print size, one after the other, packed as JPEG
// pictures and put in a hidden element that only shows when the page is printed (see `.print-root`
// in app.css). Then the system's print dialog is opened. Nothing leaves the computer except what
// the user sends to a printer.
//
// A page is a picture of the page, not text: the print is exactly what is on the screen. Pages are
// prepared one at a time and the bitmap of each is dropped as soon as it is packed, so memory holds
// only the packed pictures (about 150 to 400 KB per page).

import type { PageSizePt } from "../viewer/layout";

/** Resolution of a printed page, in dots per inch. */
export const PRINT_DPI = 150;
/** Most pages in one print job. */
export const MAX_PRINT_PAGES = 1000;
/** Largest bitmap side, in pixels: the engine allows 16 384. */
const MAX_SIDE = 6000;
/** Largest bitmap, in pixels. */
const MAX_PIXELS = 20_000_000;

export type RangeResult = { pages: number[] } | { error: string };

/**
 * Reads a list of pages such as "1-3, 7, 9-" (1-based) into page indexes, counted from 0, in
 * ascending order and without repeats. An open end means "to the last page", an open start "from
 * the first".
 */
export function parsePageRange(text: string, pageCount: number): RangeResult {
  const cleaned = text.trim();
  if (cleaned === "") return { error: "Type the pages to print, such as 1-3, 7." };
  const wanted = new Set<number>();
  for (const part of cleaned.split(/[,;\s]+/)) {
    if (part === "") continue;
    const match = /^(\d*)(-?)(\d*)$/.exec(part);
    if (!match || (match[1] === "" && match[3] === "")) {
      return { error: `“${part}” is not a page or a range of pages.` };
    }
    const [, from, dash, to] = match;
    const first = from === "" ? 1 : Number(from);
    const last = dash === "" ? first : to === "" ? pageCount : Number(to);
    if (first < 1 || last < 1) return { error: "Pages are counted from 1." };
    if (first > pageCount || last > pageCount) {
      return { error: `The document has ${pageCount} ${pageCount === 1 ? "page" : "pages"}.` };
    }
    if (last < first) return { error: `“${part}” goes backwards.` };
    for (let p = first; p <= last; p++) wanted.add(p - 1);
    if (wanted.size > MAX_PRINT_PAGES) {
      return { error: `At most ${MAX_PRINT_PAGES} pages can be printed at once.` };
    }
  }
  return { pages: [...wanted].sort((a, b) => a - b) };
}

/** The bitmap to ask the engine for: the page at the print resolution, within the limits. */
export function printBitmapSize(page: PageSizePt, dpi = PRINT_DPI): { width: number; height: number } {
  let width = Math.max(1, Math.round((page.width / 72) * dpi));
  let height = Math.max(1, Math.round((page.height / 72) * dpi));
  const shrink = Math.min(1, MAX_SIDE / Math.max(width, height), Math.sqrt(MAX_PIXELS / (width * height)));
  if (shrink < 1) {
    width = Math.max(1, Math.floor(width * shrink));
    height = Math.max(1, Math.floor(height * shrink));
  }
  return { width, height };
}

/** Packs a bitmap of RGBA bytes into a JPEG picture. */
export function encodeJpeg(rgba: ArrayBuffer, width: number, height: number): Promise<Blob> {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return Promise.reject(new Error("The canvas is not available."));
  ctx.putImageData(new ImageData(new Uint8ClampedArray(rgba), width, height), 0, 0);
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) => {
        // Free the bitmap at once instead of when the garbage collector gets to it.
        canvas.width = 0;
        canvas.height = 0;
        if (blob) resolve(blob);
        else reject(new Error("The page could not be packed."));
      },
      "image/jpeg",
      0.92,
    );
  });
}

export interface PrintDeps {
  render(doc: number, page: number, width: number, height: number): Promise<ArrayBuffer>;
  encode(rgba: ArrayBuffer, width: number, height: number): Promise<Blob>;
  createUrl(blob: Blob): string;
  revokeUrl(url: string): void;
  /** Opens the system's print dialog. */
  print(): Promise<void>;
}

export type PrintOutcome = "printing" | "cancelled";

export interface PrintProgress {
  /** Pages prepared so far, and how many there will be. */
  done: number;
  total: number;
}

export class PrintJob {
  private urls: string[] = [];
  private cancelled = false;
  private afterPrint: (() => void) | null = null;

  constructor(
    private readonly root: HTMLElement,
    private readonly deps: PrintDeps,
  ) {}

  /** Stops preparing pages. The pictures made so far are dropped. */
  cancel(): void {
    this.cancelled = true;
  }

  /** Throws away every picture of this job and of the one before. */
  cleanup(): void {
    if (this.afterPrint) {
      window.removeEventListener("afterprint", this.afterPrint);
      this.afterPrint = null;
    }
    for (const url of this.urls) this.deps.revokeUrl(url);
    this.urls = [];
    this.root.replaceChildren();
  }

  /**
   * Prepares the pages and opens the print dialog. The pictures are kept until printing is over
   * (the browser's `afterprint`), or until the next job starts. Rejects when a page cannot be drawn.
   */
  async run(
    doc: number,
    pages: PageSizePt[],
    indices: number[],
    onProgress: (p: PrintProgress) => void,
  ): Promise<PrintOutcome> {
    this.cleanup();
    this.cancelled = false;
    onProgress({ done: 0, total: indices.length });
    try {
      for (let i = 0; i < indices.length; i++) {
        if (this.cancelled) {
          this.cleanup();
          return "cancelled";
        }
        const index = indices[i];
        const size = printBitmapSize(pages[index]);
        const rgba = await this.deps.render(doc, index, size.width, size.height);
        if (rgba.byteLength !== size.width * size.height * 4) {
          throw new Error("The engine returned a bitmap of the wrong size.");
        }
        const blob = await this.deps.encode(rgba, size.width, size.height);
        if (this.cancelled) {
          this.cleanup();
          return "cancelled";
        }
        const url = this.deps.createUrl(blob);
        this.urls.push(url);
        const img = document.createElement("img");
        img.className = "print-page";
        img.alt = `Page ${index + 1}`;
        img.src = url;
        this.root.append(img);
        onProgress({ done: i + 1, total: indices.length });
      }
    } catch (error) {
      this.cleanup();
      throw error;
    }
    if (this.cancelled) {
      this.cleanup();
      return "cancelled";
    }
    await Promise.all([...this.root.querySelectorAll("img")].map((img) => img.decode?.().catch(() => undefined)));
    this.afterPrint = () => this.cleanup();
    window.addEventListener("afterprint", this.afterPrint, { once: true });
    try {
      await this.deps.print();
    } catch (error) {
      this.cleanup();
      throw error;
    }
    return "printing";
  }
}

/** Number of pages, for the sentence in the dialog. */
function pagesWord(count: number): string {
  return count === 1 ? "1 page" : `${count} pages`;
}

export interface PrintChoice {
  pages: number[];
}

/** The question before printing: all pages, the page in view, or a list of pages. */
export function showPrintDialog(
  layer: HTMLElement,
  options: { name: string; pageCount: number; currentPage: number },
): Promise<PrintChoice | null> {
  return new Promise((resolve) => {
    const previousFocus = document.activeElement as HTMLElement | null;
    const box = document.createElement("form");
    box.className = "sheet notice print-dialog";
    box.setAttribute("role", "dialog");
    box.setAttribute("aria-modal", "true");
    box.setAttribute("aria-labelledby", "print-title");

    const title = document.createElement("h2");
    title.className = "sheet-title";
    title.id = "print-title";
    title.textContent = "Print";
    const detail = document.createElement("p");
    detail.className = "sheet-text";
    detail.textContent = `${options.name} has ${pagesWord(options.pageCount)}. The pages are prepared at ${PRINT_DPI} dpi and then the print dialog of your system opens.`;
    box.append(title, detail);

    const group = document.createElement("div");
    group.className = "print-choices";
    group.setAttribute("role", "radiogroup");
    group.setAttribute("aria-label", "Pages to print");
    const choices: { value: "all" | "current" | "range"; label: string }[] = [
      { value: "all", label: `All ${pagesWord(options.pageCount)}` },
      { value: "current", label: `This page (page ${options.currentPage + 1})` },
      { value: "range", label: "Pages" },
    ];
    const radios = new Map<string, HTMLInputElement>();
    for (const choice of choices) {
      const label = document.createElement("label");
      label.className = "check";
      const radio = document.createElement("input");
      radio.type = "radio";
      radio.name = "print-pages";
      radio.value = choice.value;
      radio.checked = choice.value === "all";
      radios.set(choice.value, radio);
      const text = document.createElement("span");
      text.textContent = choice.label;
      label.append(radio, text);
      group.append(label);
    }
    const range = document.createElement("input");
    range.type = "text";
    range.className = "field-input sheet-field";
    range.placeholder = "1-3, 7, 9-";
    range.setAttribute("aria-label", "Pages to print, such as 1-3, 7");
    range.autocomplete = "off";
    range.spellcheck = false;
    range.disabled = true;
    const problem = document.createElement("p");
    problem.className = "sheet-error";
    problem.setAttribute("role", "alert");
    box.append(group, range, problem);

    const updateRange = () => {
      const on = radios.get("range")?.checked === true;
      range.disabled = !on;
      problem.textContent = "";
      if (on) range.focus();
    };
    for (const radio of radios.values()) radio.addEventListener("change", updateRange);

    const foot = document.createElement("div");
    foot.className = "sheet-foot";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "btn btn-secondary";
    cancel.textContent = "Cancel";
    const print = document.createElement("button");
    print.type = "submit";
    print.className = "btn btn-primary";
    print.textContent = "Print";
    foot.append(cancel, print);
    box.append(foot);

    const finish = (choice: PrintChoice | null) => {
      layer.removeEventListener("keydown", onKey);
      box.remove();
      layer.hidden = layer.childElementCount === 0;
      previousFocus?.focus?.();
      resolve(choice);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && layer.lastElementChild === box) {
        e.preventDefault();
        e.stopPropagation();
        finish(null);
      }
    };
    cancel.addEventListener("click", () => finish(null));
    box.addEventListener("submit", (e) => {
      e.preventDefault();
      const which = [...radios.values()].find((r) => r.checked)?.value ?? "all";
      if (which === "all") {
        finish({ pages: Array.from({ length: options.pageCount }, (_, i) => i).slice(0, MAX_PRINT_PAGES) });
      } else if (which === "current") {
        finish({ pages: [options.currentPage] });
      } else {
        const parsed = parsePageRange(range.value, options.pageCount);
        if ("error" in parsed) {
          problem.textContent = parsed.error;
          range.setAttribute("aria-invalid", "true");
          range.focus();
          return;
        }
        finish({ pages: parsed.pages });
      }
    });
    layer.addEventListener("keydown", onKey);
    layer.append(box);
    layer.hidden = false;
    radios.get("all")?.focus();
  });
}

/** The box shown while pages are prepared: how far it is, and a Cancel button. */
export function showPrintProgress(
  layer: HTMLElement,
  onCancel: () => void,
): { update(p: PrintProgress): void; close(): void } {
  const box = document.createElement("div");
  box.className = "sheet notice";
  box.setAttribute("role", "alertdialog");
  box.setAttribute("aria-modal", "true");
  box.setAttribute("aria-labelledby", "print-progress-title");
  const title = document.createElement("h2");
  title.className = "sheet-title";
  title.id = "print-progress-title";
  title.textContent = "Preparing the pages";
  const text = document.createElement("p");
  text.className = "sheet-text num";
  text.setAttribute("role", "status");
  const bar = document.createElement("progress");
  bar.className = "print-progress";
  bar.max = 1;
  bar.value = 0;
  const foot = document.createElement("div");
  foot.className = "sheet-foot";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "btn btn-secondary";
  cancel.textContent = "Cancel";
  cancel.addEventListener("click", onCancel);
  foot.append(cancel);
  box.append(title, text, bar, foot);
  const previousFocus = document.activeElement as HTMLElement | null;
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && layer.lastElementChild === box) {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
    }
  };
  layer.addEventListener("keydown", onKey);
  layer.append(box);
  layer.hidden = false;
  cancel.focus();
  return {
    update(p) {
      text.textContent = `Page ${Math.min(p.done + 1, p.total)} of ${p.total}`;
      bar.max = Math.max(1, p.total);
      bar.value = p.done;
    },
    close() {
      layer.removeEventListener("keydown", onKey);
      box.remove();
      layer.hidden = layer.childElementCount === 0;
      previousFocus?.focus?.();
    },
  };
}

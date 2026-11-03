// The commands of the page tools in the app process (src-tauri/src/pages.rs). The window names pages
// and numbers; the app process does the rest. The window never names a file to write.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { PageSizePt } from "../viewer/layout";
import type { DesignOp, DesignWidgetInfo } from "./formdesign";
import type { SavedFile } from "./api";

/** A page box in the page's own coordinates: left, bottom, right, top, in points. */
export type PageBox = [number, number, number, number];

/** The document after a change. */
export interface PagesChanged {
  /** The size of every page now, as it is drawn. */
  pages: PageSizePt[];
  /** The number of the copy of the document before the change, to go back to it; 0 when there is none. */
  snapshot: number;
  /** How many pages the change touched. */
  count: number;
  /** How many form fields sat on the pages that were deleted. */
  formFields: number;
}

export interface PagesCropped {
  pages: PageSizePt[];
  before: PageBox[];
  after: PageBox[];
}

export interface PagesInserted {
  changed: PagesChanged;
  /** How many pages the other file had. */
  inserted: number;
}

/**
 * What stopping the work did: nothing was going on, the transfer of pages was stopped (the document
 * is as it was), or the engine is in the middle of a change, which cannot be stopped without losing
 * the changes that are not saved, so nothing was done and the change finishes.
 */
export type CancelResult = "idle" | "handover" | "working";

/** Told how many bytes of a document have gone from one engine to the other. */
export type PageProgress = (done: number, total: number) => void;

export interface InsertOptions {
  password?: string;
  /** The file is protected by a password and its pages may join this document without that protection. */
  allowProtected?: boolean;
  /** The file is signed and its pages may come without the signature. */
  allowSigned?: boolean;
}

export interface ExtractOptions {
  /** The document is signed and the new file will not carry the signature. */
  allowSigned?: boolean;
  /** The document is protected by a password and the new file will not be. */
  allowUnprotected?: boolean;
}

export interface PageToolsApi {
  pagesDelete(doc: number, pages: number[]): Promise<PagesChanged>;
  pagesReorder(doc: number, order: number[]): Promise<PagesChanged>;
  pagesRotate(doc: number, pages: number[], turns: number): Promise<PagesChanged>;
  /** `margins` are left, top, right, bottom in points on the page as it is drawn. */
  pagesCrop(doc: number, pages: number[], margins: [number, number, number, number]): Promise<PagesCropped>;
  pagesSetBoxes(doc: number, pages: number[], boxes: PageBox[]): Promise<PagesChanged>;
  pagesInsertBlank(doc: number, at: number, width: number, height: number): Promise<PagesChanged>;
  pagesRestore(doc: number, snapshot: number): Promise<PagesChanged>;
  pagesInsertPdf(
    doc: number,
    at: number,
    path: string,
    options: InsertOptions,
    onProgress: PageProgress,
  ): Promise<PagesInserted>;
  /** Asks where to write the new file (the app opens the dialog); null when the dialog is cancelled. */
  pagesExtract(
    doc: number,
    pages: number[],
    options: ExtractOptions,
    onProgress: PageProgress,
  ): Promise<SavedFile | null>;
  pagesCancel(doc: number): Promise<CancelResult>;
  /** The form designer: whether the document can take fields, and the names of the fields it has. */
  designCheck?(doc: number): Promise<string[]>;
  /** The widgets of a page that the designer lists. */
  designList?(doc: number, page: number): Promise<DesignWidgetInfo[]>;
  /** Makes a batch of changes with one reload of the document. */
  designApply?(doc: number, ops: DesignOp[]): Promise<PagesChanged>;
}

interface ChangedWire {
  pages: [number, number][];
  snapshot: number;
  count: number;
  formFields: number;
}

function sizes(wire: [number, number][]): PageSizePt[] {
  return wire.map(([width, height]) => ({ width, height }));
}

function changed(wire: ChangedWire): PagesChanged {
  return { pages: sizes(wire.pages), snapshot: wire.snapshot, count: wire.count, formFields: wire.formFields };
}

function progress(onProgress: PageProgress): Channel<[number, number]> {
  const channel = new Channel<[number, number]>();
  channel.onmessage = ([done, total]) => onProgress(done, total);
  return channel;
}

export const pagesApi: PageToolsApi = {
  async pagesDelete(doc, pages) {
    return changed(await invoke<ChangedWire>("pages_delete", { doc, pages }));
  },
  async pagesReorder(doc, order) {
    return changed(await invoke<ChangedWire>("pages_reorder", { doc, order }));
  },
  async pagesRotate(doc, pages, turns) {
    return changed(await invoke<ChangedWire>("pages_rotate", { doc, pages, turns }));
  },
  async pagesCrop(doc, pages, margins) {
    const wire = await invoke<{ pages: [number, number][]; before: PageBox[]; after: PageBox[] }>("pages_crop", {
      doc,
      pages,
      margins,
    });
    return { pages: sizes(wire.pages), before: wire.before, after: wire.after };
  },
  async pagesSetBoxes(doc, pages, boxes) {
    return changed(await invoke<ChangedWire>("pages_set_boxes", { doc, pages, boxes }));
  },
  async pagesInsertBlank(doc, at, width, height) {
    return changed(await invoke<ChangedWire>("pages_insert_blank", { doc, at, width, height }));
  },
  async pagesRestore(doc, snapshot) {
    return changed(await invoke<ChangedWire>("pages_restore", { doc, snapshot }));
  },
  async pagesInsertPdf(doc, at, path, options, onProgress) {
    const wire = await invoke<{ changed: ChangedWire; inserted: number }>("pages_insert_pdf", {
      doc,
      at,
      path,
      password: options.password ?? null,
      allowProtected: options.allowProtected ?? false,
      allowSigned: options.allowSigned ?? false,
      progress: progress(onProgress),
    });
    return { changed: changed(wire.changed), inserted: wire.inserted };
  },
  pagesExtract(doc, pages, options, onProgress) {
    return invoke<SavedFile | null>("pages_extract", {
      doc,
      pages,
      allowSigned: options.allowSigned ?? false,
      allowUnprotected: options.allowUnprotected ?? false,
      progress: progress(onProgress),
    });
  },
  pagesCancel(doc) {
    return invoke<CancelResult>("pages_cancel", { doc });
  },
  designCheck(doc) {
    return invoke<string[]>("author_check", { doc });
  },
  designList(doc, page) {
    return invoke<DesignWidgetInfo[]>("author_list", { doc, page });
  },
  async designApply(doc, ops) {
    return changed(await invoke<ChangedWire>("author_apply", { doc, ops }));
  },
};

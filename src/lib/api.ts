// The commands the app process offers to the window. Everything about PDFs goes through here.

import { invoke } from "@tauri-apps/api/core";
import type { PageSizePt } from "../viewer/layout";

export interface OpenedDocument {
  doc: number;
  pages: PageSizePt[];
  name: string;
}

interface OpenedWire {
  doc: number;
  pages: [number, number][];
  name: string;
}

export const api = {
  async open(path: string, password?: string): Promise<OpenedDocument> {
    const wire = await invoke<OpenedWire>("open_pdf", { path, password: password ?? null });
    return {
      doc: wire.doc,
      name: wire.name,
      pages: wire.pages.map(([width, height]) => ({ width, height })),
    };
  },

  /** A page as raw RGBA bytes: `width * height * 4` of them. */
  render(doc: number, page: number, width: number, height: number): Promise<ArrayBuffer> {
    return invoke<ArrayBuffer>("render_page", { doc, page, width, height });
  },

  async close(doc: number): Promise<void> {
    await invoke<void>("close_pdf", { doc });
  },

  /** Paths the app was started with. Also tells the app that the window is listening now. */
  takePendingPaths(): Promise<string[]> {
    return invoke<string[]>("take_pending_paths");
  },

  measureMode(): Promise<boolean> {
    return invoke<boolean>("measure_mode");
  },

  measureReport(openMs: number, firstPageMs: number, pages: number): Promise<void> {
    return invoke<void>("measure_report", { openMs, firstPageMs, pages });
  },
};

// The commands the app process offers to the window. Everything about PDFs goes through here.

import { invoke } from "@tauri-apps/api/core";
import type { PageSizePt } from "../viewer/layout";

/** Where the reader was in a file, as remembered on this computer. */
export interface SavedPosition {
  page: number;
  /** 0 at the top of the page, 1 at the bottom. */
  offset: number;
  /** The zoom, or null for fit width. */
  zoom: number | null;
}

export interface OpenedDocument {
  doc: number;
  pages: PageSizePt[];
  name: string;
  /** Where this file was left the last time, if it was. */
  position?: SavedPosition | null;
}

interface OpenedWire {
  doc: number;
  pages: [number, number][];
  name: string;
  position: SavedPosition | null;
}

export const api = {
  async open(path: string, password?: string): Promise<OpenedDocument> {
    const wire = await invoke<OpenedWire>("open_pdf", { path, password: password ?? null });
    return {
      doc: wire.doc,
      name: wire.name,
      pages: wire.pages.map(([width, height]) => ({ width, height })),
      position: wire.position,
    };
  },

  /** A page as raw RGBA bytes: `width * height * 4` of them. */
  render(doc: number, page: number, width: number, height: number): Promise<ArrayBuffer> {
    return invoke<ArrayBuffer>("render_page", { doc, page, width, height });
  },

  async close(doc: number): Promise<void> {
    await invoke<void>("close_pdf", { doc });
  },

  /** Remembers where the reader is in a file. Kept in a file on this computer, nowhere else. */
  async setPosition(path: string, position: SavedPosition): Promise<void> {
    await invoke<void>("set_position", {
      path,
      page: position.page,
      offset: position.offset,
      zoom: position.zoom,
    });
  },

  /** Forgets every remembered position. */
  async clearPositions(): Promise<void> {
    await invoke<void>("clear_positions");
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

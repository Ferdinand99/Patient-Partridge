// The commands the app process offers to the window. Everything about PDFs goes through here.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { PageSizePt } from "../viewer/layout";
import type { PageLinkInfo } from "../viewer/overlays";

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

/** One entry of the table of contents, in reading order. */
export interface OutlineEntry {
  depth: number;
  /** The page the entry leads to, counted from 0, or null when it leads nowhere in the document. */
  page: number | null;
  title: string;
}

/** What the app process says about updates. */
export type InstallMethod = "automatic" | "manual";

export interface UpdateSettings {
  /** Whether the app checks for updates by itself. On unless the user turned it off. */
  autoUpdate: boolean;
  /** Whether this copy can install an update by itself, or the user has to do it by hand (deb). */
  method: InstallMethod;
  currentVersion: string;
}

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  /** The release notes. Text from the network: show it as text only. */
  notes: string;
  method: InstallMethod;
}

export type UpdateFailureKind =
  | "offline"
  | "invalidSignature"
  | "notConfigured"
  | "badManifest"
  | "disabled"
  | "busy"
  | "manual"
  | "other";

export interface UpdateFailure {
  kind: UpdateFailureKind;
  message: string;
  /** The engines were stopped for the install and it then failed: open documents are gone. */
  enginesStopped: boolean;
}

export type UpdateEvent =
  | { event: "progress"; chunk: number; total: number | null }
  | { event: "installing" };

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

  /** The table of contents of a document. Empty when it has none. */
  getOutline(doc: number): Promise<OutlineEntry[]> {
    return invoke<OutlineEntry[]>("get_outline", { doc });
  },

  /**
   * The text of a page. `rects` holds four numbers for every character of `text`, in order: left,
   * top, right and bottom in points from the top left of the page as it is drawn.
   */
  getPageText(doc: number, page: number): Promise<{ text: string; rects: number[] }> {
    return invoke<{ text: string; rects: number[] }>("get_page_text", { doc, page });
  },

  /** The links of a page that lead to a web address or to a page of the document. */
  getPageLinks(doc: number, page: number): Promise<PageLinkInfo[]> {
    return invoke<PageLinkInfo[]>("get_page_links", { doc, page });
  },

  /** Finds a text on one page. Each match is `[start, length]` in characters of the page text. */
  searchPage(doc: number, page: number, query: string): Promise<[number, number][]> {
    return invoke<[number, number][]>("search_page", { doc, page, query });
  },

  /**
   * Opens a web address of a document in the user's browser. Call this only after the user said
   * yes. The Rust side checks it again: http and https only, and only an address of that document.
   */
  async openLink(doc: number, url: string): Promise<void> {
    await invoke<void>("open_link", { doc, url });
  },

  /** Opens the print dialog of the web view. Only needed on macOS: elsewhere `window.print()` works. */
  async printWebview(): Promise<void> {
    await invoke<void>("print_webview");
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

  /** Tells the app process about an error in the window. It is written to the app's error stream. */
  frontendError(message: string): Promise<void> {
    return invoke<void>("frontend_error", { message });
  },

  measureMode(): Promise<boolean> {
    return invoke<boolean>("measure_mode");
  },

  measureReport(openMs: number, firstPageMs: number, pages: number): Promise<void> {
    return invoke<void>("measure_report", { openMs, firstPageMs, pages });
  },

  getUpdateSettings(): Promise<UpdateSettings> {
    return invoke<UpdateSettings>("get_update_settings");
  },

  async setAutoUpdate(enabled: boolean): Promise<void> {
    await invoke<void>("set_auto_update", { enabled });
  },

  /**
   * Asks the update server whether a newer version exists: null when there is none. The only
   * network use of the app. An automatic check (`manual` false) is refused while the setting is off.
   */
  checkUpdate(manual: boolean): Promise<UpdateInfo | null> {
    return invoke<UpdateInfo | null>("check_update", { manual });
  },

  /**
   * Downloads and installs the update the last check found. `paths` are the open documents, to be
   * opened again after the restart. On Windows the app ends when the installer starts.
   */
  async installUpdate(paths: string[], onEvent: (event: UpdateEvent) => void): Promise<void> {
    const channel = new Channel<UpdateEvent>();
    channel.onmessage = onEvent;
    await invoke<void>("install_update", { paths, onEvent: channel });
  },

  /** Starts the app again (after an update). */
  async restartApp(): Promise<void> {
    await invoke<void>("restart_app");
  },

  /** Opens the page with the releases of the app in the browser. The address is fixed in the app. */
  async openReleasesPage(): Promise<void> {
    await invoke<void>("open_releases_page");
  },
};

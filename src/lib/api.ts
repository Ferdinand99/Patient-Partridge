// The commands the app process offers to the window. Everything about PDFs goes through here.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { PageSizePt } from "../viewer/layout";
import type { FormFieldInfo, FormInfo, FormValueIn } from "../viewer/forms";
import type { PageLinkInfo } from "../viewer/overlays";
import type { Annotation, AnnotKind, Rgba, StampName } from "./annotations";

/** Where the reader was in a file, as remembered on this computer. */
export interface SavedPosition {
  page: number;
  /** 0 at the top of the page, 1 at the bottom. */
  offset: number;
  /** The zoom, or null for fit width. */
  zoom: number | null;
}

/** The tabs of the last run: the paths in tab order, and which one was in view. */
export interface SavedTabs {
  paths: string[];
  /** Index into `paths` of the tab that was in view, or null. */
  active: number | null;
  /** The paths (a part of `paths`) that could not be reached at the last start. */
  failed?: string[];
}

/** The file a save wrote. */
export interface SavedFile {
  path: string;
  name: string;
  /** The original was touched while it was open, with parts of it unread: this copy holds what was read. */
  touched?: boolean;
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
  | "unsaved"
  | "manual"
  | "location"
  | "other";

export interface UpdateFailure {
  kind: UpdateFailureKind;
  message: string;
  /** The engines were stopped for the install and it then failed: open documents are gone. */
  enginesStopped: boolean;
}

export type UpdateEvent = { event: "progress"; chunk: number; total: number | null };

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

  /**
   * Tells the app whether a document has changes that are not saved. The app then holds back
   * closing, quitting and installing an update until the user has decided.
   */
  async setDirty(doc: number, dirty: boolean): Promise<void> {
    await invoke<void>("set_dirty", { doc, dirty });
  },

  /**
   * Saves a document to the file it was opened from. The app writes a new file next to it, checks
   * it, and replaces the old one in one step; the old file is as it was if anything fails. The
   * answers to the two questions a save can ask come in as flags: the file was changed by
   * something else since it was opened (`overwriteChanged`), and the document is signed and a
   * save breaks the signature (`allowSigned`). Without them the save fails with `save_changed`
   * or `save_signed`.
   */
  save(doc: number, overwriteChanged: boolean, allowSigned: boolean): Promise<SavedFile> {
    return invoke<SavedFile>("save_pdf", { doc, overwriteChanged, allowSigned });
  },

  /**
   * Asks where to save a copy (the app opens the dialog of the system itself) and saves the
   * document there; the document belongs to the new file afterwards. Null when the dialog is
   * cancelled.
   */
  saveAs(doc: number, allowSigned: boolean): Promise<SavedFile | null> {
    return invoke<SavedFile | null>("save_pdf_as", { doc, allowSigned });
  },

  /** Quits the app, after the user decided what happens to the changes that are not saved. */
  async quitApp(): Promise<void> {
    await invoke<void>("quit_app");
  },

  /** The user answered "stay" to the question before closing: the next request to close asks again. */
  async stayOpen(): Promise<void> {
    await invoke<void>("stay_open");
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

  /**
   * The annotations of a page that can be shown and changed. Their text and author come from the
   * file: show them as text only.
   */
  listAnnotations(doc: number, page: number): Promise<Annotation[]> {
    return invoke<Annotation[]>("list_annotations", { doc, page });
  },

  /** Marks the characters `start..end` of the page text (numbered as `getPageText` numbers them). */
  addMarkup(
    doc: number,
    page: number,
    kind: Extract<AnnotKind, "highlight" | "underline" | "strikeout">,
    color: Rgba,
    start: number,
    end: number,
  ): Promise<Annotation> {
    return invoke<Annotation>("add_markup", { doc, page, kind, color, start, end });
  },

  /** A sticky note at a place of the page, in points from its top left as it is drawn. */
  addNote(doc: number, page: number, x: number, y: number, color: Rgba, text: string): Promise<Annotation> {
    return invoke<Annotation>("add_note", { doc, page, x, y, color, text });
  },

  /** A free drawing: strokes of points in points from the top left of the page as it is drawn. */
  addInk(doc: number, page: number, color: Rgba, width: number, strokes: [number, number][][]): Promise<Annotation> {
    return invoke<Annotation>("add_ink", { doc, page, color, width, strokes });
  },

  addStamp(doc: number, page: number, x: number, y: number, stamp: StampName): Promise<Annotation> {
    return invoke<Annotation>("add_stamp", { doc, page, x, y, stamp });
  },

  /** Changes the text, the colour or the place (`dx`, `dy` in points on the drawn page). */
  updateAnnotation(
    doc: number,
    page: number,
    id: number,
    change: { text?: string; color?: Rgba; dx?: number; dy?: number },
  ): Promise<Annotation> {
    return invoke<Annotation>("update_annotation", {
      doc,
      page,
      id,
      text: change.text ?? null,
      color: change.color ?? null,
      dx: change.dx ?? 0,
      dy: change.dy ?? 0,
    });
  },

  async removeAnnotation(doc: number, page: number, id: number): Promise<void> {
    await invoke<void>("remove_annotation", { doc, page, id });
  },

  /** Puts back an annotation that was removed. It keeps its id. */
  restoreAnnotation(doc: number, page: number, id: number): Promise<Annotation> {
    return invoke<Annotation>("restore_annotation", { doc, page, id });
  },

  /**
   * The scripts for which the engine of a document was asked for a font and had none to give
   * ("japanese", "korean", "chinese-simplified", ...): text in them that the PDF does not carry a
   * font for is drawn as blanks.
   */
  missedFonts(doc: number): Promise<string[]> {
    return invoke<string[]>("missed_fonts", { doc });
  },

  /** The links of a page that lead to a web address or to a page of the document. */
  getPageLinks(doc: number, page: number): Promise<PageLinkInfo[]> {
    return invoke<PageLinkInfo[]>("get_page_links", { doc, page });
  },

  /** What kind of form a document has, and whether it has scripts or actions that are never run. */
  formInfo(doc: number): Promise<FormInfo> {
    return invoke<FormInfo>("form_info", { doc });
  },

  /** The fields of a page that can be filled in. Names, tooltips and values are text from the PDF. */
  getFormFields(doc: number, page: number): Promise<FormFieldInfo[]> {
    return invoke<FormFieldInfo[]>("get_form_fields", { doc, page });
  },

  /**
   * Sets one field and returns the fields of the page as they are then. `index` and `name` say
   * which field; the app refuses the request when the widget at that place is not that field.
   */
  setFormField(
    doc: number,
    page: number,
    index: number,
    name: string,
    value: FormValueIn,
  ): Promise<FormFieldInfo[]> {
    return invoke<FormFieldInfo[]>("set_form_field", { doc, page, index, name, value });
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

  /** Whether the tabs of the last run are reopened at the start. */
  getReopenTabs(): Promise<boolean> {
    return invoke<boolean>("get_reopen_tabs");
  },

  /** Turns "Reopen tabs at start" on or off. Off deletes the list of tabs at once. */
  async setReopenTabs(enabled: boolean): Promise<void> {
    await invoke<void>("set_reopen_tabs", { enabled });
  },

  /** Remembers which tabs are open (paths only), for the next start. Does nothing while the setting is off. */
  async saveSession(paths: string[], active: number | null, failed: string[]): Promise<void> {
    await invoke<void>("save_session", { paths, active, failed });
  },

  /** The tabs the last run left, once. Empty while the setting is off. */
  takeSession(): Promise<SavedTabs> {
    return invoke<SavedTabs>("take_session");
  },

  /** Forgets every remembered position, and the list of tabs with them. */
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
   * Downloads the update the last check found, with progress, and checks it against the public key.
   * Nothing about the running app changes: the engines and the open files are not touched.
   */
  async downloadUpdate(onEvent: (event: UpdateEvent) => void): Promise<void> {
    const channel = new Channel<UpdateEvent>();
    channel.onmessage = onEvent;
    await invoke<void>("download_update", { onEvent: channel });
  },

  /**
   * Installs the downloaded update: the app stops the engines, writes down the files that are open
   * at that moment (it knows them itself), and replaces the program. `active` is the file in view,
   * which comes back first. On Windows the app ends when the installer starts.
   */
  async installUpdate(active: string | null): Promise<void> {
    await invoke<void>("install_update", { active });
  },

  /** The files that were open before the last update, once. They are opened again without any limit of the file dialog. */
  takeRestoredPaths(): Promise<string[]> {
    return invoke<string[]>("take_restored_paths");
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

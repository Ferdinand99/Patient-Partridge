// The app: the tab strip, the toolbar, opening files, and showing one document at a time.
// Anything that touches the outside world (the engine, the file dialog, the window title, the
// storage) comes in through `AppDeps`, so tests can run the whole flow with fakes.

import type { OpenedDocument, OutlineEntry, SavedFile, SavedPosition } from "../lib/api";
import { PALETTE, PEN_WIDTHS, type Annotation, type Tool } from "../lib/annotations";
import { describeError, fileNameOf, isEngineLost, toAppError, type AppErrorLike } from "../lib/errors";
import { classifyUri, visibleAddress } from "../lib/linkPolicy";
import { parsePageInput } from "../lib/pageInput";
import { describeFontScripts } from "../lib/scripts";
import { MAX_OPEN_AT_ONCE, MAX_TABS, pickSessionTabs, wantedPdfPaths, type SessionTabs } from "../lib/paths";
import {
  THEME_CHOICES,
  applyTheme,
  loadTheme,
  parseTheme,
  saveTheme,
  type StorageLike,
} from "../lib/theme";
import { actionForKey } from "../viewer/keys";
import type { CopyResult, PageLinkInfo } from "../viewer/overlays";
import { DocumentView, type ViewPosition, type ViewerOptions } from "../viewer/viewer";
import { zoomLabel, type ZoomMode } from "../viewer/zoom";
import { FindBar } from "./find";
import { Annotator, CLEAN_REFUSALS, type AnnotatorApi } from "./annotator";
import { AnnotBar } from "./annotbar";
import { EditHistory, type Edit } from "./history";
import { MenuController } from "./menu";
import {
  PrintJob,
  encodeJpeg,
  showPrintDialog,
  showPrintProgress,
  type PrintDeps,
} from "./print";
import { showNotice } from "./notice";
import { Sidebar, type SidebarDocument } from "./sidebar";
import { TabStrip } from "./tabs";

export interface AppDeps {
  api: {
    open(path: string, password?: string): Promise<OpenedDocument>;
    render(doc: number, page: number, width: number, height: number): Promise<ArrayBuffer>;
    close(doc: number): Promise<void>;
    setPosition(path: string, position: SavedPosition): Promise<void>;
    clearPositions(): Promise<void>;
    getReopenTabs(): Promise<boolean>;
    setReopenTabs(enabled: boolean): Promise<void>;
    saveSession(paths: string[], active: number | null, failed: string[]): Promise<void>;
    getOutline(doc: number): Promise<OutlineEntry[]>;
    getPageText(doc: number, page: number): Promise<{ text: string; rects: number[] }>;
    getPageLinks(doc: number, page: number): Promise<PageLinkInfo[]>;
    searchPage(doc: number, page: number, query: string): Promise<[number, number][]>;
    openLink(doc: number, url: string): Promise<void>;
    /** The scripts the engine of a document had no font for when PDFium asked. */
    missedFonts?(doc: number): Promise<string[]>;
    measureMode(): Promise<boolean>;
    measureReport(openMs: number, firstPageMs: number, pages: number): Promise<void>;
    setDirty(doc: number, dirty: boolean): Promise<void>;
    save(doc: number, overwriteChanged: boolean, allowSigned: boolean): Promise<SavedFile>;
    saveAs(doc: number, allowSigned: boolean): Promise<SavedFile | null>;
    quitApp(): Promise<void>;
    stayOpen(): Promise<void>;
  } & Partial<AnnotatorApi> & {
      /** The annotations of a page. Without it, and without the other annotation calls, the annotation bar is off. */
      listAnnotations?(doc: number, page: number): Promise<Annotation[]>;
    };
  /** Shows the file dialog. Resolves with the chosen paths, or an empty list when cancelled. */
  pickFiles(): Promise<string[]>;
  setWindowTitle(title: string): void;
  now(): number;
  /** Where the colour theme choice is kept. Without it the choice lasts until the app closes. */
  storage?: StorageLike | null;
  /** How long the reader must rest before the position is saved, in milliseconds. */
  positionDelayMs?: number;
  /** How long the thumbnail list must rest before thumbnails are asked for, in milliseconds. */
  thumbSettleMs?: number;
  /** How long typing in the find box must rest before the search starts, in milliseconds. */
  findDebounceMs?: number;
  /** What printing needs besides the engine. Without it the browser's own functions are used. */
  print?: Partial<Omit<PrintDeps, "render">>;
  /**
   * How long the question before a link is opened cannot be answered with its primary button, in
   * milliseconds. Default: LINK_ANSWER_DELAY_MS.
   */
  linkAnswerDelayMs?: number;
  /**
   * How long a tab of the last session may take to open at the start, in milliseconds. Default:
   * SESSION_OPEN_LIMIT_MS.
   */
  sessionOpenLimitMs?: number;
  /** Limits on the text kept for each document. For tests; the defaults are meant for the app. */
  viewLimits?: ViewerOptions["limits"];
}

interface Tab {
  id: number;
  doc: number;
  path: string;
  name: string;
  /** What was changed in the document since it was opened, and whether it is saved. */
  history: EditHistory;
  /** What the app process was last told about unsaved changes of this document. */
  dirtyTold: boolean;
  /** Set while the document is being saved; resolves with the answer of that save. */
  saving: Promise<boolean> | null;
  view: DocumentView;
  /** The annotations of the document, or null when the app process offers none. */
  annotator: Annotator | null;
  /** What the side panel knows about this document. */
  sidebarDoc: SidebarDocument;
  saveTimer: ReturnType<typeof setTimeout> | undefined;
  /**
   * Where the reader was when the remembered positions were forgotten. Until the reader moves
   * from there, the place of this file is not written down again.
   */
  forgottenAt: ViewPosition | null;
  /** True once the reader was told that some pages have more text than is kept. */
  limitTold: boolean;
  /**
   * Where the tab belongs among the tabs of the last session (its index there), so that tabs
   * reopened one after the other end up in the old order. Infinity for every other tab, which goes
   * to the end.
   */
  rank: number;
  /** True once a page of the document had text in a script that needs a font. */
  sawScripts: boolean;
  /** True while the engine is being asked which fonts it had to refuse. */
  fontsAsking: boolean;
  /** True once the reader was told about text that cannot be drawn for lack of a font. */
  fontsTold: boolean;
}

/**
 * What happened to one tab of the last session at the start. "unreached" is a file that gave no
 * answer or could not be read: it may be there again next time (a network drive that was off).
 */
type Reopened = "opened" | "locked" | "failed" | "unreached";

/** Errors of opening a file that may go away by themselves. */
const UNREACHED_CODES = new Set(["io", "engine_unavailable", "engine_stopped", "updating", "internal"]);

/** How long a tab of the last session may take to open at the start, in milliseconds. */
export const SESSION_OPEN_LIMIT_MS = 10_000;

/**
 * Failures of a save that a copy gets round: the file itself cannot be replaced (not writable,
 * held open, shared under several names, belonging to someone else, with access rights that
 * cannot be kept), but another file can be written.
 */
const COPY_REMEDY = new Set([
  "save_read_only",
  "save_locked",
  "save_hard_link",
  "save_owner_changes",
  "save_access_kept",
  "save_source_touched",
]);

const ZOOM_CHOICES = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];
const DEFAULT_POSITION_DELAY_MS = 600;
/**
 * How long the question before a web address is opened has its primary button off. A double click
 * on a link raises the question with the first press, and the second press must not answer it.
 * The delay is a little longer than the double-click time of the systems (500 ms).
 */
export const LINK_ANSWER_DELAY_MS = 700;

/** True when two places in a document are the same, as far as the reader can tell. */
function samePlace(a: ViewPosition, b: ViewPosition): boolean {
  const sameZoom = a.zoom === b.zoom || (a.zoom !== null && b.zoom !== null && Math.abs(a.zoom - b.zoom) < 1e-6);
  return a.page === b.page && Math.abs(a.offset - b.offset) < 0.002 && sameZoom;
}

const APP_NAME = "Patient Partridge";
/** How long a save is waited for before a box says that it is going on. */
const SAVE_SHEET_DELAY_MS = 500;
const SAVE_SLOW_DETAIL =
  "A large file or a slow drive can take a while. You can wait, or give up the changes you made: the file on the disk stays as it is. Cancel keeps the program open and lets the save carry on.";

function byId<T extends HTMLElement>(root: Document, id: string): T {
  const el = root.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
}

export class App {
  private tabs: Tab[] = [];
  private activeId: number | null = null;
  private nextTabId = 1;
  /** Paths that are being opened right now, so that one file is not opened twice at once. */
  private readonly opening = new Set<string>();
  private measuring = false;
  private reported = false;
  /** Whether the tabs are remembered for the next start (the setting "Reopen tabs at start"). */
  private reopenTabs = true;
  /** True while the tabs of the last session are being reopened: the list is not written yet. */
  private sessionHeld = false;
  /** What was last sent to be remembered, so that the same list is not sent twice. */
  private lastSession: string | null = null;
  /** Files of the last session that could not be reached at this start: kept in the list once. */
  private kept: string[] = [];
  /** True while the question before the app quits is open, so that it is not asked twice. */
  private quitting = false;
  /** What a second request to quit does while the first is still being dealt with. */
  private quitReveal: (() => void) | null = null;

  private readonly stage: HTMLElement;
  private readonly empty: HTMLElement;
  private readonly dropOverlay: HTMLElement;
  private readonly noticeLayer: HTMLElement;
  private readonly pageInput: HTMLInputElement;
  private readonly pageTotal: HTMLElement;
  private readonly zoomSelect: HTMLSelectElement;
  private readonly zoomIn: HTMLButtonElement;
  private readonly zoomOut: HTMLButtonElement;
  private readonly tabStrip: TabStrip;
  private readonly menu: MenuController;
  private readonly sidebar: Sidebar;
  private readonly find: FindBar;
  private readonly selectionProxy: HTMLElement;
  private readonly annotbar: AnnotBar;
  private readonly annotStatus: HTMLElement;
  private readonly popovers: HTMLElement;
  /** Whether the annotation bar is shown. */
  private annotOpen = false;
  /** Makes the same sentence be read twice in a row. */
  private announceFlip = false;
  private readonly copyStatus: HTMLElement;
  /** The line of each running copy (copies of different tabs can run together) shows in one place. */
  private readonly copyLines = new Map<DocumentView, string>();
  /** The place of each path in the list of the last session, while that list is being reopened. */
  private sessionRanks = new Map<string, number>();
  private readonly printRoot: HTMLElement;
  private printJob: PrintJob | null = null;
  private printing = false;
  private readonly positionDelay: number;

  constructor(
    private readonly root: Document,
    private readonly deps: AppDeps,
  ) {
    this.stage = byId(root, "stage");
    this.empty = byId(root, "empty");
    this.dropOverlay = byId(root, "drop-overlay");
    this.noticeLayer = byId(root, "notice-layer");
    this.pageInput = byId(root, "page-input");
    this.pageTotal = byId(root, "page-total");
    this.zoomSelect = byId(root, "zoom-select");
    this.zoomIn = byId(root, "zoom-in");
    this.zoomOut = byId(root, "zoom-out");
    this.positionDelay = deps.positionDelayMs ?? DEFAULT_POSITION_DELAY_MS;
    this.tabStrip = new TabStrip(byId(root, "tabs"), {
      select: (id) => this.activate(id),
      close: (id) => void this.requestClose(id),
    });
    this.menu = new MenuController(byId(root, "menu-button"), byId(root, "menu"));
    this.selectionProxy = byId(root, "sel-proxy");
    this.annotStatus = byId(root, "annot-status");
    this.popovers = byId(root, "annot-popovers");
    this.annotbar = new AnnotBar(root, {
      toggle: (open) => this.setAnnotating(open, true),
      setTool: (tool) => this.active?.annotator?.setTool(tool),
      setColor: (color) => this.active?.annotator?.setColor(color),
      setPenWidth: (width) => this.active?.annotator?.setPenWidth(width),
      setStamp: (stamp) => {
        this.active?.annotator?.setStamp(stamp);
        this.active?.view.element.focus({ preventScroll: true });
      },
      editText: () => {
        const a = this.active?.annotator;
        if (a?.selected) void a.editText(a.selected.page, a.selected.id);
      },
      remove: () => void this.active?.annotator?.removeSelected(),
      undo: () => void this.undoActive(),
      redo: () => void this.redoActive(),
    });
    this.copyStatus = byId(root, "copy-status");
    this.printRoot = byId(root, "print-root");
    this.find = new FindBar(root, {
      searchPage: (doc, page, query) => deps.api.searchPage(doc, page, query),
      onEngineLost: (target, error) => {
        const lost = this.tabs.find((t) => t.view === target.view);
        if (lost) void this.documentLost(lost, error);
      },
      debounceMs: deps.findDebounceMs,
    });
    this.sidebar = new Sidebar(root, {
      api: deps.api,
      storage: deps.storage ?? null,
      thumbs: deps.thumbSettleMs === undefined ? undefined : { settleMs: deps.thumbSettleMs },
    });
  }

  init(): void {
    byId(this.root, "open-button").addEventListener("click", () => void this.openDialog());
    byId(this.root, "print-button").addEventListener("click", () => void this.printActive());
    byId(this.root, "empty-open").addEventListener("click", () => void this.openDialog());
    this.zoomIn.addEventListener("click", () => this.active?.view.stepZoom(1));
    this.zoomOut.addEventListener("click", () => this.active?.view.stepZoom(-1));
    this.zoomSelect.addEventListener("change", () => this.onZoomSelect());
    this.pageInput.addEventListener("keydown", (e) => this.onPageInputKey(e));
    this.pageInput.addEventListener("blur", () => this.showCurrentPage());
    this.root.addEventListener("keydown", (e) => this.onKeyDown(e));
    this.root.addEventListener("copy", (e) => this.onCopy(e));
    this.initContextMenu();
    this.initTheme();
    this.initReopenSetting();
    byId(this.root, "forget-positions").addEventListener("click", () => void this.forgetPositions());
    // The reader's place is saved when they rest, and again when the window goes away.
    this.root.addEventListener("visibilitychange", () => {
      if (this.root.visibilityState === "hidden") this.saveAll();
    });
    this.root.defaultView?.addEventListener("pagehide", () => this.saveAll());
    this.buildZoomOptions(null);
    this.syncChrome(false);
    void this.deps.api.measureMode().then((on) => (this.measuring = on));
  }

  /** The document that is shown, if any. */
  get currentView(): DocumentView | null {
    return this.active?.view ?? null;
  }

  /** How many documents are open. */
  get tabCount(): number {
    return this.tabs.length;
  }

  /**
   * The edit history of the document that is shown: a feature that changes a document pushes
   * what it did here, and the tab, the window title and the questions before closing, quitting
   * and updating follow.
   */
  get currentHistory(): EditHistory | null {
    return this.active?.history ?? null;
  }

  /** Records an edit of the document that is shown. Does nothing when there is none. */
  recordEdit(edit: Edit): void {
    this.active?.history.push(edit);
  }

  /**
   * True when a document has changes that are not saved: the history says so, or a change is still
   * being made, or text typed in the box of a note has not been saved yet.
   */
  private isDirty(tab: Tab): boolean {
    return tab.history.dirty || (tab.annotator?.unsaved ?? false);
  }

  /** How many documents have changes that are not saved. */
  get unsavedCount(): number {
    return this.tabs.filter((t) => this.isDirty(t)).length;
  }

  /**
   * The paths of the open documents, the one in view first. The place of the reader in each of them
   * is written down first, because the app is about to be restarted (after an update).
   */
  openDocumentPaths(): string[] {
    this.saveAll();
    const active = this.active;
    const others = this.tabs.filter((t) => t !== active).map((t) => t.path);
    return active ? [active.path, ...others] : others;
  }

  /**
   * Opens the files that were open before an update. They are what the reader had, so the rules
   * for files that are named at the start do not apply: there is no limit of eight, and a file
   * without the ending .pdf is opened too. The first one is shown.
   */
  async restorePaths(paths: string[]): Promise<void> {
    if (paths.length > 0) await this.openPaths(paths);
  }

  /**
   * The start of the app. Opens, in this order: the files that were open before an update (all of
   * them, whatever the setting says), or else the tabs of the last session; then the files named
   * on the command line or sent by the system, which take the focus. The list of tabs is not
   * written until this is done, so that the tabs still to come back are not forgotten.
   */
  async start(from: { restored?: string[]; session?: SessionTabs | null; given?: string[] }): Promise<void> {
    this.sessionHeld = true;
    try {
      const given = from.given ?? [];
      if ((from.restored ?? []).length > 0) {
        await this.restorePaths(from.restored ?? []);
        if (given.length > 0) await this.openChosen(given);
      } else {
        await this.reopenSession(from.session, given);
      }
    } finally {
      this.releaseSession();
    }
  }

  /** The list of tabs may be written from now on, with the tabs that are open at this moment. */
  private releaseSession(): void {
    this.sessionHeld = false;
    this.saveSession();
  }

  /**
   * Reopens the tabs of the last session in their old order. The files named at the start open
   * first and take the focus, and no tab of the last session can hold them back. The tab that was
   * in view comes first among the others and is shown when nothing else is; the rest follow one at
   * a time, so that many tabs do not hold the start back. Nothing asks a question: a file that
   * cannot be opened is skipped, and one notice at the end says how many. A file that does not
   * answer within `sessionOpenLimitMs` counts as one that could not be opened.
   */
  private async reopenSession(saved: SessionTabs | null | undefined, given: string[]): Promise<void> {
    const named = wantedPdfPaths(given).slice(0, MAX_OPEN_AT_ONCE);
    // A named file that is in the list already is one tab, not two: it takes no room of its own,
    // unless the list is cut and leaves it out.
    const savedPaths = new Set(saved?.paths ?? []);
    const extra = named.filter((p) => !savedPaths.has(p)).length;
    // Named files that the cut leaves out are opened as named files, so they take room too. Cutting
    // more can leave out another named file, so the cut is made again until the count stops growing.
    let cutNamed = 0;
    let pick = pickSessionTabs(saved, MAX_TABS - extra);
    for (;;) {
      const now = named.filter((p) => savedPaths.has(p) && !pick.paths.includes(p)).length;
      if (now <= cutNamed) break;
      cutNamed = now;
      pick = pickSessionTabs(saved, MAX_TABS - extra - cutNamed);
    }
    // A named file takes the place it had in the list.
    this.sessionRanks = new Map(pick.paths.map((p, i) => [p, i]));
    const naming = given.length > 0 ? this.openChosen(given) : Promise.resolve();
    try {
      await this.reopenPicked(pick, savedPaths, named, saved, naming);
    } finally {
      this.sessionRanks = new Map();
    }
  }

  private async reopenPicked(
    pick: { paths: string[]; active: number; left: number },
    savedPaths: Set<string>,
    named: string[],
    saved: SessionTabs | null | undefined,
    naming: Promise<void>,
  ): Promise<void> {
    if (pick.paths.length === 0) {
      await naming;
      return;
    }
    // A file that could not be reached gets one more start before it is forgotten: the network
    // may have been down.
    const failedBefore = new Set(saved?.failed ?? []);
    this.kept = [];
    // Named files that the list cut are opened as named files: they are not lost.
    let failed = pick.left - named.filter((p) => savedPaths.has(p) && !pick.paths.includes(p)).length;
    let locked = 0;
    const count = (result: Reopened, path: string) => {
      if (result === "locked") locked++;
      else if (result !== "opened") {
        failed++;
        if (result === "unreached" && !failedBefore.has(path)) this.kept.push(path);
      }
    };
    // The tab that was in view comes first, then the others in their old order.
    const order = [pick.active, ...pick.paths.keys()].filter((v, i, all) => all.indexOf(v) === i);
    for (const i of order) {
      const path = pick.paths[i];
      if (named.includes(path)) continue;
      count(await this.reopen(path, i), path);
    }
    await naming;
    this.tellAboutSession(failed, locked, pick.left > 0);
  }

  /**
   * Opens a file of the last session without asking anything. It is shown only if nothing else is
   * shown by then, so a file named at the start or opened by hand keeps the focus.
   */
  private async reopen(path: string, rank: number): Promise<Reopened> {
    if (this.tabs.some((t) => t.path === path) || this.opening.has(path)) return "opened";
    this.opening.add(path);
    const started = this.deps.now();
    try {
      const opened = await this.openWithin(path);
      if (opened === null) return "unreached";
      if (opened.pages.length === 0) {
        // No dialog per file at the start: it counts in the one notice.
        void this.deps.api.close(opened.doc).catch(() => undefined);
        return "failed";
      }
      this.addTab(opened, path, started, false, rank);
      return "opened";
    } catch (e) {
      const error = toAppError(e);
      // A protected file is not asked for its password at the start: it stays closed.
      if (describeError(error).action === "password") return "locked";
      return UNREACHED_CODES.has(error.code) ? "unreached" : "failed";
    } finally {
      this.opening.delete(path);
    }
  }

  /**
   * Asks the app process to open a file, and gives up on it after `sessionOpenLimitMs`: then the
   * list of tabs is released and null is given. A document that arrives after that is closed.
   */
  private async openWithin(path: string): Promise<OpenedDocument | null> {
    const limit = this.deps.sessionOpenLimitMs ?? SESSION_OPEN_LIMIT_MS;
    const pending = this.deps.api.open(path);
    let late = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const giveUp = new Promise<null>((resolve) => {
      timer = setTimeout(() => {
        late = true;
        this.releaseSession();
        resolve(null);
      }, limit);
    });
    pending.then(
      (doc) => {
        if (late) void this.deps.api.close(doc.doc).catch(() => undefined);
      },
      () => undefined,
    );
    try {
      return await Promise.race([pending, giveUp]);
    } finally {
      clearTimeout(timer);
    }
  }

  /** One calm message about the tabs that did not come back, if there are any. */
  private tellAboutSession(failed: number, locked: number, overLimit: boolean): void {
    if (failed === 0 && locked === 0) return;
    const parts: string[] = [];
    if (failed > 0) {
      const tabs = failed === 1 ? "1 tab" : `${failed} tabs`;
      const files = failed === 1 ? "The file may have" : "The files may have";
      const limit = overLimit ? ` Patient Partridge keeps at most ${MAX_TABS} documents open.` : "";
      parts.push(`${tabs} from your last session could not be opened. ${files} moved or been deleted.${limit}`);
    }
    if (locked > 0) {
      const files = locked === 1 ? "1 password protected file was" : `${locked} password protected files were`;
      parts.push(`${files} left closed. Open ${locked === 1 ? "it" : "them"} yourself to enter the password.`);
    }
    void showNotice(this.noticeLayer, {
      title: "Some tabs were not reopened",
      detail: parts.join(" "),
      buttons: [{ id: "ok", label: "OK", primary: true }],
    });
  }

  /** Closes the settings menu, if it is open. */
  closeMenu(): void {
    this.menu.close();
  }

  setDropActive(active: boolean): void {
    this.dropOverlay.hidden = !active;
  }

  private get active(): Tab | null {
    return this.tabs.find((t) => t.id === this.activeId) ?? null;
  }

  // ---- opening and closing ---------------------------------------------------

  async openDialog(): Promise<void> {
    // The same choice and the same limit as for files that are dropped or named at the start.
    await this.openChosen(await this.deps.pickFiles());
  }

  /**
   * Opens the files chosen in the file dialog, dropped on the window or named at the start. At most
   * `MAX_OPEN_AT_ONCE` PDF files open at once; the reader is told how many were left out.
   */
  async openChosen(paths: string[]): Promise<void> {
    const wanted = wantedPdfPaths(paths);
    const chosen = wanted.slice(0, MAX_OPEN_AT_ONCE);
    if (chosen.length === 0) return;
    const opening = this.openPaths(chosen);
    const left = wanted.length - chosen.length;
    if (left > 0) {
      void showNotice(this.noticeLayer, {
        title: "Not all files were opened",
        detail: `At most ${MAX_OPEN_AT_ONCE} files are opened at once. ${left === 1 ? "1 file was" : `${left} files were`} left out. Open ${left === 1 ? "it" : "them"} again in a smaller group.`,
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
    }
    await opening;
  }

  /** Opens several files. The first one is shown; the others open in tabs behind it. */
  async openPaths(paths: string[]): Promise<void> {
    await Promise.all(paths.map((path, i) => this.openPath(path, undefined, i === 0)));
  }

  /** Opens a file in a tab. If it needs a password the user is asked for one. */
  async openPath(path: string, password?: string, activate = true): Promise<void> {
    const existing = this.tabs.find((t) => t.path === path);
    if (existing) {
      if (activate) this.activate(existing.id);
      return;
    }
    if (this.opening.has(path)) return;
    this.opening.add(path);
    try {
      await this.openNew(path, password, activate);
    } finally {
      this.opening.delete(path);
    }
  }

  /** Opens a file that is not open yet. A question about it (password, try again) stays inside. */
  private async openNew(path: string, password: string | undefined, activate: boolean): Promise<void> {
    const name = fileNameOf(path);
    const started = this.deps.now();
    let opened: OpenedDocument;
    try {
      opened = await this.deps.api.open(path, password);
    } catch (e) {
      await this.openFailed(path, name, toAppError(e), activate);
      return;
    }
    this.addTab(opened, path, started, activate);
  }

  private async openFailed(
    path: string,
    name: string,
    error: AppErrorLike,
    activate: boolean,
  ): Promise<void> {
    const text = describeError(error, name);
    if (text.action === "password") {
      const answer = await showNotice(this.noticeLayer, {
        title: text.title,
        detail: text.detail,
        password: true,
        buttons: [
          { id: "cancel", label: "Cancel" },
          { id: "open", label: "Open", primary: true },
        ],
      });
      if (answer.button === "open" && answer.password) {
        await this.openNew(path, answer.password, activate);
      }
      return;
    }
    if (text.action === "retry") {
      const answer = await showNotice(this.noticeLayer, {
        title: text.title,
        detail: text.detail,
        buttons: [
          { id: "close", label: "Close" },
          { id: "retry", label: "Try again", primary: true },
        ],
      });
      if (answer.button === "retry") await this.openNew(path, undefined, activate);
      return;
    }
    await showNotice(this.noticeLayer, {
      title: text.title,
      detail: text.detail,
      buttons: [{ id: "ok", label: "OK", primary: true }],
    });
  }

  private addTab(
    opened: OpenedDocument,
    path: string,
    started: number,
    activate: boolean,
    rank = Infinity,
  ): void {
    // A file named at the start that was in the last session's list goes back to its old place.
    if (rank === Infinity) rank = this.sessionRanks.get(path) ?? Infinity;
    const openedAfter = this.deps.now() - started;
    const tab: Tab = {
      id: this.nextTabId++,
      doc: opened.doc,
      path,
      name: opened.name,
      history: new EditHistory(),
      dirtyTold: false,
      saving: null,
      view: undefined as unknown as DocumentView,
      annotator: null,
      sidebarDoc: undefined as unknown as SidebarDocument,
      saveTimer: undefined,
      forgottenAt: null,
      limitTold: false,
      rank,
      sawScripts: false,
      fontsAsking: false,
      fontsTold: false,
    };
    tab.view = new DocumentView(
      { doc: opened.doc, pages: opened.pages },
      this.deps.api,
      {
        onPageChange: (page) => {
          if (this.active !== tab) return;
          this.showCurrentPage(page);
          this.sidebar.setCurrentPage(page);
        },
        onZoomChange: (zoom, mode) => {
          if (this.active === tab) this.showZoom(zoom, mode);
        },
        onFatal: (error) => void this.documentLost(tab, error),
        onFirstPaint: () => {
          // Thumbnails wait for this: the page the reader asked for comes first.
          if (this.active === tab) this.sidebar.notifyPainted();
          // The measurement run opens one file; report that one.
          if (!this.measuring || this.reported) return;
          this.reported = true;
          const paintedAfter = this.deps.now() - started;
          void this.deps.api.measureReport(openedAfter, paintedAfter, opened.pages.length);
        },
        onViewChange: () => this.scheduleSave(tab),
        onLink: (link, page) => void this.onLink(tab, link, page),
        onSelectionChange: (has) => {
          if (!has && this.active === tab) this.selectionProxy.textContent = "";
        },
        onSelectionSettled: () => {
          tab.annotator?.selectionSettled();
          this.updateSelectionProxy(tab);
        },
        onTextLimit: () => void this.textLimitHit(tab),
        onScripts: () => {
          tab.sawScripts = true;
          void this.fontsMissed(tab);
        },
        onPageDrawn: () => void this.fontsMissed(tab),
        annotations: {
          onSelect: (page, id) => tab.annotator?.onSelect(page, id),
          onPlace: (page, x, y, client) => tab.annotator?.onPlace(page, x, y, client),
          onStroke: (page, points) => tab.annotator?.onStroke(page, points),
          onMove: (page, id, dx, dy) => tab.annotator?.onMove(page, id, dx, dy),
          onEdit: (page, id) => tab.annotator?.onEdit(page, id),
          onLoadFailed: (page, error) => tab.annotator?.onLoadFailed(page, error),
        },
      },
      { start: opened.position ?? null, limits: this.deps.viewLimits },
    );
    tab.annotator = this.makeAnnotator(tab);
    tab.history.onChange(() => this.historyChanged(tab));
    tab.sidebarDoc = {
      doc: opened.doc,
      pages: opened.pages,
      goToPage: (index) => tab.view.goToPage(index),
      hasPainted: () => tab.view.hasPainted,
      currentPage: () => tab.view.currentPageIndex,
    };
    // Tabs of the last session go where they were; any other tab goes to the end.
    const before = this.tabs.findIndex((t) => t.rank > rank);
    if (before < 0) this.tabs.push(tab);
    else this.tabs.splice(before, 0, tab);
    this.stage.append(tab.view.element);

    const showIt = activate || this.activeId === null;
    if (showIt) {
      this.activate(tab.id);
    } else {
      // Opened behind the one that is shown: it starts when its tab is first selected.
      tab.view.setActive(false);
      this.syncChrome();
    }
    if (opened.pages.length === 0) {
      void showNotice(this.noticeLayer, {
        title: "This PDF has no pages",
        detail: `"${opened.name}" opened, but it does not contain any pages to show.`,
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
    }
  }

  /**
   * A page with text in a script that needs a font was read or drawn. If the PDF carries its fonts
   * nothing is wrong. If it does not, PDFium asks the engine for one, and when the engine has none
   * it can use (this computer has none of the fonts it looks for, one is damaged, or the document
   * names more fonts than the engine hands out) that text is drawn as blanks. The engine remembers
   * which scripts it had to refuse; one calm message, once for the tab, says what that means.
   * Asked again after each page drawn until the reader has been told.
   */
  private async fontsMissed(tab: Tab): Promise<void> {
    if (tab.fontsTold || tab.fontsAsking || !tab.sawScripts || !this.deps.api.missedFonts) return;
    if (!this.tabs.includes(tab)) return;
    tab.fontsAsking = true;
    let missed: string[];
    try {
      missed = await this.deps.api.missedFonts(tab.doc);
    } catch {
      return;
    } finally {
      tab.fontsAsking = false;
    }
    if (missed.length === 0 || tab.fontsTold || !this.tabs.includes(tab)) return;
    tab.fontsTold = true;
    await showNotice(this.noticeLayer, {
      title: "Some text may not show",
      detail: `"${tab.name}" has text in ${describeFontScripts(missed)} that the file does not carry the fonts for, and the reader could not use a font of this computer for it, so that text shows as blanks. You can still search, select and copy it. The reader only looks for a few fonts that come with the system (the list is in the project's documentation). If you install one of them for all users, close this tab and open the file again; a font installed for one user only is not used.`,
      buttons: [{ id: "ok", label: "OK", primary: true }],
    });
  }

  /** A page of a tab has more text or more links than the window may keep. Told once for the tab. */
  private async textLimitHit(tab: Tab): Promise<void> {
    if (tab.limitTold || !this.tabs.includes(tab)) return;
    tab.limitTold = true;
    await showNotice(this.noticeLayer, {
      title: "Some pages have a lot of text",
      detail: `Some pages of "${tab.name}" have more text or links than Patient Partridge keeps in memory at once. They look the same, but on those pages some text may not be selectable, some search matches may not be marked and some links may not be pressable. Select all with Copy, and Find, still reach all of the text. Scrolling on can bring them back.`,
      buttons: [{ id: "ok", label: "OK", primary: true }],
    });
  }

  /** Brings a tab to the front. */
  activate(id: number): void {
    const next = this.tabs.find((t) => t.id === id);
    if (!next) return;
    const previous = this.active;
    if (previous && previous !== next) {
      this.saveNow(previous);
      previous.view.setActive(false);
    }
    this.activeId = id;
    if (!next.view.isActive) {
      // A tab that was behind: it comes back where it was (or starts, if it was never shown).
      next.view.setActive(true);
    } else if (!next.view.hasStarted) {
      // A tab that was just opened: it is active already, but has not been laid out yet.
      next.view.start();
    }
    this.syncChrome();
    next.view.element.focus({ preventScroll: true });
  }

  /**
   * Closes a tab at the user's request. A document with changes that are not saved asks first:
   * save, give the changes up, or keep the tab. Resolves with true when the tab is closed.
   */
  async requestClose(id: number): Promise<boolean> {
    const tab = this.tabs.find((t) => t.id === id);
    if (!tab) return true;
    if (!this.isDirty(tab)) {
      this.closeTab(id);
      return true;
    }
    const answer = await showNotice(this.noticeLayer, {
      title: `Save changes to "${tab.name}"?`,
      detail: "If you close it without saving, the changes you made are lost.",
      buttons: [
        { id: "cancel", label: "Cancel", safe: true },
        { id: "discard", label: "Don't save" },
        { id: "save", label: "Save", primary: true },
      ],
    });
    if (answer.button === "cancel") return false;
    if (answer.button === "save") {
      const ended = await this.waitForSave(() => this.saveTab(tab, false), {
        title: `Saving "${tab.name}"…`,
        detail: SAVE_SLOW_DETAIL,
        leave: "Don't save",
      });
      if (ended === "stay" || ended === "unsaved") return false;
    }
    // The tab may be gone already (its engine stopped while the question was open).
    if (this.tabs.includes(tab)) this.closeTab(tab.id);
    return true;
  }

  /**
   * Waits for a save that the user asked for, and does not leave the window without an answer if
   * it takes long: after a moment a box shows that the save is going on, and lets the user stop
   * waiting (the save carries on, the program stays open) or give the changes up. A second request
   * to close or quit shows the box at once. Resolves with "saved" or "unsaved" when the save ends
   * (see `saveTab`), "stay" when the user stopped waiting, "leave" when the changes are given up.
   * `work` is told when the user stops waiting, so that it saves no more documents.
   */
  private async waitForSave(
    work: (halt: AbortSignal) => Promise<boolean>,
    text: { title: string; detail: string; leave: string },
  ): Promise<"saved" | "unsaved" | "stay" | "leave"> {
    const halt = new AbortController();
    const over = new AbortController();
    let shown = false;
    let choose!: (answer: "stay" | "leave") => void;
    const chosen = new Promise<"stay" | "leave">((resolve) => (choose = resolve));
    const reveal = () => {
      if (over.signal.aborted) return;
      // The save waits for the answer to a question (or an error) that is open: a second box with
      // a moving bar beside it, and the focus, would only be in the way. The question has the focus.
      if (!shown && this.noticeLayer.childElementCount > 0) {
        this.focusTopNotice();
        return;
      }
      if (!shown) {
        shown = true;
        void showNotice(this.noticeLayer, {
          title: text.title,
          detail: text.detail,
          busy: true,
          signal: over.signal,
          buttons: [
            { id: "stay", label: "Cancel", safe: true },
            { id: "leave", label: text.leave },
          ],
        }).then((answer) => {
          if (answer.button === "leave") choose("leave");
          else if (answer.button !== "dismissed") choose("stay");
        });
      }
      this.focusTopNotice();
    };
    // While a question is open the timer is held: it looks again after the same time, so the box
    // still comes up when the save goes on long after the answer.
    let timer: ReturnType<typeof setTimeout>;
    const tick = () => {
      if (over.signal.aborted) return;
      if (this.noticeLayer.childElementCount > 0) timer = setTimeout(tick, SAVE_SHEET_DELAY_MS);
      else reveal();
    };
    timer = setTimeout(tick, SAVE_SHEET_DELAY_MS);
    const before = this.quitReveal;
    this.quitReveal = reveal;
    const finished = work(halt.signal).then(
      (ok) => (ok ? ("saved" as const) : ("unsaved" as const)),
      () => "unsaved" as const,
    );
    try {
      const ended = await Promise.race([finished, chosen]);
      if (ended === "stay" || ended === "leave") halt.abort();
      return ended;
    } finally {
      clearTimeout(timer);
      over.abort();
      this.quitReveal = before;
    }
  }

  /** Moves the focus to the first button of the box that is on top, so that a key press reaches it. */
  private focusTopNotice(): void {
    this.noticeLayer.lastElementChild?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
  }

  /** Closes a tab at once, whatever it holds. The questions are asked by `requestClose`. */
  closeTab(id: number): void {
    const index = this.tabs.findIndex((t) => t.id === id);
    if (index < 0) return;
    const [tab] = this.tabs.splice(index, 1);
    this.saveNow(tab);
    clearTimeout(tab.saveTimer);
    // The pictures made for printing go with the document they show.
    if (this.printJob && (this.tabs.length === 0 || this.printJob.document === tab.doc)) {
      this.printJob.cancel();
      this.printJob.cleanup();
    }
    this.find.forget(tab.view);
    tab.annotator?.destroy();
    tab.view.destroy();
    this.sidebar.forget(tab.doc);
    void this.deps.api.close(tab.doc).catch(() => undefined);
    if (this.activeId === id) {
      this.activeId = null;
      const neighbour = this.tabs[index] ?? this.tabs[index - 1];
      if (neighbour) this.activate(neighbour.id);
    }
    this.syncChrome();
  }

  // ---- annotations -----------------------------------------------------------

  /** The annotations of a document, if the app process offers the calls for them. */
  private makeAnnotator(tab: Tab): Annotator | null {
    const api = this.deps.api;
    if (
      !api.listAnnotations ||
      !api.addMarkup ||
      !api.addNote ||
      !api.addInk ||
      !api.addStamp ||
      !api.updateAnnotation ||
      !api.removeAnnotation ||
      !api.restoreAnnotation
    ) {
      return null;
    }
    const annotator = new Annotator(tab.doc, tab.view, tab.history, api as AnnotatorApi, {
      popovers: this.popovers,
      announce: (text) => this.announce(text),
      ask: (options) => showNotice(this.noticeLayer, options),
      failed: (error) => void this.annotationFailed(tab, error),
      changed: () => {
        this.historyChanged(tab);
        if (this.active === tab) this.syncAnnotBar();
      },
      pageChanged: (page) => this.sidebar.pageChanged(tab.doc, page),
    });
    // Reading is the way a page is shown until the bar is opened.
    annotator.setReading(!this.annotOpen);
    return annotator;
  }

  /** A change to an annotation failed. A lost engine ends the tab; anything else is told and forgotten. */
  private async annotationFailed(tab: Tab, error: unknown): Promise<void> {
    if (!this.tabs.includes(tab)) return;
    const app = toAppError(error);
    if (isEngineLost(app)) {
      await this.documentLost(tab, app);
      return;
    }
    const text = describeError(app, tab.name);
    await showNotice(this.noticeLayer, {
      title: text.title === "Something went wrong" ? "The annotation was not changed" : text.title,
      detail: CLEAN_REFUSALS.has(app.code)
        ? `${text.detail} The document is as it was before this change.`
        : `${text.detail} Part of the change may have been made, so the document is marked as changed.`,
      buttons: [{ id: "ok", label: "OK", primary: true }],
    });
  }

  /** Says something to a screen reader, in the live region of the page. */
  private announce(text: string): void {
    this.announceFlip = !this.announceFlip;
    this.annotStatus.textContent = this.announceFlip ? text : `${text}\u00a0`;
  }

  /** Shows or hides the annotation bar. Hiding it puts the pointer back to picking. */
  private setAnnotating(open: boolean, focus = false): void {
    this.annotOpen = open;
    if (!open) {
      for (const tab of this.tabs) tab.annotator?.setTool("select");
    }
    for (const tab of this.tabs) tab.annotator?.setReading(!open);
    this.syncAnnotBar();
    if (open && focus) this.annotbar.focusBar();
  }

  /** Picks a tool from the keyboard: the bar opens if it was closed. */
  private chooseTool(tool: Tool): void {
    const annotator = this.active?.annotator;
    if (!annotator) return;
    if (!this.annotOpen) this.setAnnotating(true);
    if (tool === "stamp" && annotator.tool === "stamp") {
      this.annotbar.openStampMenu();
      return;
    }
    annotator.setTool(tool);
    this.active?.view.element.focus({ preventScroll: true });
  }

  /** Brings the annotation bar in step with the document in front. */
  private syncAnnotBar(): void {
    const tab = this.active;
    const annotator = tab?.annotator ?? null;
    const picked = annotator?.current ?? null;
    this.annotbar.render({
      open: this.annotOpen,
      enabled: annotator !== null,
      tool: annotator?.tool ?? "select",
      color: annotator?.color ?? PALETTE[0].rgba,
      penWidth: annotator?.penWidth ?? PEN_WIDTHS[1].width,
      stamp: annotator?.stamp ?? "approved",
      note: annotator?.blockedReason ?? annotator?.pickedReason ?? null,
      blocked: annotator?.blockedReason != null,
      canEditText: picked?.can.editText ?? false,
      canRemove: picked?.can.remove ?? false,
      canUndo: tab?.history.canUndo ?? false,
      canRedo: tab?.history.canRedo ?? false,
      undoLabel: tab?.history.undoLabel ?? null,
      redoLabel: tab?.history.redoLabel ?? null,
    });
  }

  /** The engine of one document stopped and took the document with it. The other tabs carry on. */
  private async documentLost(tab: Tab, error: AppErrorLike): Promise<void> {
    if (!this.tabs.includes(tab)) return;
    const lostChanges = this.isDirty(tab);
    this.closeTab(tab.id);
    const text = describeError(error, tab.name);
    const answer = await showNotice(this.noticeLayer, {
      title: text.title,
      detail: lostChanges ? `${text.detail} The changes that were not saved are lost.` : text.detail,
      buttons: [
        { id: "close", label: "Close" },
        { id: "retry", label: "Open the file again", primary: true },
      ],
    });
    if (answer.button === "retry") await this.openPath(tab.path);
  }

  // ---- changes that are not saved ---------------------------------------------

  /** The history of a tab changed: tell the app process if the document turned dirty or clean. */
  private historyChanged(tab: Tab): void {
    if (!this.tabs.includes(tab)) return;
    const dirty = this.isDirty(tab);
    if (dirty !== tab.dirtyTold) {
      tab.dirtyTold = dirty;
      // If the app process does not get to know, the next change tries again.
      this.deps.api.setDirty(tab.doc, dirty).catch(() => {
        tab.dirtyTold = !dirty;
      });
    }
    this.drawTabs();
    this.drawTitle();
    if (this.active === tab) this.syncAnnotBar();
  }

  /** Saves the document that is shown, if it has changes. Resolves with true when nothing is left unsaved. */
  async saveActive(): Promise<boolean> {
    const tab = this.active;
    if (!tab || !this.isDirty(tab)) return true;
    return this.saveTab(tab, false);
  }

  /** Saves a copy of the document that is shown under a name the user chooses; the tab follows the copy. */
  async saveActiveAs(): Promise<boolean> {
    const tab = this.active;
    if (!tab || tab.view.pageCount === 0) return false;
    return this.saveTab(tab, true);
  }

  /** Saves every document that has changes. Resolves with true when all were saved. */
  async saveAllChanges(halt?: AbortSignal): Promise<boolean> {
    for (const tab of this.tabs.filter((t) => this.isDirty(t))) {
      // The user stopped waiting: what is running finishes, but no other document is begun.
      if (halt?.aborted) return false;
      if (!(await this.saveTab(tab, false))) return false;
    }
    return true;
  }

  /**
   * Saves one document, in place or as a copy, and asks what the save asks: whether to go on when
   * the file was changed by something else since it was opened, and when the document is signed
   * (a save breaks a signature). A failure is told to the user. Resolves with true when saved.
   */
  private async saveTab(tab: Tab, asCopy: boolean): Promise<boolean> {
    // What is under way is done first (an open box of a note is saved, running changes finish),
    // so that the file holds what the window shows.
    await tab.annotator?.flush();
    // A save that is running is waited for, and then the state is looked at again: it may have
    // saved everything, or an edit may have come after it.
    while (tab.saving) {
      await tab.saving.catch(() => false);
      if (!asCopy && !this.isDirty(tab)) return true;
    }
    const running = this.writeTab(tab, asCopy);
    tab.saving = running;
    try {
      return await running;
    } finally {
      tab.saving = null;
    }
  }

  /**
   * Does one save. Resolves with true only when the document has nothing unsaved afterwards: a
   * save marks as saved the revision it wrote, so an edit made while the file was written (or one
   * that pushed the oldest edit out of the history) keeps the document dirty and the answer is no.
   * Only the window clears the dirty state; the app process is told when that happens.
   */
  private async writeTab(tab: Tab, asCopy: boolean): Promise<boolean> {
    const began = tab.history.revision;
    // The engine is not changed while the file is written; changes made meanwhile wait.
    const release = tab.annotator?.hold();
    let saved: SavedFile | null;
    try {
      saved = asCopy ? await this.runSaveAs(tab, false) : await this.runSave(tab);
    } finally {
      release?.();
    }
    if (!saved) return false;
    tab.history.markSaved(began);
    if (saved.path !== tab.path || saved.name !== tab.name) {
      tab.path = saved.path;
      tab.name = saved.name;
      this.syncChrome();
    }
    return !this.isDirty(tab);
  }

  private async runSave(tab: Tab): Promise<SavedFile | null> {
    let overwrite = false;
    let allowSigned = false;
    for (;;) {
      try {
        return await this.deps.api.save(tab.doc, overwrite, allowSigned);
      } catch (e) {
        const error = toAppError(e);
        if (error.code === "save_signed" && !allowSigned) {
          const answer = await this.askSigned(tab);
          if (answer === "copy") return this.runSaveAs(tab, true);
          if (answer !== "anyway") return null;
          allowSigned = true;
        } else if (error.code === "save_changed" && !overwrite) {
          const answer = await this.askChanged(tab);
          if (answer === "copy") return this.runSaveAs(tab, allowSigned);
          if (answer !== "overwrite") return null;
          overwrite = true;
        } else if (COPY_REMEDY.has(error.code)) {
          // A copy is the way out: the file stays as it is, the work goes to another file.
          if ((await this.askCopy(tab, error)) !== "copy") return null;
          return this.runSaveAs(tab, allowSigned);
        } else {
          await this.saveFailed(tab, error);
          return null;
        }
      }
    }
  }

  /** `allowSigned`: the user already agreed that the signature breaks (a copy was their choice). */
  private async runSaveAs(tab: Tab, allowSigned: boolean): Promise<SavedFile | null> {
    for (;;) {
      try {
        const saved = await this.deps.api.saveAs(tab.doc, allowSigned);
        if (saved?.touched) {
          // One calm line: nothing is wrong with the copy, but the user should know what it holds.
          void showNotice(this.noticeLayer, {
            title: "The file was touched since it was opened",
            detail: `"${tab.name}" got a new time of change while it was open. "${saved.name}" holds what Patient Partridge had read of it, and the original was not replaced.`,
            buttons: [{ id: "ok", label: "OK", primary: true }],
          });
        }
        return saved;
      } catch (e) {
        const error = toAppError(e);
        if (error.code === "save_signed" && !allowSigned) {
          // A copy keeps the signed file as it is, and the copy is the changed document.
          const answer = await this.askSigned(tab, true);
          if (answer !== "anyway") return null;
          allowSigned = true;
          continue;
        }
        await this.saveFailed(tab, error);
        return null;
      }
    }
  }

  private async askSigned(tab: Tab, copy = false): Promise<"copy" | "anyway" | "cancel"> {
    const answer = await showNotice(this.noticeLayer, {
      title: "This document is signed",
      detail: copy
        ? `"${tab.name}" has a digital signature. Saving writes the document out again, and that breaks the signature: programs that check it will say the document was changed after it was signed.`
        : `"${tab.name}" has a digital signature. Saving writes the document out again, and that breaks the signature: programs that check it will say the document was changed after it was signed. A copy leaves the signed file as it is.`,
      buttons: [
        { id: "cancel", label: "Cancel", safe: true },
        ...(copy ? [] : [{ id: "copy", label: "Save a copy…", primary: true }]),
        { id: "anyway", label: copy ? "Save and break the signature" : "Save anyway", primary: copy },
      ],
    });
    return answer.button === "copy" || answer.button === "anyway" ? answer.button : "cancel";
  }

  /** The save cannot replace this file, for a reason a copy gets round. */
  private async askCopy(tab: Tab, error: AppErrorLike): Promise<"copy" | "cancel"> {
    const text = describeError(error, tab.name);
    const answer = await showNotice(this.noticeLayer, {
      title: text.title,
      detail: text.detail,
      buttons: [
        { id: "cancel", label: "Cancel", safe: true },
        { id: "copy", label: "Save a copy…", primary: true },
      ],
    });
    return answer.button === "copy" ? "copy" : "cancel";
  }

  private async askChanged(tab: Tab): Promise<"copy" | "overwrite" | "cancel"> {
    const answer = await showNotice(this.noticeLayer, {
      title: "The file was changed by something else",
      detail: `"${tab.name}" was changed on the disk after you opened it. If you save, your version replaces those changes. A copy keeps both.`,
      buttons: [
        { id: "cancel", label: "Cancel", safe: true },
        { id: "copy", label: "Save a copy…", primary: true },
        { id: "overwrite", label: "Replace the file" },
      ],
    });
    return answer.button === "copy" || answer.button === "overwrite" ? answer.button : "cancel";
  }

  private async saveFailed(tab: Tab, error: AppErrorLike): Promise<void> {
    // The engine of the document is gone: so are the changes, and the tab with them.
    if (isEngineLost(error)) {
      await this.documentLost(tab, error);
      return;
    }
    const text = describeError(error, tab.name);
    await showNotice(this.noticeLayer, {
      title: text.title,
      detail: text.detail,
      buttons: [{ id: "ok", label: "OK", primary: true }],
    });
  }

  /**
   * The app asks the window to close: the user used the close button of the window, or quit. With
   * changes that are not saved the user is asked first. Quitting goes on only when every change
   * was saved or given up.
   */
  async requestQuit(): Promise<void> {
    if (this.quitting) {
      // Asked again while the first request is still at work (a question, or a save that takes
      // long): it must not look as if nothing happened, so the box that answers it is brought up.
      this.quitReveal?.();
      return;
    }
    this.quitting = true;
    this.quitReveal = () => this.focusTopNotice();
    try {
      const dirty = this.tabs.filter((t) => this.isDirty(t));
      if (dirty.length === 0) {
        await this.deps.api.quitApp();
        return;
      }
      const names = dirty.slice(0, 5).map((t) => `"${t.name}"`);
      if (dirty.length > 5) names.push(`and ${dirty.length - 5} more`);
      const answer = await showNotice(this.noticeLayer, {
        title:
          dirty.length === 1 ? `Save changes to "${dirty[0].name}"?` : `Save changes to ${dirty.length} documents?`,
        detail:
          dirty.length === 1
            ? "If you quit without saving, the changes you made are lost."
            : `${names.join(", ")} have changes that are not saved. If you quit without saving, they are lost.`,
        buttons: [
          { id: "cancel", label: "Cancel", safe: true },
          { id: "discard", label: "Quit without saving" },
          { id: "save", label: "Save and quit", primary: true },
        ],
      });
      if (answer.button === "discard") {
        await this.deps.api.quitApp();
        return;
      }
      if (answer.button === "save") {
        const ended = await this.waitForSave((halt) => this.saveAllChanges(halt), {
          title: dirty.length === 1 ? `Saving "${dirty[0].name}"…` : `Saving ${dirty.length} documents…`,
          detail: SAVE_SLOW_DETAIL,
          leave: "Quit without saving",
        });
        if (ended === "saved" || ended === "leave") {
          await this.deps.api.quitApp();
          return;
        }
      }
      await this.deps.api.stayOpen().catch(() => undefined);
    } finally {
      this.quitting = false;
      this.quitReveal = null;
    }
  }

  private async undoActive(): Promise<void> {
    await this.stepHistory("undo");
  }

  private async redoActive(): Promise<void> {
    await this.stepHistory("redo");
  }

  private async stepHistory(direction: "undo" | "redo"): Promise<void> {
    const tab = this.active;
    if (!tab || tab.saving) return;
    const run = () => (direction === "undo" ? tab.history.undo() : tab.history.redo());
    try {
      // After the changes asked for before it, and before the ones asked for later.
      await (tab.annotator ? tab.annotator.sequenced(run) : run());
    } catch (error) {
      await showNotice(this.noticeLayer, {
        title: direction === "undo" ? "The change could not be taken back" : "The change could not be made again",
        detail: CLEAN_REFUSALS.has(toAppError(error).code)
          ? "The document is as it was before. Nothing was lost."
          : "Part of the change may have been made. The document is marked as changed; close it without saving to go back to the file.",
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
    }
  }

  // ---- remembered positions --------------------------------------------------

  private scheduleSave(tab: Tab): void {
    clearTimeout(tab.saveTimer);
    tab.saveTimer = setTimeout(() => this.saveNow(tab), this.positionDelay);
  }

  private saveNow(tab: Tab): void {
    clearTimeout(tab.saveTimer);
    tab.saveTimer = undefined;
    if (tab.view.pageCount === 0) return;
    const place = tab.view.getPosition();
    if (tab.forgottenAt) {
      // The reader asked to forget positions: a file that was open at that moment is written down
      // again only after the reader has really moved in it.
      if (samePlace(tab.forgottenAt, place)) return;
      tab.forgottenAt = null;
    }
    void this.deps.api.setPosition(tab.path, place).catch(() => undefined);
  }

  private saveAll(): void {
    for (const tab of this.tabs) this.saveNow(tab);
  }

  private async forgetPositions(): Promise<void> {
    this.menu.close();
    const answer = await showNotice(this.noticeLayer, {
      title: "Forget remembered positions?",
      detail:
        "Patient Partridge remembers where you stopped reading in each file, on this computer only. Forgetting them opens every file at the top again. The list of tabs to reopen at the next start is cleared too, and starts again with the tabs that are open when you next open or close one.",
      buttons: [
        { id: "cancel", label: "Keep them", safe: true },
        { id: "forget", label: "Forget positions", primary: true },
      ],
    });
    if (answer.button !== "forget") return;
    // Nothing may write a position back after this, also not the files that are open now: closing
    // a tab, choosing another one or hiding the window all save the place of a tab.
    for (const tab of this.tabs) {
      clearTimeout(tab.saveTimer);
      tab.saveTimer = undefined;
      tab.forgottenAt = tab.view.getPosition();
    }
    // The list of tabs goes with the positions, also the files that could not be reached.
    this.kept = [];
    await this.deps.api.clearPositions().catch(() => undefined);
  }

  // ---- reopening the tabs ----------------------------------------------------

  /** Remembers which tabs are open, in order, and which one is in view. Paths only. */
  private saveSession(): void {
    if (this.sessionHeld || !this.reopenTabs) return;
    const open = this.tabs.map((t) => t.path);
    const active = this.tabs.findIndex((t) => t.id === this.activeId);
    // The files that could not be reached stay at the end of the list for one more start.
    const failed = this.kept.filter((p) => !open.includes(p));
    const paths = [...open, ...failed];
    const key = JSON.stringify([paths, active, failed]);
    if (key === this.lastSession) return;
    this.lastSession = key;
    void this.deps.api.saveSession(paths, active >= 0 ? active : null, failed).catch(() => {
      this.lastSession = null;
    });
  }

  private initReopenSetting(): void {
    const toggle = byId<HTMLButtonElement>(this.root, "reopen-tabs");
    const show = () => toggle.setAttribute("aria-checked", String(this.reopenTabs));
    show();
    void this.deps.api
      .getReopenTabs()
      .then((on) => {
        this.reopenTabs = on;
        show();
      })
      .catch(() => undefined);
    toggle.addEventListener("click", () => void this.toggleReopen(show));
  }

  private async toggleReopen(show: () => void): Promise<void> {
    const wanted = !this.reopenTabs;
    this.reopenTabs = wanted;
    // Turning it off deletes the list, so turning it on again must write the list afresh.
    this.lastSession = null;
    show();
    try {
      await this.deps.api.setReopenTabs(wanted);
    } catch {
      await showNotice(this.noticeLayer, {
        title: "The setting was not saved",
        detail:
          "Patient Partridge could not write the setting to this computer. It holds until the app closes, then it goes back to what it was.",
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
    }
    this.saveSession();
  }

  // ---- colour theme ----------------------------------------------------------

  private initTheme(): void {
    const storage = this.deps.storage ?? null;
    const apply = (value: string) => {
      const pref = parseTheme(value);
      applyTheme(this.root.documentElement, pref);
      return pref;
    };
    const current = loadTheme(storage);
    apply(current);
    const radios = this.root.querySelectorAll<HTMLInputElement>('#theme-choice input[name="theme"]');
    for (const radio of radios) {
      radio.checked = radio.value === current;
      radio.addEventListener("change", () => {
        if (!radio.checked) return;
        saveTheme(storage, apply(radio.value));
      });
    }
    if (radios.length !== THEME_CHOICES.length) throw new Error("the menu lacks a theme choice");
  }

  // ---- toolbar and tabs ------------------------------------------------------

  /** Brings the tab strip, the toolbar, the title and the empty state in step with the tabs. */
  private syncChrome(saveTabs = true): void {
    // Every change of the tabs (opened, closed, brought to the front) ends here.
    if (saveTabs) this.saveSession();
    const active = this.active;
    this.drawTabs();
    this.empty.hidden = this.tabs.length > 0;
    this.setControlsEnabled(active !== null);
    this.sidebar.setDocument(active?.sidebarDoc ?? null);
    this.find.setTarget(active ? { doc: active.doc, view: active.view } : null);
    this.selectionProxy.textContent = "";
    this.syncAnnotBar();
    if (!active) {
      this.pageInput.value = "";
      this.pageTotal.textContent = "";
      this.drawTitle();
      this.buildZoomOptions(null);
      return;
    }
    this.pageTotal.textContent = `/ ${active.view.pageCount}`;
    this.drawTitle();
    this.showZoom(active.view.zoomValue, active.view.zoomMode);
    this.showCurrentPage();
  }

  /** Draws the tab strip: one tab for each document, with a dot on those with unsaved changes. */
  private drawTabs(): void {
    this.tabStrip.render(
      this.tabs.map((t) => ({ id: t.id, title: t.name, path: t.path, dirty: this.isDirty(t) })),
      this.activeId,
    );
  }

  private drawTitle(): void {
    const active = this.active;
    if (!active) {
      this.deps.setWindowTitle(APP_NAME);
      return;
    }
    this.deps.setWindowTitle(`${this.isDirty(active) ? "\u2022 " : ""}${active.name} - ${APP_NAME}`);
  }

  private setControlsEnabled(on: boolean): void {
    for (const el of [this.pageInput, this.zoomSelect, this.zoomIn, this.zoomOut]) el.disabled = !on;
    byId<HTMLButtonElement>(this.root, "print-button").disabled = !on;
  }

  private showCurrentPage(page?: number): void {
    const active = this.active;
    if (!active) return;
    // Do not overwrite what the user is typing in the box.
    if (page === undefined || this.root.activeElement !== this.pageInput) {
      this.pageInput.value = String(
        (page ?? active.view.currentPageIndex) + (active.view.pageCount === 0 ? 0 : 1),
      );
    }
  }

  private onPageInputKey(e: KeyboardEvent): void {
    const view = this.active?.view;
    if (!view) return;
    if (e.key === "Enter") {
      e.preventDefault();
      const index = parsePageInput(this.pageInput.value, view.pageCount);
      if (index !== null) view.goToPage(index);
      view.element.focus();
      this.showCurrentPage();
    } else if (e.key === "Escape") {
      e.preventDefault();
      view.element.focus();
      this.showCurrentPage();
    }
  }

  private buildZoomOptions(extra: number | null): void {
    this.zoomSelect.replaceChildren();
    const add = (value: string, label: string) => {
      const option = this.root.createElement("option");
      option.value = value;
      option.textContent = label;
      this.zoomSelect.append(option);
    };
    add("fit", "Fit width");
    const choices = [...ZOOM_CHOICES];
    if (extra !== null && !choices.some((c) => Math.abs(c - extra) < 0.005)) {
      choices.push(extra);
      choices.sort((a, b) => a - b);
    }
    for (const zoom of choices) add(String(zoom), zoomLabel(zoom));
  }

  private showZoom(zoom: number, mode: ZoomMode): void {
    if (mode.kind === "fit-width") {
      this.buildZoomOptions(null);
      this.zoomSelect.value = "fit";
      // The label of the fit option tells the actual percentage.
      this.zoomSelect.options[0].textContent = `Fit width (${zoomLabel(zoom)})`;
      return;
    }
    this.buildZoomOptions(zoom);
    const match = [...this.zoomSelect.options].find(
      (o) => o.value !== "fit" && Math.abs(Number(o.value) - zoom) < 0.005,
    );
    this.zoomSelect.value = match ? match.value : "fit";
  }

  private onZoomSelect(): void {
    const view = this.active?.view;
    if (!view) return;
    const value = this.zoomSelect.value;
    if (value === "fit") view.setFitWidth();
    else view.setZoom(Number(value));
    view.element.focus();
  }

  // ---- keyboard --------------------------------------------------------------

  private onKeyDown(e: KeyboardEvent): void {
    if (this.noticeLayer.childElementCount === 0 && this.annotationKey(e)) {
      e.preventDefault();
      return;
    }
    if (e.key === "Escape" && this.noticeLayer.childElementCount === 0) {
      const view = this.active?.view;
      if (view?.hasSelection && !(e.target instanceof HTMLInputElement)) {
        e.preventDefault();
        view.clearSelection();
        return;
      }
    }
    const action = actionForKey(e);
    if (!action) return;
    const target = e.target as HTMLElement | null;
    const typing =
      target instanceof HTMLInputElement ||
      target instanceof HTMLSelectElement ||
      target instanceof HTMLTextAreaElement;
    // Plain keys belong to a field or button while one has the focus; shortcuts with Ctrl always work.
    const isShortcut = e.ctrlKey || e.metaKey;
    // F3 steps through the matches also while the find box has the focus.
    const stepsMatches = action.type === "find-next" || action.type === "find-previous";
    if (!isShortcut && !stepsMatches && (typing || target instanceof HTMLButtonElement)) return;
    if (this.noticeLayer.childElementCount > 0) return;

    if (action.type === "open-file") {
      e.preventDefault();
      void this.openDialog();
    } else if (action.type === "focus-page-box") {
      e.preventDefault();
      if (!this.pageInput.disabled) {
        this.pageInput.focus();
        this.pageInput.select();
      }
    } else if (action.type === "print") {
      e.preventDefault();
      void this.printActive();
    } else if (action.type === "find") {
      e.preventDefault();
      this.find.open();
    } else if (action.type === "find-next" || action.type === "find-previous") {
      e.preventDefault();
      this.find.step(action.type === "find-next" ? 1 : -1);
    } else if (action.type === "select-all") {
      // In a text box, Ctrl+A belongs to the box.
      if (typing) return;
      e.preventDefault();
      this.active?.view.selectAll();
    } else if (action.type === "toggle-sidebar") {
      e.preventDefault();
      this.sidebar.setOpen(!this.sidebar.isOpen);
    } else if (action.type === "close-tab") {
      e.preventDefault();
      if (this.activeId !== null) void this.requestClose(this.activeId);
    } else if (action.type === "save") {
      e.preventDefault();
      void this.saveActive();
    } else if (action.type === "save-as") {
      e.preventDefault();
      void this.saveActiveAs();
    } else if (action.type === "undo" || action.type === "redo") {
      // In a text box, undo and redo belong to the box.
      if (typing) return;
      e.preventDefault();
      void (action.type === "undo" ? this.undoActive() : this.redoActive());
    } else if (action.type === "tool") {
      e.preventDefault();
      this.chooseTool(action.tool);
    } else if (action.type === "toggle-annotations") {
      e.preventDefault();
      if (this.active?.annotator) this.setAnnotating(!this.annotOpen, !this.annotOpen);
    } else if (action.type === "next-tab" || action.type === "previous-tab") {
      e.preventDefault();
      this.stepTab(action.type === "next-tab" ? 1 : -1);
    } else if (this.active?.view.handleAction(action)) {
      e.preventDefault();
    }
  }

  /**
   * Keys for the annotation that is picked (Delete, Enter, the arrows) and for the tool in use
   * (Escape, and Enter for the note and the stamp). They belong to the annotation only when no text
   * is being typed and no button but an annotation has the focus.
   */
  private annotationKey(e: KeyboardEvent): boolean {
    const annotator = this.active?.annotator;
    if (!annotator || e.ctrlKey || e.metaKey || e.altKey) return false;
    const target = e.target as HTMLElement | null;
    const typing =
      target instanceof HTMLInputElement ||
      target instanceof HTMLSelectElement ||
      target instanceof HTMLTextAreaElement;
    if (typing) return false;
    const onBox = target instanceof Element && target.closest(".annot") !== null;
    if (e.key === "Escape") return annotator.handleKey(e);
    if (target instanceof HTMLButtonElement && !onBox) return false;
    return annotator.handleKey(e);
  }

  // ---- printing --------------------------------------------------------------

  /** Asks which pages, prepares them, and opens the print dialog of the system. */
  async printActive(): Promise<void> {
    const tab = this.active;
    if (!tab || this.printing || tab.view.pageCount === 0) return;
    this.printing = true;
    try {
      const choice = await showPrintDialog(this.noticeLayer, {
        name: tab.name,
        pageCount: tab.view.pageCount,
        currentPage: tab.view.currentPageIndex,
      });
      if (!choice || choice.pages.length === 0) return;
      const job = (this.printJob ??= new PrintJob(this.printRoot, {
        render: (doc, page, width, height) => this.deps.api.render(doc, page, width, height),
        encode: this.deps.print?.encode ?? encodeJpeg,
        createUrl: this.deps.print?.createUrl ?? ((blob) => URL.createObjectURL(blob)),
        revokeUrl: this.deps.print?.revokeUrl ?? ((url) => URL.revokeObjectURL(url)),
        print:
          this.deps.print?.print ??
          (() => {
            this.root.defaultView?.print();
            return Promise.resolve();
          }),
      }));
      const progress = showPrintProgress(this.noticeLayer, () => job.cancel());
      try {
        await job.run(tab.doc, tab.sidebarDoc.pages, choice.pages, (p) => progress.update(p));
      } finally {
        progress.close();
      }
    } catch (e) {
      const text = describeError(toAppError(e), tab.name);
      await showNotice(this.noticeLayer, {
        title: "The document could not be printed",
        detail:
          text.title === "Something went wrong"
            ? `${text.detail} Nothing was sent to the printer.`
            : `${text.title}. Nothing was sent to the printer.`,
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
    } finally {
      this.printing = false;
    }
  }

  // ---- copying, and links ----------------------------------------------------

  /**
   * Keeps the selected text in a hidden element that is really selected, so that the Copy of the
   * system (the menu, the keyboard, a context menu) has something to copy and is not greyed out.
   * A very long selection is not put in the page; Copy then uses the loaded text directly.
   */
  private updateSelectionProxy(tab: Tab): void {
    if (this.active !== tab) return;
    const { text, missing } = tab.view.selectedText();
    if (missing.length > 0 || text.length === 0) {
      this.selectionProxy.textContent = "";
      return;
    }
    this.selectionProxy.textContent = text.length <= 200_000 ? text : " ";
    const selection = this.root.getSelection?.() ?? null;
    selection?.removeAllRanges();
    const range = this.root.createRange();
    range.selectNodeContents(this.selectionProxy);
    selection?.addRange(range);
  }

  private onCopy(e: ClipboardEvent): void {
    const target = e.target;
    // A text box copies its own text.
    if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) return;
    const view = this.active?.view;
    if (!view?.hasSelection) return;
    const { text, missing, cut } = view.selectedText();
    // A selection with no text in it (a scanned page) leaves the clipboard as it was.
    if (missing.length === 0 && text.length === 0) return;
    if (missing.length === 0 && e.clipboardData) {
      e.clipboardData.setData("text/plain", text);
      e.preventDefault();
      // Everything is loaded, but the engine may have cut a page: the reader is told here too.
      if (cut > 0) void this.tellAboutCopy({ pages: 0, total: 0, failed: [], truncated: false, cut });
      return;
    }
    // Some pages are not loaded yet: fetch them, then write the text to the clipboard.
    e.preventDefault();
    void this.copyFetched(view);
  }

  /**
   * Copies a selection that has pages not loaded yet. The text is written after the copy event is
   * over, which some web views allow only after a press of a key: that can fail. Whatever goes
   * wrong, or is left out, the reader is told, so a copy is never taken for the whole text when it
   * is not. A copy that is already running for the document, a document that was closed on the
   * way and a selection that was changed on the way write nothing.
   */
  private async copyFetched(view: DocumentView): Promise<void> {
    let shown = -1;
    const copy = await view.loadSelectedText((done, total) => {
      // The line is updated in steps of 5 percent, so a screen reader is not told every page.
      const step = Math.floor((done * 20) / total);
      if (step === shown) return;
      shown = step;
      this.copyLines.set(view, `Copying the text: page ${done} of ${total}`);
      this.showCopyLine();
    });
    // A press that found a copy already running did not start a line, so it leaves the line alone.
    if (copy.status !== "busy") {
      this.copyLines.delete(view);
      this.showCopyLine();
    }
    if (copy.status === "changed") {
      await showNotice(this.noticeLayer, {
        title: "The text was not copied",
        detail: "The selection changed while the text was being copied, so nothing was put on the clipboard. Select the text again and press Copy.",
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
      return;
    }
    if (copy.status !== "done") return;
    if (copy.text.length > 0) {
      try {
        const clipboard = this.root.defaultView?.navigator.clipboard;
        if (!clipboard) throw new Error("no clipboard");
        await clipboard.writeText(copy.text);
      } catch {
        await showNotice(this.noticeLayer, {
          title: "The text could not be copied",
          detail:
            "Patient Partridge could not put the text on the clipboard. Select it again and press Copy, or select less text.",
          buttons: [{ id: "ok", label: "OK", primary: true }],
        });
        return;
      }
    } else if (copy.failed.length === 0 && !copy.truncated) {
      return;
    }
    await this.tellAboutCopy(copy, copy.text.length === 0);
  }

  /** Shows the line of the copy that started last and is still running, or hides the line. */
  private showCopyLine(): void {
    const last = [...this.copyLines.values()].at(-1);
    this.copyStatus.textContent = last ?? "";
    this.copyStatus.hidden = last === undefined;
  }

  /** Says what a copy left out, if anything. */
  private async tellAboutCopy(
    copy: Pick<CopyResult, "pages" | "total" | "failed" | "truncated" | "cut">,
    nothing = false,
  ): Promise<void> {
    const left: string[] = [];
    if (copy.truncated) {
      left.push(
        `The text of ${copy.pages} of ${copy.total} pages was copied. One copy holds a limited amount of text, so the rest is missing. Select a smaller part to copy the rest.`,
      );
    }
    if (copy.failed.length > 0) {
      const n = copy.failed.length;
      left.push(`${n === 1 ? "1 page" : `${n} pages`} could not be read, so the text is missing there.`);
    }
    if (copy.cut > 0) {
      left.push(
        `${copy.cut === 1 ? "1 page has" : `${copy.cut} pages have`} more text than the reader can handle, so the end of the text on ${copy.cut === 1 ? "it is" : "them is"} missing.`,
      );
    }
    if (left.length === 0) return;
    await showNotice(this.noticeLayer, {
      title: nothing ? "The text could not be copied" : "Only part of the text was copied",
      detail: left.join(" "),
      buttons: [{ id: "ok", label: "OK", primary: true }],
    });
  }

  // ---- the menu of the right mouse button -----------------------------------------

  /**
   * The web view's own context menu is off in the finished app (it holds Reload), and it would
   * not copy the text of a page anyway: the text is drawn by the app. So the right mouse button on
   * selected text opens a small menu of the app's own with Copy. Text boxes keep the menu of the
   * web view, which has Cut, Copy and Paste.
   */
  private initContextMenu(): void {
    const menu = byId(this.root, "context-menu");
    const copy = byId<HTMLButtonElement>(this.root, "context-copy");
    const close = () => {
      menu.hidden = true;
    };
    this.root.addEventListener("contextmenu", (e) => {
      const target = e.target;
      if (target instanceof Element && target.closest("input, textarea, select")) return;
      close();
      const view = this.active?.view;
      if (!view?.hasSelection || !(target instanceof Node) || !view.element.contains(target)) return;
      e.preventDefault();
      menu.style.left = `${Math.max(0, Math.min(e.clientX, (this.root.defaultView?.innerWidth ?? 0) - 140))}px`;
      menu.style.top = `${Math.max(0, Math.min(e.clientY, (this.root.defaultView?.innerHeight ?? 0) - 56))}px`;
      menu.hidden = false;
      copy.focus({ preventScroll: true });
    });
    copy.addEventListener("click", () => {
      close();
      const view = this.active?.view;
      if (view?.hasSelection) void this.copySelection(view);
      view?.element.focus({ preventScroll: true });
    });
    menu.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close();
        this.active?.view.element.focus({ preventScroll: true });
      }
    });
    this.root.addEventListener("pointerdown", (e) => {
      if (!menu.hidden && e.target instanceof Node && !menu.contains(e.target)) close();
    });
    this.root.addEventListener("scroll", close, true);
  }

  /** Copy from the menu: the same as Ctrl+C, except that the text is written here, on a press. */
  private async copySelection(view: DocumentView): Promise<void> {
    const { text, missing, cut } = view.selectedText();
    if (missing.length > 0) {
      await this.copyFetched(view);
      return;
    }
    // Nothing to copy (a scanned page): the clipboard keeps what it had.
    if (text.length === 0) return;
    try {
      const clipboard = this.root.defaultView?.navigator.clipboard;
      if (!clipboard) throw new Error("no clipboard");
      await clipboard.writeText(text);
    } catch {
      await showNotice(this.noticeLayer, {
        title: "The text could not be copied",
        detail: "Patient Partridge could not put the text on the clipboard. Press Ctrl+C instead.",
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
      return;
    }
    if (cut > 0) await this.tellAboutCopy({ pages: 0, total: 0, failed: [], truncated: false, cut });
  }

  /** A link on a page was pressed. A page link just goes there; a web address is asked about first. */
  private async onLink(tab: Tab, link: PageLinkInfo, _page: number): Promise<void> {
    if (link.page !== null) {
      tab.view.goToPage(link.page);
      return;
    }
    const uri = link.uri ?? "";
    const verdict = classifyUri(uri);
    if (verdict.kind === "blocked") {
      await showNotice(this.noticeLayer, {
        title: "This link is not opened",
        detail: verdict.reason,
        quote: visibleAddress(uri.length > 300 ? `${uri.slice(0, 300)}…` : uri),
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
      return;
    }
    const answer = await showNotice(this.noticeLayer, {
      title: "Open this link in your browser?",
      detail: `This document wants to open a web address on the site ${verdict.host}. Only open it if you trust the document.`,
      quote: verdict.url,
      armMs: this.deps.linkAnswerDelayMs ?? LINK_ANSWER_DELAY_MS,
      buttons: [
        { id: "cancel", label: "Cancel", safe: true },
        { id: "open", label: "Open in browser", primary: true },
      ],
    });
    if (answer.button !== "open") return;
    try {
      await this.deps.api.openLink(tab.doc, uri);
    } catch {
      await showNotice(this.noticeLayer, {
        title: "The link could not be opened",
        detail: "Patient Partridge could not start your web browser. Copy the address from the document instead.",
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
    }
  }

  private stepTab(direction: 1 | -1): void {
    if (this.tabs.length < 2) return;
    const index = this.tabs.findIndex((t) => t.id === this.activeId);
    const next = this.tabs[(index + direction + this.tabs.length) % this.tabs.length];
    this.activate(next.id);
  }
}

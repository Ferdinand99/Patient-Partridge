// The app: the tab strip, the toolbar, opening files, and showing one document at a time.
// Anything that touches the outside world (the engine, the file dialog, the window title, the
// storage) comes in through `AppDeps`, so tests can run the whole flow with fakes.

import type { OpenedDocument, OutlineEntry, SavedPosition } from "../lib/api";
import { describeError, fileNameOf, toAppError, type AppErrorLike } from "../lib/errors";
import { parsePageInput } from "../lib/pageInput";
import {
  THEME_CHOICES,
  applyTheme,
  loadTheme,
  parseTheme,
  saveTheme,
  type StorageLike,
} from "../lib/theme";
import { actionForKey } from "../viewer/keys";
import { DocumentView } from "../viewer/viewer";
import { zoomLabel, type ZoomMode } from "../viewer/zoom";
import { MenuController } from "./menu";
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
    getOutline(doc: number): Promise<OutlineEntry[]>;
    measureMode(): Promise<boolean>;
    measureReport(openMs: number, firstPageMs: number, pages: number): Promise<void>;
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
}

interface Tab {
  id: number;
  doc: number;
  path: string;
  name: string;
  view: DocumentView;
  /** What the side panel knows about this document. */
  sidebarDoc: SidebarDocument;
  saveTimer: ReturnType<typeof setTimeout> | undefined;
}

const ZOOM_CHOICES = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];
const DEFAULT_POSITION_DELAY_MS = 600;
const APP_NAME = "Patient Partridge";

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
      close: (id) => this.closeTab(id),
    });
    this.menu = new MenuController(byId(root, "menu-button"), byId(root, "menu"));
    this.sidebar = new Sidebar(root, {
      api: deps.api,
      storage: deps.storage ?? null,
      thumbs: deps.thumbSettleMs === undefined ? undefined : { settleMs: deps.thumbSettleMs },
    });
  }

  init(): void {
    byId(this.root, "open-button").addEventListener("click", () => void this.openDialog());
    byId(this.root, "empty-open").addEventListener("click", () => void this.openDialog());
    this.zoomIn.addEventListener("click", () => this.active?.view.stepZoom(1));
    this.zoomOut.addEventListener("click", () => this.active?.view.stepZoom(-1));
    this.zoomSelect.addEventListener("change", () => this.onZoomSelect());
    this.pageInput.addEventListener("keydown", (e) => this.onPageInputKey(e));
    this.pageInput.addEventListener("blur", () => this.showCurrentPage());
    this.root.addEventListener("keydown", (e) => this.onKeyDown(e));
    this.initTheme();
    byId(this.root, "forget-positions").addEventListener("click", () => void this.forgetPositions());
    // The reader's place is saved when they rest, and again when the window goes away.
    this.root.addEventListener("visibilitychange", () => {
      if (this.root.visibilityState === "hidden") this.saveAll();
    });
    this.root.defaultView?.addEventListener("pagehide", () => this.saveAll());
    this.buildZoomOptions(null);
    this.syncChrome();
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

  setDropActive(active: boolean): void {
    this.dropOverlay.hidden = !active;
  }

  private get active(): Tab | null {
    return this.tabs.find((t) => t.id === this.activeId) ?? null;
  }

  // ---- opening and closing ---------------------------------------------------

  async openDialog(): Promise<void> {
    const paths = await this.deps.pickFiles();
    if (paths.length > 0) await this.openPaths(paths);
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

  private addTab(opened: OpenedDocument, path: string, started: number, activate: boolean): void {
    const openedAfter = this.deps.now() - started;
    const tab: Tab = {
      id: this.nextTabId++,
      doc: opened.doc,
      path,
      name: opened.name,
      view: undefined as unknown as DocumentView,
      sidebarDoc: undefined as unknown as SidebarDocument,
      saveTimer: undefined,
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
      },
      { start: opened.position ?? null },
    );
    tab.sidebarDoc = {
      doc: opened.doc,
      pages: opened.pages,
      goToPage: (index) => tab.view.goToPage(index),
      hasPainted: () => tab.view.hasPainted,
      currentPage: () => tab.view.currentPageIndex,
    };
    this.tabs.push(tab);
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

  closeTab(id: number): void {
    const index = this.tabs.findIndex((t) => t.id === id);
    if (index < 0) return;
    const [tab] = this.tabs.splice(index, 1);
    this.saveNow(tab);
    clearTimeout(tab.saveTimer);
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

  /** The engine of one document stopped and took the document with it. The other tabs carry on. */
  private async documentLost(tab: Tab, error: AppErrorLike): Promise<void> {
    if (!this.tabs.includes(tab)) return;
    this.closeTab(tab.id);
    const text = describeError(error, tab.name);
    const answer = await showNotice(this.noticeLayer, {
      title: text.title,
      detail: text.detail,
      buttons: [
        { id: "close", label: "Close" },
        { id: "retry", label: "Open the file again", primary: true },
      ],
    });
    if (answer.button === "retry") await this.openPath(tab.path);
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
    void this.deps.api.setPosition(tab.path, tab.view.getPosition()).catch(() => undefined);
  }

  private saveAll(): void {
    for (const tab of this.tabs) this.saveNow(tab);
  }

  private async forgetPositions(): Promise<void> {
    this.menu.close();
    const answer = await showNotice(this.noticeLayer, {
      title: "Forget remembered positions?",
      detail:
        "Patient Partridge remembers where you stopped reading in each file, on this computer only. Forgetting them opens every file at the top again.",
      buttons: [
        { id: "cancel", label: "Keep them", safe: true },
        { id: "forget", label: "Forget positions", primary: true },
      ],
    });
    if (answer.button !== "forget") return;
    // Clear the file first, then the timers: nothing may write a position back after this.
    for (const tab of this.tabs) {
      clearTimeout(tab.saveTimer);
      tab.saveTimer = undefined;
    }
    await this.deps.api.clearPositions().catch(() => undefined);
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
  private syncChrome(): void {
    const active = this.active;
    this.tabStrip.render(
      this.tabs.map((t) => ({ id: t.id, title: t.name, path: t.path })),
      this.activeId,
    );
    this.empty.hidden = this.tabs.length > 0;
    this.setControlsEnabled(active !== null);
    this.sidebar.setDocument(active?.sidebarDoc ?? null);
    if (!active) {
      this.pageInput.value = "";
      this.pageTotal.textContent = "";
      this.deps.setWindowTitle(APP_NAME);
      this.buildZoomOptions(null);
      return;
    }
    this.pageTotal.textContent = `/ ${active.view.pageCount}`;
    this.deps.setWindowTitle(`${active.name} - ${APP_NAME}`);
    this.showZoom(active.view.zoomValue, active.view.zoomMode);
    this.showCurrentPage();
  }

  private setControlsEnabled(on: boolean): void {
    for (const el of [this.pageInput, this.zoomSelect, this.zoomIn, this.zoomOut]) el.disabled = !on;
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
    const action = actionForKey(e);
    if (!action) return;
    const target = e.target as HTMLElement | null;
    const typing =
      target instanceof HTMLInputElement ||
      target instanceof HTMLSelectElement ||
      target instanceof HTMLTextAreaElement;
    // Plain keys belong to a field or button while one has the focus; shortcuts with Ctrl always work.
    const isShortcut = e.ctrlKey || e.metaKey;
    if (!isShortcut && (typing || target instanceof HTMLButtonElement)) return;
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
    } else if (action.type === "toggle-sidebar") {
      e.preventDefault();
      this.sidebar.setOpen(!this.sidebar.isOpen);
    } else if (action.type === "close-tab") {
      e.preventDefault();
      if (this.activeId !== null) this.closeTab(this.activeId);
    } else if (action.type === "next-tab" || action.type === "previous-tab") {
      e.preventDefault();
      this.stepTab(action.type === "next-tab" ? 1 : -1);
    } else if (this.active?.view.handleAction(action)) {
      e.preventDefault();
    }
  }

  private stepTab(direction: 1 | -1): void {
    if (this.tabs.length < 2) return;
    const index = this.tabs.findIndex((t) => t.id === this.activeId);
    const next = this.tabs[(index + direction + this.tabs.length) % this.tabs.length];
    this.activate(next.id);
  }
}

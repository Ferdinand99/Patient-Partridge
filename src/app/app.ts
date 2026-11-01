// The app: the toolbar, opening files, and showing one document at a time.
// Anything that touches the outside world (the engine, the file dialog, the window title) comes
// in through `AppDeps`, so tests can run the whole flow with fakes.

import { describeError, fileNameOf, toAppError, type AppErrorLike } from "../lib/errors";
import type { OpenedDocument } from "../lib/api";
import { parsePageInput } from "../lib/pageInput";
import { actionForKey } from "../viewer/keys";
import { DocumentView } from "../viewer/viewer";
import { zoomLabel, type ZoomMode } from "../viewer/zoom";
import { showNotice } from "./notice";

export interface AppDeps {
  api: {
    open(path: string, password?: string): Promise<OpenedDocument>;
    render(doc: number, page: number, width: number, height: number): Promise<ArrayBuffer>;
    close(doc: number): Promise<void>;
    measureMode(): Promise<boolean>;
    measureReport(openMs: number, firstPageMs: number, pages: number): Promise<void>;
  };
  /** Shows the file dialog. Resolves with a path, or null when the user cancelled. */
  pickFile(): Promise<string | null>;
  setWindowTitle(title: string): void;
  now(): number;
}

interface CurrentDocument {
  doc: number;
  path: string;
  name: string;
  view: DocumentView;
}

const ZOOM_CHOICES = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4];

function byId<T extends HTMLElement>(root: Document, id: string): T {
  const el = root.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
}

export class App {
  private current: CurrentDocument | null = null;
  private openToken = 0;
  private measuring = false;

  private readonly stage: HTMLElement;
  private readonly empty: HTMLElement;
  private readonly dropOverlay: HTMLElement;
  private readonly noticeLayer: HTMLElement;
  private readonly pageInput: HTMLInputElement;
  private readonly pageTotal: HTMLElement;
  private readonly zoomSelect: HTMLSelectElement;
  private readonly zoomIn: HTMLButtonElement;
  private readonly zoomOut: HTMLButtonElement;

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
  }

  init(): void {
    byId(this.root, "open-button").addEventListener("click", () => void this.openDialog());
    byId(this.root, "empty-open").addEventListener("click", () => void this.openDialog());
    this.zoomIn.addEventListener("click", () => this.current?.view.stepZoom(1));
    this.zoomOut.addEventListener("click", () => this.current?.view.stepZoom(-1));
    this.zoomSelect.addEventListener("change", () => this.onZoomSelect());
    this.pageInput.addEventListener("keydown", (e) => this.onPageInputKey(e));
    this.pageInput.addEventListener("blur", () => this.showCurrentPage());
    this.root.addEventListener("keydown", (e) => this.onKeyDown(e));
    this.buildZoomOptions(null);
    void this.deps.api.measureMode().then((on) => (this.measuring = on));
  }

  /** The document that is open, if any. */
  get currentView(): DocumentView | null {
    return this.current?.view ?? null;
  }

  setDropActive(active: boolean): void {
    this.dropOverlay.hidden = !active;
  }

  async openDialog(): Promise<void> {
    const path = await this.deps.pickFile();
    if (path) await this.openPath(path);
  }

  /** Opens a file. If it needs a password the user is asked for one. */
  async openPath(path: string, password?: string): Promise<void> {
    const token = ++this.openToken;
    const name = fileNameOf(path);
    const started = this.deps.now();
    let opened: OpenedDocument;
    try {
      opened = await this.deps.api.open(path, password);
    } catch (e) {
      if (token !== this.openToken) return;
      await this.openFailed(path, name, toAppError(e));
      return;
    }
    if (token !== this.openToken) {
      // Another file was opened while this one was loading: this one is not wanted any more.
      void this.deps.api.close(opened.doc).catch(() => undefined);
      return;
    }
    this.showDocument(opened, path, started);
  }

  private async openFailed(path: string, name: string, error: AppErrorLike): Promise<void> {
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
        await this.openPath(path, answer.password);
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
      if (answer.button === "retry") await this.openPath(path);
      return;
    }
    await showNotice(this.noticeLayer, {
      title: text.title,
      detail: text.detail,
      buttons: [{ id: "ok", label: "OK", primary: true }],
    });
  }

  private showDocument(opened: OpenedDocument, path: string, started: number): void {
    this.closeCurrent();
    const openedAfter = this.deps.now() - started;

    const view = new DocumentView(
      { doc: opened.doc, pages: opened.pages },
      this.deps.api,
      {
        onPageChange: (page) => {
          if (this.current?.view === view) this.showCurrentPage(page);
        },
        onZoomChange: (zoom, mode) => {
          if (this.current?.view === view) this.showZoom(zoom, mode);
        },
        onFatal: (error) => void this.documentLost(view, error),
        onFirstPaint: () => {
          if (!this.measuring) return;
          const paintedAfter = this.deps.now() - started;
          void this.deps.api.measureReport(openedAfter, paintedAfter, opened.pages.length);
        },
      },
    );

    this.current = { doc: opened.doc, path, name: opened.name, view };
    this.empty.hidden = true;
    this.stage.append(view.element);
    this.setControlsEnabled(true);
    this.pageTotal.textContent = `/ ${opened.pages.length}`;
    this.deps.setWindowTitle(`${opened.name} - Patient Partridge`);
    view.start();
    if (opened.pages.length === 0) {
      void showNotice(this.noticeLayer, {
        title: "This PDF has no pages",
        detail: `"${opened.name}" opened, but it does not contain any pages to show.`,
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
    }
  }

  private closeCurrent(): void {
    const current = this.current;
    if (!current) return;
    this.current = null;
    current.view.destroy();
    void this.deps.api.close(current.doc).catch(() => undefined);
  }

  /** The engine stopped and took the document with it. The app itself carries on. */
  private async documentLost(view: DocumentView, error: AppErrorLike): Promise<void> {
    const lost = this.current;
    if (!lost || lost.view !== view) return;
    this.current = null;
    view.destroy();
    this.empty.hidden = false;
    this.setControlsEnabled(false);
    this.pageInput.value = "";
    this.pageTotal.textContent = "";
    this.deps.setWindowTitle("Patient Partridge");
    const text = describeError(error, lost.name);
    const answer = await showNotice(this.noticeLayer, {
      title: text.title,
      detail: text.detail,
      buttons: [
        { id: "close", label: "Close" },
        { id: "retry", label: "Open the file again", primary: true },
      ],
    });
    if (answer.button === "retry") await this.openPath(lost.path);
  }

  // ---- toolbar ---------------------------------------------------------------

  private setControlsEnabled(on: boolean): void {
    for (const el of [this.pageInput, this.zoomSelect, this.zoomIn, this.zoomOut]) el.disabled = !on;
  }

  private showCurrentPage(page?: number): void {
    const current = this.current;
    if (!current) return;
    // Do not overwrite what the user is typing in the box.
    if (page === undefined || this.root.activeElement !== this.pageInput) {
      this.pageInput.value = String(
        (page ?? current.view.currentPageIndex) + (current.view.pageCount === 0 ? 0 : 1),
      );
    }
  }

  private onPageInputKey(e: KeyboardEvent): void {
    const view = this.current?.view;
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
    const view = this.current?.view;
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
    } else if (this.current?.view.handleAction(action)) {
      e.preventDefault();
    }
  }
}

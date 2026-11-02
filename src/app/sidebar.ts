// The side panel: page thumbnails and the table of contents of the document in front.
//
// It does no work while it is closed. The thumbnails are only drawn while the Pages panel is
// showing and the main view has drawn its first page; the table of contents is asked from the
// engine the first time the Contents panel is shown for a document.

import type { StorageLike } from "../lib/theme";
import type { PageSizePt } from "../viewer/layout";
import { OutlineView, type OutlineEntry } from "./outline";
import type { PagePanel } from "./pagepanel";
import { ThumbnailList, type ThumbApi, type ThumbOptions } from "./thumbnails";

export type Panel = "pages" | "contents";

export const SIDEBAR_KEY = "partridge.sidebar";

export interface SidebarState {
  open: boolean;
  panel: Panel;
}

export function parseSidebarState(value: string | null | undefined): SidebarState {
  try {
    const raw = JSON.parse(value ?? "null") as { open?: unknown; panel?: unknown } | null;
    return {
      open: raw?.open === true,
      panel: raw?.panel === "contents" ? "contents" : "pages",
    };
  } catch {
    return { open: false, panel: "pages" };
  }
}

/** What the side panel needs to know about the document in front. */
export interface SidebarDocument {
  doc: number;
  pages: PageSizePt[];
  goToPage(index: number): void;
  /** True once the main view has drawn its first page. */
  hasPainted(): boolean;
  currentPage(): number;
}

export interface SidebarDeps {
  api: ThumbApi & { getOutline(doc: number): Promise<OutlineEntry[]> };
  storage: StorageLike | null;
  thumbs?: ThumbOptions;
  /** The page tools on top of the thumbnails. Without it the thumbnails only go to a page. */
  pagePanel?: PagePanel;
}

type OutlineCache =
  | { kind: "loading" }
  | { kind: "error" }
  | { kind: "ready"; items: OutlineEntry[] };

function byId<T extends HTMLElement>(root: Document, id: string): T {
  const el = root.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
}

export class Sidebar {
  private state: SidebarState;
  private document: SidebarDocument | null = null;
  private thumbs: ThumbnailList | null = null;
  private readonly outlineView: OutlineView;
  /** The table of contents of each document asked so far. */
  private readonly outlines = new Map<number, OutlineCache>();
  /** Where each document's thumbnail list was scrolled to. */
  private readonly scrolled = new Map<number, number>();

  private readonly aside: HTMLElement;
  private readonly toggle: HTMLElement;
  private readonly thumbsRoot: HTMLElement;
  private readonly outlineRoot: HTMLElement;
  private readonly tabs: HTMLButtonElement[];

  constructor(
    root: Document,
    private readonly deps: SidebarDeps,
  ) {
    this.aside = byId(root, "sidebar");
    this.toggle = byId(root, "sidebar-toggle");
    this.thumbsRoot = byId(root, "thumbs");
    this.outlineRoot = byId(root, "outline");
    this.tabs = [...this.aside.querySelectorAll<HTMLButtonElement>("[data-panel]")];
    this.state = parseSidebarState(deps.storage?.getItem(SIDEBAR_KEY));
    this.outlineView = new OutlineView(this.outlineRoot, (page) => this.document?.goToPage(page));

    this.toggle.addEventListener("click", () => this.setOpen(!this.state.open));
    for (const tab of this.tabs) {
      tab.addEventListener("click", () => this.setPanel(tab.dataset.panel === "contents" ? "contents" : "pages"));
    }
    this.aside.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && this.aside.contains(root.activeElement)) {
        this.setOpen(false);
        this.toggle.focus();
      }
    });
    this.apply();
  }

  get isOpen(): boolean {
    return this.state.open;
  }

  get panel(): Panel {
    return this.state.panel;
  }

  setOpen(open: boolean): void {
    if (open === this.state.open) return;
    this.state = { ...this.state, open };
    this.save();
    this.apply();
  }

  setPanel(panel: Panel): void {
    if (panel === this.state.panel) {
      if (!this.state.open) this.setOpen(true);
      return;
    }
    this.state = { open: true, panel };
    this.save();
    this.apply();
  }

  /** The document in front changed, or there is none. */
  setDocument(document: SidebarDocument | null): void {
    if (this.document === document) return;
    this.dropThumbs();
    this.document = document;
    this.apply();
  }

  /** The page in view in the main view changed. */
  setCurrentPage(page: number): void {
    this.thumbs?.setCurrent(page);
    this.outlineView.setPage(page);
  }

  /** The main view has drawn its first page: thumbnails may start now. */
  notifyPainted(): void {
    this.thumbs?.setEnabled(this.thumbsWanted());
  }

  /** The pages of a document changed (a page tool, taking a change back): the thumbnails are made again. */
  pagesChanged(doc: number): void {
    this.deps.pagePanel?.pagesChanged(doc);
    this.outlines.delete(doc);
    if (this.document?.doc !== doc) return;
    this.dropThumbs();
    this.apply();
  }

  /** Forgets what is kept for a document that was closed. */
  forget(doc: number): void {
    this.outlines.delete(doc);
    this.scrolled.delete(doc);
    this.deps.pagePanel?.forget(doc);
  }

  private thumbsWanted(): boolean {
    return this.state.open && this.state.panel === "pages" && (this.document?.hasPainted() ?? false);
  }

  private save(): void {
    try {
      this.deps.storage?.setItem(SIDEBAR_KEY, JSON.stringify(this.state));
    } catch {
      // The choice then lasts until the app closes.
    }
  }

  private dropThumbs(): void {
    if (!this.thumbs) return;
    if (this.document) this.scrolled.set(this.document.doc, this.thumbs.scrollTop);
    this.deps.pagePanel?.detach();
    this.thumbs.destroy();
    this.thumbs = null;
  }

  /** Brings the panel in step with the state and the document. */
  private apply(): void {
    const { open, panel } = this.state;
    this.aside.hidden = !open;
    this.toggle.setAttribute("aria-pressed", String(open));
    this.toggle.title = open ? "Hide the side panel (Ctrl+B)" : "Show the side panel (Ctrl+B)";
    for (const tab of this.tabs) {
      tab.setAttribute("aria-selected", String(tab.dataset.panel === panel));
    }
    this.thumbsRoot.hidden = !open || panel !== "pages";
    this.outlineRoot.hidden = !open || panel !== "contents";

    const document = this.document;
    this.deps.pagePanel?.showTools(open && panel === "pages" && document !== null);
    if (!open || !document) {
      // Closed: nothing is built, nothing is drawn, nothing is asked from the engine.
      this.dropThumbs();
      return;
    }
    if (panel === "pages") {
      if (!this.thumbs) {
        const panel = this.deps.pagePanel;
        this.thumbs = new ThumbnailList(
          this.thumbsRoot,
          document.doc,
          document.pages,
          this.deps.api,
          (page) => document.goToPage(page),
          // A click that picks pages (Ctrl, Shift, the end of a drag) stays in the panel.
          panel
            ? { ...this.deps.thumbs, decorate: panel.decorate, pick: (p, e) => panel.select(p, e) }
            : this.deps.thumbs,
        );
        panel?.attach(this.thumbs, this.thumbsRoot, document.doc, document.pages);
        const saved = this.scrolled.get(document.doc);
        // A list the reader scrolled stays where it was; a new one shows the page in view.
        if (saved !== undefined) this.thumbs.scrollTop = saved;
        this.thumbs.setCurrent(document.currentPage(), saved === undefined);
      }
      this.thumbs.setEnabled(this.thumbsWanted());
    } else {
      this.dropThumbs();
      this.showOutline(document);
    }
  }

  private showOutline(document: SidebarDocument): void {
    this.outlineView.setPage(document.currentPage());
    const cached = this.outlines.get(document.doc);
    if (cached) {
      this.outlineView.show(cached);
      return;
    }
    this.outlines.set(document.doc, { kind: "loading" });
    this.outlineView.show({ kind: "loading" });
    this.deps.api
      .getOutline(document.doc)
      .then((items) => ({ kind: "ready", items }) as const)
      // A document that is gone is dealt with by the main view; here it is just an error line.
      .catch(() => ({ kind: "error" }) as const)
      .then((result) => {
        // An error is not kept, so showing the panel again tries again.
        if (result.kind === "ready") this.outlines.set(document.doc, result);
        else this.outlines.delete(document.doc);
        // Only draw it if this document is still the one in front and the panel is showing.
        if (this.document?.doc === document.doc && this.state.open && this.state.panel === "contents") {
          this.outlineView.setPage(document.currentPage());
          this.outlineView.show(result);
        }
      });
  }
}

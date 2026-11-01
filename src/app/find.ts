// The find bar: type a text, see how many matches there are, highlight them on the pages and move
// from one to the next. Each open document keeps its own search, so switching tabs does not lose
// it.

import { toAppError, type AppErrorLike } from "../lib/errors";
import { MatchCursor, SearchSession, cleanQuery, type PageSearch, type SearchLimit } from "../viewer/search";
import type { DocumentView } from "../viewer/viewer";

/** The document a search runs on. */
export interface FindTarget {
  doc: number;
  view: DocumentView;
}

export interface FindDeps {
  searchPage(doc: number, page: number, query: string): ReturnType<PageSearch>;
  /** The search could not go on because the engine of the document is gone. */
  onEngineLost(target: FindTarget, error: AppErrorLike): void;
  /** How long typing must rest before a search starts, in milliseconds. */
  debounceMs?: number;
}

interface State {
  /** What is in the box for this document. */
  text: string;
  session: SearchSession | null;
  cursor: MatchCursor;
  /** The screen update that is waiting for this document. */
  frame: ReturnType<typeof setTimeout> | undefined;
}

export interface StatusInput {
  query: string | null;
  running: boolean;
  failed: boolean;
  total: number;
  position: number;
  searched: number;
  pageCount: number;
  /** What cut the search short, if anything. */
  limit?: SearchLimit;
}

/** The short line next to the box: how far the search is, or which match is shown. */
export function findStatus(s: StatusInput): string {
  if (s.query === null) return "";
  if (s.failed) return "The search stopped";
  if (s.running) {
    const progress = s.pageCount > 40 ? ` (page ${s.searched} of ${s.pageCount})` : "";
    return s.total === 0 ? `Searching…${progress}` : `${s.position || 1} of ${s.total}+${progress}`;
  }
  if (s.total === 0) return "No matches";
  if (s.limit === "total") {
    return `${s.position || 1} of ${s.total.toLocaleString("en-US")}+ (stopped at the limit)`;
  }
  if (s.limit === "page") {
    return `${s.position || 1} of ${s.total} (some pages have more matches than are shown)`;
  }
  return `${s.position || 1} of ${s.total}`;
}

function byId<T extends HTMLElement>(root: Document, id: string): T {
  const el = root.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
}

export class FindBar {
  private readonly bar: HTMLElement;
  private readonly input: HTMLInputElement;
  private readonly status: HTMLElement;
  private readonly toggle: HTMLElement;
  private readonly states = new Map<DocumentView, State>();
  private target: FindTarget | null = null;
  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    root: Document,
    private readonly deps: FindDeps,
  ) {
    this.bar = byId(root, "findbar");
    this.input = byId(root, "find-input");
    this.status = byId(root, "find-status");
    this.toggle = byId(root, "find-button");

    this.toggle.addEventListener("click", () => (this.isOpen ? this.close() : this.open()));
    byId(root, "find-next").addEventListener("click", () => this.step(1));
    byId(root, "find-prev").addEventListener("click", () => this.step(-1));
    byId(root, "find-close").addEventListener("click", () => this.close());
    this.input.addEventListener("input", () => {
      clearTimeout(this.timer);
      this.timer = setTimeout(() => this.search(), this.deps.debounceMs ?? 250);
    });
    this.input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        // A search that is waiting for the typing to rest starts now.
        if (this.timer !== undefined) {
          clearTimeout(this.timer);
          this.timer = undefined;
          this.search();
        } else {
          this.step(e.shiftKey ? -1 : 1);
        }
      } else if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.close();
      }
    });
  }

  get isOpen(): boolean {
    return !this.bar.hidden;
  }

  /** Shows the bar with the focus in the box. A text already in the box is searched for. */
  open(): void {
    if (!this.target) return;
    this.bar.hidden = false;
    this.toggle.setAttribute("aria-pressed", "true");
    this.input.focus();
    this.input.select();
    const state = this.stateOf(this.target);
    if (this.input.value !== "" && state.session === null) this.search();
  }

  /** Hides the bar, stops the search and takes the highlights off the pages. */
  close(): void {
    clearTimeout(this.timer);
    this.timer = undefined;
    this.bar.hidden = true;
    this.toggle.setAttribute("aria-pressed", "false");
    if (this.target) {
      this.reset(this.target.view);
      this.target.view.element.focus({ preventScroll: true });
    }
  }

  /** The document in front changed, or there is none. */
  setTarget(target: FindTarget | null): void {
    if (this.target?.view === target?.view) {
      this.target = target;
      return;
    }
    clearTimeout(this.timer);
    this.timer = undefined;
    this.target = target;
    this.toggle.toggleAttribute("disabled", target === null);
    if (!target) {
      if (this.isOpen) this.bar.hidden = true;
      this.toggle.setAttribute("aria-pressed", "false");
      return;
    }
    if (this.isOpen) {
      const state = this.stateOf(target);
      this.input.value = state.text;
      this.showStatus();
    } else {
      this.reset(target.view);
    }
  }

  /** A document was closed: its search goes with it. */
  forget(view: DocumentView): void {
    const state = this.states.get(view);
    state?.session?.cancel();
    clearTimeout(state?.frame);
    this.states.delete(view);
  }

  /** The next match (F3), also when the bar is closed but a search is still there. */
  step(direction: 1 | -1): void {
    const target = this.target;
    if (!target) return;
    const state = this.stateOf(target);
    if (!state.session) {
      if (!this.isOpen) this.open();
      return;
    }
    const match = direction === 1 ? state.cursor.next() : state.cursor.previous();
    if (match) {
      this.showMatches(target, state);
      void target.view.revealMatch(match);
    }
    this.showStatus();
  }

  private stateOf(target: FindTarget): State {
    let state = this.states.get(target.view);
    if (!state) {
      const created: State = {
        text: "",
        session: null,
        cursor: new MatchCursor(() => this.states.get(target.view)?.session ?? null),
        frame: undefined,
      };
      this.states.set(target.view, created);
      state = created;
    }
    return state;
  }

  private reset(view: DocumentView): void {
    const state = this.states.get(view);
    if (state) {
      state.session?.cancel();
      state.session = null;
      state.cursor.clear();
    }
    view.clearFind();
    this.showStatus();
  }

  /** Starts a search for what is in the box, on the document in front. */
  private search(): void {
    this.timer = undefined;
    const target = this.target;
    if (!target) return;
    const state = this.stateOf(target);
    state.text = this.input.value;
    state.session?.cancel();
    state.cursor.clear();
    const query = cleanQuery(this.input.value);
    if (query === null) {
      state.session = null;
      target.view.clearFind();
      this.showStatus();
      return;
    }
    const session = new SearchSession({
      query,
      pageCount: target.view.pageCount,
      startPage: target.view.currentPageIndex,
      search: (page, text) => this.deps.searchPage(target.doc, page, text),
      onUpdate: () => this.scheduleUpdate(target, session),
      onError: (error) => {
        const app = toAppError(error);
        this.showStatus();
        if (app.code === "engine_stopped" || app.code === "unknown_document") {
          this.deps.onEngineLost(target, app);
        }
      },
    });
    state.session = session;
    this.showStatus();
    void session.run();
  }

  /**
   * The pages come in quickly; the screen is brought up to date at most every 60 ms. Each document
   * has its own wait, so an update for one never takes the place of an update for another.
   */
  private scheduleUpdate(target: FindTarget, session: SearchSession): void {
    const waiting = this.states.get(target.view);
    if (!waiting || waiting.frame !== undefined) return;
    waiting.frame = setTimeout(() => {
      waiting.frame = undefined;
      const state = this.states.get(target.view);
      if (!state || state.session !== session) return;
      let revealed: ReturnType<MatchCursor["start"]> = null;
      if (state.cursor.current === null && session.matches.length > 0) {
        revealed = state.cursor.start(target.view.currentPageIndex);
      }
      this.showMatches(target, state);
      if (revealed) void target.view.revealMatch(revealed);
      if (this.target?.view === target.view) this.showStatus();
    }, 60);
  }

  private showMatches(target: FindTarget, state: State): void {
    if (!state.session) return;
    target.view.setFind(state.session.foundPages(), state.cursor.current);
  }

  private showStatus(): void {
    const target = this.target;
    const state = target ? this.states.get(target.view) : undefined;
    const session = state?.session ?? null;
    this.status.textContent = findStatus({
      query: session ? session.query : null,
      running: session?.running ?? false,
      failed: session?.hasFailed ?? false,
      total: state?.cursor.total ?? 0,
      position: state?.cursor.position ?? 0,
      searched: session?.pagesSearched ?? 0,
      pageCount: session?.pageCount ?? 0,
      limit: session?.limit ?? null,
    });
  }
}

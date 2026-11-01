// Search in a document: ask the engine for one page at a time, keep the matches in reading order,
// and move from one match to the next. The search runs page by page so it can be stopped at any
// moment and shows what it has found so far.

export interface SearchMatch {
  page: number;
  /** Where the match starts, in characters of the page text. */
  start: number;
  /** How many characters it covers. */
  len: number;
}

/** Longest search text, in characters. The engine takes at most 256 bytes. */
export const MAX_QUERY_CHARS = 64;

/** Asks the engine for the matches on one page: pairs of start and length. */
export type PageSearch = (page: number, query: string) => Promise<[number, number][]>;

export interface SearchOptions {
  query: string;
  pageCount: number;
  /** The page the reader is on. Pages are searched from here to the end, then from the start. */
  startPage: number;
  search: PageSearch;
  /** Something new was found, or the search moved on. */
  onUpdate(): void;
  /** The search could not go on (the engine stopped, for example). */
  onError(error: unknown): void;
}

/** Trims a search text and cuts it to what is allowed. Returns null when nothing is left. */
export function cleanQuery(text: string): string | null {
  const query = Array.from(text.replace(/\s+/g, " ").trim()).slice(0, MAX_QUERY_CHARS).join("");
  return query.length > 0 ? query : null;
}

export class SearchSession {
  readonly query: string;
  private readonly found = new Map<number, [number, number][]>();
  private searched = 0;
  private cancelled = false;
  private finished = false;
  private failed = false;
  private ordered: SearchMatch[] | null = null;

  constructor(private readonly options: SearchOptions) {
    this.query = options.query;
  }

  get pageCount(): number {
    return this.options.pageCount;
  }

  /** How many pages have been searched so far. */
  get pagesSearched(): number {
    return this.searched;
  }

  /** True when every page has been searched. */
  get done(): boolean {
    return this.finished;
  }

  get hasFailed(): boolean {
    return this.failed;
  }

  get running(): boolean {
    return !this.finished && !this.cancelled && !this.failed;
  }

  /** The matches found so far, in reading order. */
  get matches(): readonly SearchMatch[] {
    if (!this.ordered) {
      const list: SearchMatch[] = [];
      const pages = [...this.found.keys()].sort((a, b) => a - b);
      for (const page of pages) {
        for (const [start, len] of this.found.get(page) ?? []) list.push({ page, start, len });
      }
      this.ordered = list;
    }
    return this.ordered;
  }

  /** Every page that has matches, with its pairs of start and length. Not a copy: do not change it. */
  foundPages(): ReadonlyMap<number, readonly [number, number][]> {
    return this.found;
  }

  /** The matches on one page, as pairs of start and length. */
  onPage(page: number): readonly [number, number][] {
    return this.found.get(page) ?? [];
  }

  /** Runs the search. Resolves when every page is searched, or the search was stopped. */
  async run(): Promise<void> {
    const { pageCount, startPage } = this.options;
    const first = Math.min(Math.max(startPage, 0), Math.max(pageCount - 1, 0));
    for (let step = 0; step < pageCount; step++) {
      if (this.cancelled) return;
      const page = (first + step) % pageCount;
      try {
        const found = await this.options.search(page, this.query);
        if (this.cancelled) return;
        if (found.length > 0) {
          this.found.set(page, found);
          this.ordered = null;
        }
      } catch (error) {
        if (this.cancelled) return;
        this.failed = true;
        this.options.onError(error);
        return;
      }
      this.searched += 1;
      this.options.onUpdate();
    }
    this.finished = true;
    this.options.onUpdate();
  }

  /** Stops the search. What it found stays. */
  cancel(): void {
    this.cancelled = true;
  }
}

/** Moves through the matches of a search: the one that is shown now, the next, the one before. */
export class MatchCursor {
  current: SearchMatch | null = null;

  constructor(private readonly session: () => SearchSession | null) {}

  private list(): readonly SearchMatch[] {
    return this.session()?.matches ?? [];
  }

  /** 1-based number of the current match among those found so far, or 0. */
  get position(): number {
    const index = this.indexOfCurrent();
    return index < 0 ? 0 : index + 1;
  }

  get total(): number {
    return this.list().length;
  }

  /** Picks the first match at or after a page, or the first one overall when there is none there. */
  start(fromPage: number): SearchMatch | null {
    const list = this.list();
    this.current = list.find((m) => m.page >= fromPage) ?? list[0] ?? null;
    return this.current;
  }

  /** The match after the current one; after the last one, the first. */
  next(): SearchMatch | null {
    const list = this.list();
    if (list.length === 0) return (this.current = null);
    const index = this.indexOfCurrent();
    this.current = list[index < 0 ? 0 : (index + 1) % list.length];
    return this.current;
  }

  /** The match before the current one; before the first one, the last. */
  previous(): SearchMatch | null {
    const list = this.list();
    if (list.length === 0) return (this.current = null);
    const index = this.indexOfCurrent();
    this.current = list[index <= 0 ? list.length - 1 : index - 1];
    return this.current;
  }

  clear(): void {
    this.current = null;
  }

  private indexOfCurrent(): number {
    const c = this.current;
    if (!c) return -1;
    return this.list().findIndex((m) => m.page === c.page && m.start === c.start);
  }
}

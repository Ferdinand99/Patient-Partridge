// The table of contents in the side panel. The engine sends it as a flat list in reading order,
// each entry with its depth. Titles come from the PDF and are only ever shown as text.

import type { OutlineEntry } from "../lib/api";
import { icon } from "../lib/icons";

export type { OutlineEntry };

/** For each entry: whether the next entry is deeper, i.e. whether it has children. */
export function hasChildren(items: OutlineEntry[]): boolean[] {
  return items.map((item, i) => i + 1 < items.length && items[i + 1].depth > item.depth);
}

/**
 * The entries that are visible when the entries in `collapsed` are closed: an entry is hidden
 * when any entry above it, at a lower depth and still in reach, is closed.
 */
export function visibleEntries(items: OutlineEntry[], collapsed: ReadonlySet<number>): number[] {
  const out: number[] = [];
  // Depth below which entries are hidden, or Infinity when nothing is closed.
  let hiddenBelow = Infinity;
  for (let i = 0; i < items.length; i++) {
    const depth = items[i].depth;
    if (depth > hiddenBelow) continue;
    hiddenBelow = Infinity;
    out.push(i);
    if (collapsed.has(i)) hiddenBelow = depth;
  }
  return out;
}

/** Which entries start closed: with many entries, everything that has children. */
export function initiallyCollapsed(items: OutlineEntry[]): Set<number> {
  const parents = hasChildren(items);
  const collapsed = new Set<number>();
  if (items.length <= 60) return collapsed;
  items.forEach((item, i) => {
    if (parents[i] && item.depth >= 1) collapsed.add(i);
  });
  return collapsed;
}

/**
 * The entry that holds the reader's place: among the entries that lead to a page at or before
 * `page`, the one with the highest page; the later one when several lead to the same page.
 */
export function entryForPage(items: OutlineEntry[], page: number): number {
  let best = -1;
  let bestPage = -1;
  items.forEach((item, i) => {
    if (item.page !== null && item.page <= page && item.page >= bestPage) {
      best = i;
      bestPage = item.page;
    }
  });
  return best;
}

export type OutlineState =
  | { kind: "loading" }
  | { kind: "error" }
  | { kind: "ready"; items: OutlineEntry[] };

export class OutlineView {
  private items: OutlineEntry[] = [];
  private collapsed = new Set<number>();
  private current = -1;
  private page = 0;

  constructor(
    private readonly root: HTMLElement,
    private readonly onSelect: (page: number) => void,
  ) {}

  show(state: OutlineState): void {
    if (state.kind === "loading") {
      this.message("Reading the table of contents…");
    } else if (state.kind === "error") {
      this.message("The table of contents could not be read.");
    } else if (state.items.length === 0) {
      this.message("This document has no table of contents.");
    } else {
      this.items = state.items;
      this.collapsed = initiallyCollapsed(state.items);
      this.current = entryForPage(state.items, this.page);
      this.revealCurrent();
      this.render();
    }
  }

  /** Marks the entry that holds the reader's place, and opens the groups above it. */
  setPage(page: number): void {
    this.page = page;
    if (this.items.length === 0) return;
    const next = entryForPage(this.items, page);
    if (next === this.current) return;
    this.current = next;
    this.revealCurrent();
    this.render();
  }

  /** Opens every group the entry of the reader's place sits in, so that it can be seen. */
  private revealCurrent(): void {
    if (this.current < 0) return;
    let depth = this.items[this.current].depth;
    for (let i = this.current - 1; i >= 0 && depth > 0; i--) {
      if (this.items[i].depth < depth) {
        this.collapsed.delete(i);
        depth = this.items[i].depth;
      }
    }
  }

  private message(text: string): void {
    this.items = [];
    const p = document.createElement("p");
    p.className = "side-message";
    p.textContent = text;
    this.root.replaceChildren(p);
  }

  private render(): void {
    const parents = hasChildren(this.items);
    const list = document.createElement("ul");
    list.className = "outline-list";
    list.setAttribute("role", "list");
    for (const i of visibleEntries(this.items, this.collapsed)) {
      list.append(this.row(i, parents[i]));
    }
    this.root.replaceChildren(list);
  }

  private row(index: number, parent: boolean): HTMLElement {
    const item = this.items[index];
    const li = document.createElement("li");
    li.className = "outline-row";
    li.style.setProperty("--depth", String(item.depth));

    if (parent) {
      const open = !this.collapsed.has(index);
      const toggle = document.createElement("button");
      toggle.type = "button";
      toggle.className = "outline-toggle";
      toggle.setAttribute("aria-expanded", String(open));
      toggle.setAttribute("aria-label", `${open ? "Close" : "Open"} ${item.title || "this group"}`);
      toggle.append(icon(open ? "chevron-down" : "chevron-right"));
      toggle.addEventListener("click", () => {
        if (this.collapsed.has(index)) this.collapsed.delete(index);
        else this.collapsed.add(index);
        this.render();
      });
      li.append(toggle);
    } else {
      const spacer = document.createElement("span");
      spacer.className = "outline-toggle";
      li.append(spacer);
    }

    const page = item.page;
    const title = item.title || "Untitled";
    if (page === null) {
      // An entry that leads nowhere in this document (a web address, a file) is not followed.
      const text = document.createElement("span");
      text.className = "outline-text muted";
      text.textContent = title;
      li.append(text);
    } else {
      const link = document.createElement("button");
      link.type = "button";
      link.className = "outline-link";
      link.textContent = title;
      link.title = `${title} (page ${page + 1})`;
      if (index === this.current) link.setAttribute("aria-current", "location");
      link.addEventListener("click", () => this.onSelect(page));
      li.append(link);
    }
    return li;
  }
}

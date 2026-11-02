// The tab strip: one tab for each open document. The strip only draws; the app decides what
// selecting or closing a tab does.

import { icon } from "../lib/icons";

export interface TabInfo {
  id: number;
  title: string;
  /** Shown as a tooltip: the full path. */
  path: string;
  /** The document has changes that are not saved: a dot is shown next to the name. */
  dirty?: boolean;
}

export interface TabHandlers {
  select(id: number): void;
  close(id: number): void;
}

export class TabStrip {
  private tabs: TabInfo[] = [];
  private activeId: number | null = null;

  constructor(
    private readonly root: HTMLElement,
    private readonly handlers: TabHandlers,
  ) {
    root.addEventListener("keydown", (e) => this.onKeyDown(e));
    // Middle click closes a tab, as in a browser.
    root.addEventListener("auxclick", (e) => {
      if (e.button !== 1) return;
      const id = idOf(e.target);
      if (id !== null) {
        e.preventDefault();
        this.handlers.close(id);
      }
    });
  }

  render(tabs: TabInfo[], activeId: number | null): void {
    this.tabs = tabs;
    this.activeId = activeId;
    const hadFocus = this.root.contains(document.activeElement);
    this.root.replaceChildren(...tabs.map((tab) => this.build(tab, tab.id === activeId)));
    if (hadFocus) this.focusActive();
  }

  focusActive(): void {
    this.root.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]')?.focus();
  }

  private build(tab: TabInfo, selected: boolean): HTMLElement {
    const item = document.createElement("div");
    item.className = "tab";
    item.dataset.id = String(tab.id);
    item.dataset.selected = String(selected);
    item.setAttribute("role", "presentation");

    const label = document.createElement("button");
    label.type = "button";
    label.className = "tab-title";
    label.setAttribute("role", "tab");
    label.setAttribute("aria-selected", String(selected));
    label.tabIndex = selected ? 0 : -1;
    label.title = tab.path;
    label.textContent = tab.title;
    label.addEventListener("click", () => this.handlers.select(tab.id));

    // The dot is not part of the title, so the title (and the label of the button) stay the name.
    let dot: HTMLElement | null = null;
    if (tab.dirty) {
      dot = document.createElement("span");
      dot.className = "tab-dirty";
      dot.setAttribute("role", "img");
      dot.setAttribute("aria-label", "Unsaved changes");
      dot.title = "Unsaved changes";
    }

    const close = document.createElement("button");
    close.type = "button";
    close.className = "tab-close";
    close.tabIndex = -1;
    close.setAttribute("aria-label", `Close ${tab.title}`);
    close.title = "Close (Ctrl+W)";
    close.append(icon("x"));
    close.addEventListener("click", (e) => {
      e.stopPropagation();
      this.handlers.close(tab.id);
    });

    if (dot) item.append(label, dot, close);
    else item.append(label, close);
    return item;
  }

  /** Left and right move between tabs, Home and End go to the first and last. */
  private onKeyDown(e: KeyboardEvent): void {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (!(e.target instanceof HTMLElement) || e.target.getAttribute("role") !== "tab") return;
    const index = this.tabs.findIndex((t) => t.id === this.activeId);
    if (index < 0) return;
    const last = this.tabs.length - 1;
    let next: number;
    if (e.key === "ArrowRight") next = index === last ? 0 : index + 1;
    else if (e.key === "ArrowLeft") next = index === 0 ? last : index - 1;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = last;
    else return;
    e.preventDefault();
    e.stopPropagation();
    this.handlers.select(this.tabs[next].id);
  }
}

function idOf(target: EventTarget | null): number | null {
  if (!(target instanceof Element)) return null;
  const tab = target.closest<HTMLElement>(".tab");
  const id = Number(tab?.dataset.id);
  return tab && Number.isFinite(id) ? id : null;
}

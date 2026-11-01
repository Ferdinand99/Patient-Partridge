// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  OutlineView,
  entryForPage,
  hasChildren,
  initiallyCollapsed,
  visibleEntries,
  type OutlineEntry,
} from "./outline";

const e = (depth: number, page: number | null, title: string): OutlineEntry => ({ depth, page, title });

const sample: OutlineEntry[] = [
  e(0, 0, "Chapter 1"),
  e(1, 1, "Section 1.1"),
  e(2, 2, "Part 1.1.1"),
  e(1, 4, "Section 1.2"),
  e(0, null, "Web page"),
  e(0, 8, "Chapter 3"),
];

describe("outline helpers", () => {
  it("finds the entries that have children", () => {
    expect(hasChildren(sample)).toEqual([true, true, false, false, false, false]);
    expect(hasChildren([])).toEqual([]);
  });

  it("hides what is under a closed entry, and shows it again when it is open", () => {
    expect(visibleEntries(sample, new Set())).toEqual([0, 1, 2, 3, 4, 5]);
    expect(visibleEntries(sample, new Set([0]))).toEqual([0, 4, 5]);
    expect(visibleEntries(sample, new Set([1]))).toEqual([0, 1, 3, 4, 5]);
    expect(visibleEntries(sample, new Set([0, 1]))).toEqual([0, 4, 5]);
  });

  it("starts with everything open for a short list, and groups closed for a long one", () => {
    expect(initiallyCollapsed(sample).size).toBe(0);
    const long: OutlineEntry[] = [];
    for (let i = 0; i < 40; i++) long.push(e(0, i, `Chapter ${i}`), e(1, i, `Section ${i}`), e(2, i, "Detail"));
    const collapsed = initiallyCollapsed(long);
    // Top-level entries stay open so the reader sees the chapters; deeper groups are closed.
    expect(collapsed.has(0)).toBe(false);
    expect(collapsed.has(1)).toBe(true);
  });

  it("finds the entry that holds the place of the reader", () => {
    expect(entryForPage(sample, 0)).toBe(0);
    expect(entryForPage(sample, 2)).toBe(2);
    expect(entryForPage(sample, 3)).toBe(2);
    expect(entryForPage(sample, 4)).toBe(3);
    expect(entryForPage(sample, 100)).toBe(5);
    expect(entryForPage([e(0, 3, "Later")], 1)).toBe(-1);
    expect(entryForPage([], 0)).toBe(-1);
    // Two entries on one page: the later one.
    expect(entryForPage([e(0, 2, "a"), e(1, 2, "b")], 2)).toBe(1);
  });
});

describe("OutlineView", () => {
  let root: HTMLElement;
  const onSelect = vi.fn();

  beforeEach(() => {
    document.body.innerHTML = '<div id="outline"></div>';
    root = document.getElementById("outline") as HTMLElement;
    onSelect.mockReset();
  });

  const titles = () => [...root.querySelectorAll(".outline-link, .outline-text")].map((x) => x.textContent);

  it("lists the entries and goes to the page of the one that is clicked", () => {
    const view = new OutlineView(root, onSelect);
    view.show({ kind: "ready", items: sample });
    expect(titles()).toEqual(["Chapter 1", "Section 1.1", "Part 1.1.1", "Section 1.2", "Web page", "Chapter 3"]);
    root.querySelectorAll<HTMLElement>(".outline-link")[2].click();
    expect(onSelect).toHaveBeenCalledWith(2);
  });

  it("shows an entry that leads nowhere as plain text that does nothing", () => {
    const view = new OutlineView(root, onSelect);
    view.show({ kind: "ready", items: sample });
    const web = [...root.querySelectorAll(".outline-text")].find((x) => x.textContent === "Web page") as HTMLElement;
    expect(web.tagName).toBe("SPAN");
    web.click();
    expect(onSelect).not.toHaveBeenCalled();
  });

  it("closes and opens a group", () => {
    const view = new OutlineView(root, onSelect);
    view.show({ kind: "ready", items: sample });
    root.querySelector<HTMLElement>('[aria-label="Close Chapter 1"]')?.click();
    expect(titles()).toEqual(["Chapter 1", "Web page", "Chapter 3"]);
    root.querySelector<HTMLElement>('[aria-label="Open Chapter 1"]')?.click();
    expect(titles()).toHaveLength(6);
  });

  it("marks the entry of the reader's place and opens the groups above it", () => {
    const long: OutlineEntry[] = [];
    for (let i = 0; i < 40; i++) long.push(e(0, i * 10, `Chapter ${i}`), e(1, i * 10 + 1, `Section ${i}`), e(2, i * 10 + 2, `Detail ${i}`));
    const view = new OutlineView(root, onSelect);
    view.setPage(302);
    view.show({ kind: "ready", items: long });
    const current = root.querySelector('[aria-current="location"]');
    expect(current?.textContent).toBe("Detail 30");
    view.setPage(382);
    expect(root.querySelector('[aria-current="location"]')?.textContent).toBe("Detail 38");
    // A group above the place that was closed by hand stays closed while the place stays in it.
    root.querySelector<HTMLElement>('[aria-label="Close Section 38"]')?.click();
    expect(root.querySelector('[aria-current="location"]')).toBeNull();
  });

  it("says so when there is nothing, when it is loading and when it failed", () => {
    const view = new OutlineView(root, onSelect);
    view.show({ kind: "loading" });
    expect(root.textContent).toContain("Reading");
    view.show({ kind: "error" });
    expect(root.textContent).toContain("could not be read");
    view.show({ kind: "ready", items: [] });
    expect(root.textContent).toContain("no table of contents");
  });

  it("shows titles only as text, never as markup, and names an empty title", () => {
    const view = new OutlineView(root, onSelect);
    view.show({ kind: "ready", items: [e(0, 0, "<img src=x onerror=alert(1)>"), e(0, 1, "")] });
    expect(root.querySelector("img")).toBeNull();
    expect(titles()).toEqual(["<img src=x onerror=alert(1)>", "Untitled"]);
  });
});

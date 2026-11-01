import { describe, expect, it } from "vitest";
import { LINE_SCROLL, actionForKey, pageScrollDistance, type KeyInfo } from "./keys";

const key = (k: string, extra: Partial<KeyInfo> = {}): KeyInfo => ({
  key: k,
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  ...extra,
});

describe("actionForKey", () => {
  it("scrolls with the arrow keys", () => {
    expect(actionForKey(key("ArrowDown"))).toEqual({ type: "scroll-by", x: 0, y: LINE_SCROLL });
    expect(actionForKey(key("ArrowUp"))).toEqual({ type: "scroll-by", x: 0, y: -LINE_SCROLL });
    expect(actionForKey(key("ArrowRight"))).toEqual({ type: "scroll-by", x: LINE_SCROLL, y: 0 });
    expect(actionForKey(key("ArrowLeft"))).toEqual({ type: "scroll-by", x: -LINE_SCROLL, y: 0 });
  });

  it("scrolls by pages with Page Down, Page Up and Space", () => {
    expect(actionForKey(key("PageDown"))).toEqual({ type: "scroll-page", direction: 1 });
    expect(actionForKey(key("PageUp"))).toEqual({ type: "scroll-page", direction: -1 });
    expect(actionForKey(key(" "))).toEqual({ type: "scroll-page", direction: 1 });
    expect(actionForKey(key(" ", { shiftKey: true }))).toEqual({ type: "scroll-page", direction: -1 });
  });

  it("jumps to the start and the end", () => {
    expect(actionForKey(key("Home"))).toEqual({ type: "go-first" });
    expect(actionForKey(key("End"))).toEqual({ type: "go-last" });
    expect(actionForKey(key("Home", { ctrlKey: true }))).toEqual({ type: "go-first" });
  });

  it("zooms with Ctrl or Cmd", () => {
    for (const mod of [{ ctrlKey: true }, { metaKey: true }]) {
      expect(actionForKey(key("+", mod))).toEqual({ type: "zoom-in" });
      expect(actionForKey(key("=", mod))).toEqual({ type: "zoom-in" });
      expect(actionForKey(key("-", mod))).toEqual({ type: "zoom-out" });
      expect(actionForKey(key("0", mod))).toEqual({ type: "zoom-fit-width" });
      expect(actionForKey(key("1", mod))).toEqual({ type: "zoom-actual" });
    }
  });

  it("has shortcuts for the page box and for opening a file", () => {
    expect(actionForKey(key("g", { ctrlKey: true }))).toEqual({ type: "focus-page-box" });
    expect(actionForKey(key("o", { metaKey: true }))).toEqual({ type: "open-file" });
  });

  it("leaves other keys and Alt combinations alone", () => {
    expect(actionForKey(key("a"))).toBeNull();
    expect(actionForKey(key("c", { ctrlKey: true }))).toBeNull();
    expect(actionForKey(key("ArrowDown", { altKey: true }))).toBeNull();
    expect(actionForKey(key("-"))).toBeNull();
  });
});

describe("pageScrollDistance", () => {
  it("is a bit less than the viewport", () => {
    expect(pageScrollDistance(1000)).toBe(900);
  });

  it("moves at least a line in a tiny viewport", () => {
    expect(pageScrollDistance(10)).toBe(LINE_SCROLL);
  });

  it("closes and switches tabs", () => {
    expect(actionForKey(key("w", { ctrlKey: true }))).toEqual({ type: "close-tab" });
    expect(actionForKey(key("W", { metaKey: true }))).toEqual({ type: "close-tab" });
    expect(actionForKey(key("Tab", { ctrlKey: true }))).toEqual({ type: "next-tab" });
    expect(actionForKey(key("Tab", { ctrlKey: true, shiftKey: true }))).toEqual({ type: "previous-tab" });
    expect(actionForKey(key("PageDown", { ctrlKey: true }))).toEqual({ type: "next-tab" });
    expect(actionForKey(key("PageUp", { ctrlKey: true }))).toEqual({ type: "previous-tab" });
    // Without Ctrl these are still scroll keys, and a plain Tab is left to the browser.
    expect(actionForKey(key("PageDown"))).toEqual({ type: "scroll-page", direction: 1 });
    expect(actionForKey(key("Tab"))).toBeNull();
    expect(actionForKey(key("w"))).toBeNull();
  });

  it("finds, finds again and selects all", () => {
    expect(actionForKey(key("f", { ctrlKey: true }))).toEqual({ type: "find" });
    expect(actionForKey(key("F", { metaKey: true }))).toEqual({ type: "find" });
    expect(actionForKey(key("F3"))).toEqual({ type: "find-next" });
    expect(actionForKey(key("F3", { shiftKey: true }))).toEqual({ type: "find-previous" });
    expect(actionForKey(key("a", { ctrlKey: true }))).toEqual({ type: "select-all" });
    expect(actionForKey(key("f"))).toBeNull();
    expect(actionForKey(key("a"))).toBeNull();
  });

  it("prints with Ctrl+P", () => {
    expect(actionForKey(key("p", { ctrlKey: true }))).toEqual({ type: "print" });
    expect(actionForKey(key("P", { metaKey: true }))).toEqual({ type: "print" });
    expect(actionForKey(key("p"))).toBeNull();
  });

  it("shows and hides the side panel with Ctrl+B", () => {
    expect(actionForKey(key("b", { ctrlKey: true }))).toEqual({ type: "toggle-sidebar" });
    expect(actionForKey(key("B", { metaKey: true }))).toEqual({ type: "toggle-sidebar" });
    expect(actionForKey(key("b"))).toBeNull();
  });
});

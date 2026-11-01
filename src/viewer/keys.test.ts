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
});

// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { blockReload, isReloadKey } from "./reload";

afterEach(() => document.body.replaceChildren());

function press(init: KeyboardEventInit): KeyboardEvent {
  const e = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  document.body.dispatchEvent(e);
  return e;
}

describe("isReloadKey", () => {
  it("knows the keys that reload a web view", () => {
    expect(isReloadKey({ key: "F5", ctrlKey: false, metaKey: false })).toBe(true);
    expect(isReloadKey({ key: "F5", ctrlKey: true, metaKey: false })).toBe(true);
    expect(isReloadKey({ key: "r", ctrlKey: true, metaKey: false })).toBe(true);
    expect(isReloadKey({ key: "R", ctrlKey: true, metaKey: false })).toBe(true);
    expect(isReloadKey({ key: "r", ctrlKey: false, metaKey: true })).toBe(true);
    expect(isReloadKey({ key: "r", ctrlKey: false, metaKey: false })).toBe(false);
    expect(isReloadKey({ key: "F4", ctrlKey: false, metaKey: false })).toBe(false);
  });
});

describe("blockReload", () => {
  it("stops F5, Ctrl+R and the context menu", () => {
    blockReload(document, true);
    expect(press({ key: "F5" }).defaultPrevented).toBe(true);
    expect(press({ key: "r", ctrlKey: true }).defaultPrevented).toBe(true);
    expect(press({ key: "R", ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(true);
    expect(press({ key: "F5", ctrlKey: true }).defaultPrevented).toBe(true);
    const menu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.body.dispatchEvent(menu);
    expect(menu.defaultPrevented).toBe(true);
  });

  it("leaves other keys alone", () => {
    blockReload(document, true);
    expect(press({ key: "f", ctrlKey: true }).defaultPrevented).toBe(false);
    expect(press({ key: "F3" }).defaultPrevented).toBe(false);
  });

  it("does nothing when it is off, as in a development build", () => {
    const doc = document.implementation.createHTMLDocument("x");
    blockReload(doc, false);
    const e = new KeyboardEvent("keydown", { key: "F5", bubbles: true, cancelable: true });
    doc.body.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
  });
});

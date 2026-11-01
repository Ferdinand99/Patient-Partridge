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
  const key = (k: string, extra: { code?: string; ctrlKey?: boolean; metaKey?: boolean } = {}) => ({
    key: k,
    code: extra.code ?? "",
    ctrlKey: extra.ctrlKey ?? false,
    metaKey: extra.metaKey ?? false,
  });

  it("knows the keys that reload a web view", () => {
    expect(isReloadKey(key("F5"))).toBe(true);
    expect(isReloadKey(key("F5", { ctrlKey: true }))).toBe(true);
    expect(isReloadKey(key("BrowserRefresh"))).toBe(true);
    expect(isReloadKey(key("r", { ctrlKey: true }))).toBe(true);
    expect(isReloadKey(key("R", { ctrlKey: true }))).toBe(true);
    expect(isReloadKey(key("r", { metaKey: true }))).toBe(true);
    expect(isReloadKey(key("r"))).toBe(false);
    expect(isReloadKey(key("F4"))).toBe(false);
  });

  it("knows Ctrl+R on a keyboard layout without Latin letters, where the key is another letter", () => {
    expect(isReloadKey(key("\u043A", { code: "KeyR", ctrlKey: true }))).toBe(true); // Russian
    expect(isReloadKey(key("\u03C1", { code: "KeyR", ctrlKey: true }))).toBe(true); // Greek
    expect(isReloadKey(key("\u05E8", { code: "KeyR", ctrlKey: true }))).toBe(true); // Hebrew
    // The same key without Ctrl is just a letter.
    expect(isReloadKey(key("\u043A", { code: "KeyR" }))).toBe(false);
  });

  it("leaves the shortcuts of a Latin layout that has another letter on that key", () => {
    // On Dvorak the key where R is on a US keyboard types P: Ctrl+P is print, not reload.
    expect(isReloadKey(key("p", { code: "KeyR", ctrlKey: true }))).toBe(false);
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

  it("keeps the menu of the web view for text boxes, which has Cut, Copy and Paste", () => {
    blockReload(document, true);
    const input = document.createElement("input");
    document.body.append(input);
    const menu = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    input.dispatchEvent(menu);
    expect(menu.defaultPrevented).toBe(false);
    const page = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
    document.body.dispatchEvent(page);
    expect(page.defaultPrevented).toBe(true);
  });

  it("stops Ctrl+R on a layout without Latin letters", () => {
    blockReload(document, true);
    expect(press({ key: "\u043A", code: "KeyR", ctrlKey: true }).defaultPrevented).toBe(true);
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

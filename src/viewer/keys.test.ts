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

  it("saves with Ctrl or Cmd+S, saves a copy with Shift, and undoes and redoes", () => {
    for (const mod of [{ ctrlKey: true }, { metaKey: true }]) {
      expect(actionForKey(key("s", mod))).toEqual({ type: "save" });
      expect(actionForKey(key("S", { ...mod, shiftKey: true }))).toEqual({ type: "save-as" });
      expect(actionForKey(key("z", mod))).toEqual({ type: "undo" });
      expect(actionForKey(key("Z", { ...mod, shiftKey: true }))).toEqual({ type: "redo" });
      expect(actionForKey(key("y", mod))).toEqual({ type: "redo" });
    }
    expect(actionForKey(key("s"))).toBeNull();
    expect(actionForKey(key("z"))).toBeNull();
  });

  it("picks a tool of the annotation bar with a plain letter, and toggles the bar with Ctrl+Shift+A", () => {
    const letters = { v: "select", h: "highlight", u: "underline", t: "strikeout", n: "note", p: "pen", m: "stamp" };
    for (const [letter, tool] of Object.entries(letters)) {
      expect(actionForKey(key(letter))).toEqual({ type: "tool", tool });
      expect(actionForKey(key(letter.toUpperCase()))).toEqual({ type: "tool", tool });
      // Shift, Ctrl and Alt make it something else or nothing.
      expect(actionForKey(key(letter.toUpperCase(), { shiftKey: true }))).toBeNull();
      expect(actionForKey(key(letter, { altKey: true }))).toBeNull();
    }
    expect(actionForKey(key("a", { ctrlKey: true }))).toEqual({ type: "select-all" });
    expect(actionForKey(key("A", { ctrlKey: true, shiftKey: true }))).toEqual({ type: "toggle-annotations" });
    expect(actionForKey(key("A", { metaKey: true, shiftKey: true }))).toEqual({ type: "toggle-annotations" });
  });

  it("leaves other keys and Alt combinations alone", () => {
    expect(actionForKey(key("a"))).toBeNull();
    expect(actionForKey(key("x"))).toBeNull();
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
    // A plain P picks the pen, no longer nothing.
    expect(actionForKey(key("p"))).toEqual({ type: "tool", tool: "pen" });
  });

  it("shows and hides the side panel with Ctrl+B", () => {
    expect(actionForKey(key("b", { ctrlKey: true }))).toEqual({ type: "toggle-sidebar" });
    expect(actionForKey(key("B", { metaKey: true }))).toEqual({ type: "toggle-sidebar" });
    expect(actionForKey(key("b"))).toBeNull();
  });

  describe("on a layout without Latin letters", () => {
    // The web view reports the letter of the layout; the code names the key by its US place.
    const layouts: [string, Record<string, string>][] = [
      ["Russian", { KeyS: "ы", KeyZ: "я", KeyW: "ц", KeyO: "щ", KeyY: "н" }],
      ["Greek", { KeyS: "σ", KeyZ: "ζ", KeyW: "ς", KeyO: "ο", KeyY: "υ" }],
      ["Hebrew", { KeyS: "ד", KeyZ: "ז", KeyW: "׳", KeyO: "ם", KeyY: "ט" }],
      ["Arabic", { KeyS: "س", KeyZ: "ئ", KeyW: "ص", KeyO: "خ", KeyY: "غ" }],
    ];
    for (const [name, letters] of layouts) {
      it(`${name}: Ctrl+S, Ctrl+Shift+S, Ctrl+Z, Ctrl+Y, Ctrl+W and Ctrl+O work`, () => {
        const press = (code: string, extra: Partial<KeyInfo> = {}) =>
          actionForKey({ ...key(letters[code]), code, ctrlKey: true, ...extra });
        expect(press("KeyS")).toEqual({ type: "save" });
        expect(press("KeyS", { shiftKey: true })).toEqual({ type: "save-as" });
        expect(press("KeyZ")).toEqual({ type: "undo" });
        expect(press("KeyZ", { shiftKey: true })).toEqual({ type: "redo" });
        expect(press("KeyY")).toEqual({ type: "redo" });
        expect(press("KeyW")).toEqual({ type: "close-tab" });
        expect(press("KeyO")).toEqual({ type: "open-file" });
        // Cmd on a Mac is the same key.
        expect(press("KeyS", { ctrlKey: false, metaKey: true })).toEqual({ type: "save" });
      });
    }

    it("does nothing without Ctrl, and does not take a letter that has no key place", () => {
      expect(actionForKey({ ...key("ы"), code: "KeyS" })).toBeNull();
      expect(actionForKey({ ...key("ы"), ctrlKey: true })).toBeNull();
      expect(actionForKey({ ...key("ы"), code: "Digit4", ctrlKey: true })).toBeNull();
    });

    it("leaves the named keys alone", () => {
      expect(actionForKey({ ...key("Tab"), code: "Tab", ctrlKey: true })).toEqual({ type: "next-tab" });
      expect(actionForKey({ ...key("Home"), code: "Home", ctrlKey: true })).toEqual({ type: "go-first" });
      expect(actionForKey({ ...key("Dead"), code: "KeyS", ctrlKey: true })).toBeNull();
    });
  });

  describe("on a Latin layout with letters in other places", () => {
    it("Dvorak: the letter printed on the key wins over the place", () => {
      // The key in the S place prints O on Dvorak, and the key in the O place prints R.
      expect(actionForKey({ ...key("o"), code: "KeyS", ctrlKey: true })).toEqual({ type: "open-file" });
      expect(actionForKey({ ...key("s"), code: "Semicolon", ctrlKey: true })).toEqual({ type: "save" });
      expect(actionForKey({ ...key("z"), code: "Slash", ctrlKey: true })).toEqual({ type: "undo" });
      expect(actionForKey({ ...key("w"), code: "Comma", ctrlKey: true })).toEqual({ type: "close-tab" });
      // The key in the S place does not save, because it prints O.
      expect(actionForKey({ ...key("o"), code: "KeyS", ctrlKey: true })).not.toEqual({ type: "save" });
    });

    it("Dvorak: punctuation on a key does not stand for the letter of the US place", () => {
      // The comma of Dvorak is in the W place, the semicolon in the Z place.
      for (const extra of [{}, { shiftKey: true }, { metaKey: true, ctrlKey: false }]) {
        expect(actionForKey({ ...key(",", extra), code: "KeyW", ctrlKey: true, ...extra })).toBeNull();
        expect(actionForKey({ ...key("<", extra), code: "KeyW", ctrlKey: true, ...extra })).toBeNull();
        expect(actionForKey({ ...key(";", extra), code: "KeyZ", ctrlKey: true, ...extra })).toBeNull();
      }
      expect(actionForKey({ ...key(","), code: "KeyW", ctrlKey: true })).toBeNull();
    });

    it("Colemak: the semicolon in the P place does not print", () => {
      expect(actionForKey({ ...key(";"), code: "KeyP", ctrlKey: true })).toBeNull();
    });

    it("BEPO: an accented letter in the W or Z place does not close a tab or undo", () => {
      expect(actionForKey({ ...key("é"), code: "KeyW", ctrlKey: true })).toBeNull();
      expect(actionForKey({ ...key("à"), code: "KeyZ", ctrlKey: true })).toBeNull();
      expect(actionForKey({ ...key("É"), code: "KeyW", ctrlKey: true, shiftKey: true })).toBeNull();
    });

    it("a digit or a symbol on a letter place is not taken for the letter", () => {
      expect(actionForKey({ ...key("1"), code: "KeyW", ctrlKey: true })).toEqual({ type: "zoom-actual" });
      expect(actionForKey({ ...key("-"), code: "KeyW", ctrlKey: true })).toEqual({ type: "zoom-out" });
    });

    it("AZERTY: A and Q follow what is printed", () => {
      expect(actionForKey({ ...key("a"), code: "KeyQ", ctrlKey: true })).toEqual({ type: "select-all" });
      expect(actionForKey({ ...key("z"), code: "KeyW", ctrlKey: true })).toEqual({ type: "undo" });
    });
  });
});

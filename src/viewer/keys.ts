// Keyboard shortcuts of the viewer, as pure data: a key press goes in, an action comes out.

import { type Tool, toolForKey } from "../lib/annotations";

export interface KeyInfo {
  key: string;
  /** The physical key. Optional, so that callers that only have the character still work. */
  code?: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

export type Action =
  | { type: "scroll-by"; x: number; y: number }
  | { type: "scroll-page"; direction: 1 | -1 }
  | { type: "go-first" }
  | { type: "go-last" }
  | { type: "zoom-in" }
  | { type: "zoom-out" }
  | { type: "zoom-fit-width" }
  | { type: "zoom-actual" }
  | { type: "focus-page-box" }
  | { type: "open-file" }
  | { type: "close-tab" }
  | { type: "next-tab" }
  | { type: "previous-tab" }
  | { type: "toggle-sidebar" }
  | { type: "find" }
  | { type: "find-next" }
  | { type: "find-previous" }
  | { type: "select-all" }
  | { type: "print" }
  | { type: "save" }
  | { type: "save-as" }
  | { type: "undo" }
  | { type: "redo" }
  /** A plain letter that picks a tool of the annotation bar. */
  | { type: "tool"; tool: Tool }
  | { type: "toggle-annotations" };

/** CSS pixels one arrow key press scrolls. */
export const LINE_SCROLL = 48;

/** A character of a script other than Latin. Punctuation, digits and marks belong to no script. */
const OTHER_SCRIPT = /^[^\p{Script=Latin}\p{Script=Common}\p{Script=Inherited}]$/u;

/**
 * The key a shortcut is matched on. What the key prints is taken as it is when the layout has
 * Latin letters: a letter of the Latin alphabet, but also punctuation and accented letters (the
 * comma of Dvorak, the e with an acute accent of the French BEPO layout), which are not the key
 * that letter of the US layout stands for. Only a key that prints a letter of another script
 * (Russian, Greek, Hebrew, Arabic) falls back to its place: the web view reports the letter of that
 * layout, so the key that prints S on a US keyboard counts as S.
 */
export function shortcutKey(e: KeyInfo): string {
  if (/^[a-z]$/i.test(e.key)) return e.key.toLowerCase();
  const place = /^Key([A-Z])$/.exec(e.code ?? "");
  if (place && [...e.key].length === 1 && OTHER_SCRIPT.test(e.key)) return place[1].toLowerCase();
  return e.key;
}

/**
 * What a key press means for the viewer, or null when the viewer does not use it.
 * Ctrl and Cmd are treated the same, so shortcuts work on Windows, Linux and macOS.
 */
export function actionForKey(e: KeyInfo): Action | null {
  const mod = e.ctrlKey || e.metaKey;
  if (e.altKey) return null;

  if (mod) {
    switch (shortcutKey(e)) {
      case "+":
      case "=":
        return { type: "zoom-in" };
      case "-":
      case "_":
        return { type: "zoom-out" };
      case "0":
        return { type: "zoom-fit-width" };
      case "1":
        return { type: "zoom-actual" };
      case "g":
        return { type: "focus-page-box" };
      case "o":
        return { type: "open-file" };
      case "w":
        return { type: "close-tab" };
      case "b":
        return { type: "toggle-sidebar" };
      case "f":
        return { type: "find" };
      case "a":
        return { type: e.shiftKey ? "toggle-annotations" : "select-all" };
      case "p":
        return { type: "print" };
      case "s":
        return { type: e.shiftKey ? "save-as" : "save" };
      case "z":
        return { type: e.shiftKey ? "redo" : "undo" };
      case "y":
        return { type: "redo" };
      case "Tab":
        return { type: e.shiftKey ? "previous-tab" : "next-tab" };
      case "PageDown":
        return { type: "next-tab" };
      case "PageUp":
        return { type: "previous-tab" };
      case "Home":
        return { type: "go-first" };
      case "End":
        return { type: "go-last" };
      default:
        return null;
    }
  }

  switch (e.key) {
    case "F3":
      return { type: e.shiftKey ? "find-previous" : "find-next" };
    case "ArrowDown":
      return { type: "scroll-by", x: 0, y: LINE_SCROLL };
    case "ArrowUp":
      return { type: "scroll-by", x: 0, y: -LINE_SCROLL };
    case "ArrowRight":
      return { type: "scroll-by", x: LINE_SCROLL, y: 0 };
    case "ArrowLeft":
      return { type: "scroll-by", x: -LINE_SCROLL, y: 0 };
    case "PageDown":
      return { type: "scroll-page", direction: 1 };
    case "PageUp":
      return { type: "scroll-page", direction: -1 };
    case " ":
      return { type: "scroll-page", direction: e.shiftKey ? -1 : 1 };
    case "Home":
      return { type: "go-first" };
    case "End":
      return { type: "go-last" };
    default: {
      // A single letter picks a tool of the annotation bar (V, H, U, T, N, P, M).
      const tool = e.shiftKey ? null : toolForKey(e.key);
      return tool ? { type: "tool", tool } : null;
    }
  }
}

/** How far a Page Down moves: a little less than the viewport, so the reader keeps their place. */
export function pageScrollDistance(viewportHeight: number): number {
  return Math.max(LINE_SCROLL, Math.round(viewportHeight * 0.9));
}

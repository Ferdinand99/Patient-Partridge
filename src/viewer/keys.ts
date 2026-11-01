// Keyboard shortcuts of the viewer, as pure data: a key press goes in, an action comes out.

export interface KeyInfo {
  key: string;
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
  | { type: "previous-tab" };

/** CSS pixels one arrow key press scrolls. */
export const LINE_SCROLL = 48;

/**
 * What a key press means for the viewer, or null when the viewer does not use it.
 * Ctrl and Cmd are treated the same, so shortcuts work on Windows, Linux and macOS.
 */
export function actionForKey(e: KeyInfo): Action | null {
  const mod = e.ctrlKey || e.metaKey;
  if (e.altKey) return null;

  if (mod) {
    switch (e.key) {
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
      case "G":
        return { type: "focus-page-box" };
      case "o":
      case "O":
        return { type: "open-file" };
      case "w":
      case "W":
        return { type: "close-tab" };
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
    default:
      return null;
  }
}

/** How far a Page Down moves: a little less than the viewport, so the reader keeps their place. */
export function pageScrollDistance(viewportHeight: number): number {
  return Math.max(LINE_SCROLL, Math.round(viewportHeight * 0.9));
}

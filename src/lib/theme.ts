// The colour theme: follow the system, or force light or dark. The choice is kept in the web
// view's own local storage on this computer and goes nowhere else.

export type ThemePref = "auto" | "light" | "dark";

export const THEME_KEY = "partridge.theme";

export const THEME_CHOICES: readonly ThemePref[] = ["auto", "light", "dark"];

export function parseTheme(value: string | null | undefined): ThemePref {
  return value === "light" || value === "dark" ? value : "auto";
}

export interface StorageLike {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** The stored choice. A missing, unreadable or unknown value means "follow the system". */
export function loadTheme(storage: StorageLike | null): ThemePref {
  try {
    return parseTheme(storage?.getItem(THEME_KEY));
  } catch {
    return "auto";
  }
}

export function saveTheme(storage: StorageLike | null, pref: ThemePref): void {
  try {
    storage?.setItem(THEME_KEY, pref);
  } catch {
    // Storage can be off. The choice then lasts until the app closes.
  }
}

/**
 * Puts the choice on the page. "auto" leaves the page alone so the system decides (the style sheet
 * follows `prefers-color-scheme`); the other two force a theme.
 */
export function applyTheme(root: HTMLElement, pref: ThemePref): void {
  if (pref === "auto") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", pref);
}

export function themeLabel(pref: ThemePref): string {
  return pref === "auto" ? "Auto" : pref === "light" ? "Light" : "Dark";
}

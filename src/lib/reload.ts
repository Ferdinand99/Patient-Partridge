// A web view can reload the page: F5, Ctrl+R and the "Reload" entry of its context menu all do it.
// The window keeps everything it knows about the open documents in memory, so a reload would drop
// all the tabs while the engines of the documents went on running. In a finished app nobody needs a
// reload, so it is turned off here. (The app side also lets go of the documents whenever a page
// starts, so whatever reload still gets through starts clean; see `on_page_load` in src-tauri.)

/** The keys that reload a web view: F5 (with or without Ctrl) and Ctrl or Cmd with R. */
export function isReloadKey(e: Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey">): boolean {
  if (e.key === "F5" || e.key === "BrowserRefresh") return true;
  if (!(e.ctrlKey || e.metaKey)) return false;
  if (e.key.toLowerCase() === "r") return true;
  // On a keyboard layout without Latin letters (Russian, Greek, Hebrew, Arabic) the web view still
  // reloads on the key where the R is. A Latin layout that has another letter there (Dvorak) keeps
  // that letter for the app's own shortcuts.
  return e.code === "KeyR" && !/^[a-z]$/i.test(e.key);
}

/**
 * True for a box the reader types in, which keeps the menu of the web view (Cut, Copy, Paste). A
 * radio button or a check box is not one: its menu is the page menu, which holds Reload.
 */
export function isTextBox(target: Element): boolean {
  const box = target.closest("input, textarea");
  if (!box) return false;
  if (box instanceof HTMLTextAreaElement) return true;
  return box instanceof HTMLInputElement && TEXT_TYPES.has(box.type);
}

const TEXT_TYPES: ReadonlySet<string> = new Set(["text", "search", "password", "email", "tel", "url", "number"]);

/**
 * Stops the reload keys and the context menu. Does nothing when `enabled` is false, which is the
 * case in a development build, where reloading the page is how a change is seen.
 */
export function blockReload(doc: Document, enabled: boolean): void {
  if (!enabled) return;
  // Text boxes keep the menu of the web view (Cut, Copy, Paste). The app has its own menu for
  // selected text on the pages.
  doc.addEventListener(
    "contextmenu",
    (e) => {
      const target = e.target;
      if (target instanceof Element && isTextBox(target)) return;
      e.preventDefault();
    },
    true,
  );
  doc.addEventListener(
    "keydown",
    (e) => {
      if (isReloadKey(e)) e.preventDefault();
    },
    true,
  );
}

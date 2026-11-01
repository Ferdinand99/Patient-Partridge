// A web view can reload the page: F5, Ctrl+R and the "Reload" entry of its context menu all do it.
// The window keeps everything it knows about the open documents in memory, so a reload would drop
// all the tabs while the engines of the documents went on running. In a finished app nobody needs a
// reload, so it is turned off here. (The app side also lets go of the documents whenever a page
// starts, so whatever reload still gets through starts clean; see `on_page_load` in src-tauri.)

/** The keys that reload a web view: F5 (with or without Ctrl) and Ctrl or Cmd with R. */
export function isReloadKey(e: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey">): boolean {
  if (e.key === "F5") return true;
  return (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "r";
}

/**
 * Stops the reload keys and the context menu. Does nothing when `enabled` is false, which is the
 * case in a development build, where reloading the page is how a change is seen.
 */
export function blockReload(doc: Document, enabled: boolean): void {
  if (!enabled) return;
  doc.addEventListener("contextmenu", (e) => e.preventDefault(), true);
  doc.addEventListener(
    "keydown",
    (e) => {
      if (isReloadKey(e)) e.preventDefault();
    },
    true,
  );
}

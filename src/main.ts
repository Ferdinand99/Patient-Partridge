import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import "./styles/tokens.css";
import "./styles/components.css";
import "./styles/app.css";
import { App } from "./app/app";
import { Updater } from "./app/updater";
import { api } from "./lib/api";
import { blockReload } from "./lib/reload";

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

// An error in the window is also written to the app's error stream, where a terminal or the log of
// a test run shows it. It carries only the text of the error.
window.addEventListener("error", (e) => {
  void api.frontendError(`${e.message} (${e.filename}:${e.lineno})`).catch(() => undefined);
});
window.addEventListener("unhandledrejection", (e) => {
  void api.frontendError(`unhandled rejection: ${String(e.reason)}`).catch(() => undefined);
});

// A finished app never reloads its page (see lib/reload.ts); a development build still can.
blockReload(document, import.meta.env.PROD);

const app = new App(document, {
  api,
  async pickFiles() {
    const picked = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "PDF documents", extensions: ["pdf"] }],
    });
    if (Array.isArray(picked)) return picked;
    return typeof picked === "string" ? [picked] : [];
  },
  setWindowTitle(title) {
    void getCurrentWindow().setTitle(title);
  },
  now: () => performance.now(),
  storage: storage(),
  print: {
    // The web view of macOS ignores window.print(); the Rust side opens its print dialog instead.
    print: () => {
      if (/Mac/i.test(navigator.userAgent)) return api.printWebview();
      window.print();
      return Promise.resolve();
    },
  },
});
app.init();

// Updates: the app process checks, downloads and installs; the window only asks and shows.
new Updater(document, {
  api,
  openPaths: () => app.openDocumentPaths(),
  closeMenu: () => app.closeMenu(),
}).init();

function openAll(paths: string[]): void {
  void app.openChosen(paths);
}

// Files dropped on the window. Tauri hands us real paths; the page never reads the file itself.
void getCurrentWebview().onDragDropEvent((event) => {
  const payload = event.payload;
  if (payload.type === "enter" || payload.type === "over") app.setDropActive(true);
  else if (payload.type === "leave") app.setDropActive(false);
  else if (payload.type === "drop") {
    app.setDropActive(false);
    openAll(payload.paths);
  }
});

// Files named on the command line, or sent by a second copy of the app (also "Open with" from a
// file manager, once for every file chosen).
void listen<string[]>("open-paths", (event) => openAll(event.payload));
void api.takePendingPaths().then(openAll);
// The files that were open before an update come back, all of them.
void api
  .takeRestoredPaths()
  .then((paths) => app.restorePaths(paths))
  .catch(() => undefined);

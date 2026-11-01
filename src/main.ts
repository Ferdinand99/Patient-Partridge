import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import "./styles/tokens.css";
import "./styles/components.css";
import "./styles/app.css";
import { App } from "./app/app";
import { api } from "./lib/api";
import { pickPdfPaths } from "./lib/paths";

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

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
});
app.init();

function openAll(paths: string[]): void {
  const chosen = pickPdfPaths(paths);
  if (chosen.length > 0) void app.openPaths(chosen);
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

import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open } from "@tauri-apps/plugin-dialog";
import "./styles/app.css";
import { App } from "./app/app";
import { api } from "./lib/api";
import { pickPdfPath } from "./lib/paths";

const app = new App(document, {
  api,
  async pickFile() {
    const picked = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "PDF documents", extensions: ["pdf"] }],
    });
    return typeof picked === "string" ? picked : null;
  },
  setWindowTitle(title) {
    void getCurrentWindow().setTitle(title);
  },
  now: () => performance.now(),
});
app.init();

function openFirst(paths: string[]): void {
  const path = pickPdfPath(paths);
  if (path) void app.openPath(path);
}

// Files dropped on the window. Tauri hands us real paths; the page never reads the file itself.
void getCurrentWebview().onDragDropEvent((event) => {
  const payload = event.payload;
  if (payload.type === "enter" || payload.type === "over") app.setDropActive(true);
  else if (payload.type === "leave") app.setDropActive(false);
  else if (payload.type === "drop") {
    app.setDropActive(false);
    openFirst(payload.paths);
  }
});

// Files named on the command line, or sent by a second copy of the app.
void listen<string[]>("open-paths", (event) => openFirst(event.payload));
void api.takePendingPaths().then(openFirst);

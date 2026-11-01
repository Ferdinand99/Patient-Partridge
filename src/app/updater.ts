// Updates of the app: the small pill in the toolbar, the question with the release notes, the
// download with its progress, and the restart. The checking, downloading and installing is all done
// by the app process (see updates.rs); this window loads nothing from the network.
//
// A check runs a few seconds after the start and then every 6 hours, unless the user turned that off
// in the settings menu. "Check for Updates…" in the same menu always checks, and tells the user the
// result. A check that finds nothing, or that cannot reach the server, says nothing by itself.

import type { InstallMethod, UpdateFailure, UpdateInfo, UpdateSettings } from "../lib/api";
import { showNotice } from "./notice";

/** The first automatic check, after the start, in milliseconds. */
export const FIRST_CHECK_DELAY_MS = 4000;
/** Time between automatic checks, in milliseconds. */
export const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

export interface UpdaterDeps {
  api: {
    getUpdateSettings(): Promise<UpdateSettings>;
    setAutoUpdate(enabled: boolean): Promise<void>;
    checkUpdate(manual: boolean): Promise<UpdateInfo | null>;
    downloadUpdate(onEvent: (event: { event: "progress"; chunk: number; total: number | null }) => void): Promise<void>;
    installUpdate(active: string | null): Promise<void>;
    restartApp(): Promise<void>;
    openReleasesPage(): Promise<void>;
  };
  /**
   * The paths of the open documents, the one in view first. The app saves where the reader is in
   * each of them before it answers. It is asked when the download is finished, just before the
   * install, and only the first path is used: the app takes the list of open files itself.
   */
  openPaths(): string[];
  /** Closes the settings menu. */
  closeMenu(): void;
}

function byId<T extends HTMLElement>(root: Document, id: string): T {
  const el = root.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
}

/**
 * Release notes as plain text for the question. They are markdown from the changelog and came over
 * the network: they are never shown as HTML, only turned into readable lines.
 */
export function notesToText(markdown: string): string {
  const text = markdown
    .replace(/\r\n?/g, "\n")
    .replace(/<[^>\n]*>/g, "")
    .replace(/\[([^\]\n]*)\]\([^)\n]*\)/g, "$1")
    .replace(/[ \t]*\((?:[0-9a-f]{7,40})\)/g, "")
    .replace(/^ {0,3}#{1,6}[ \t]*/gm, "")
    .replace(/^[ \t]*[*+-][ \t]+/gm, "• ")
    .replace(/(\*\*|__)(.+?)\1/g, "$2")
    .replace(/[ \t]+$/gm, "")
    .replace(/\n{3,}/g, "\n\n");
  return text.trim();
}

/** The sentence for a failure, in words the user can act on. */
export function failureText(f: UpdateFailure): string {
  switch (f.kind) {
    case "offline":
      return "Patient Partridge could not reach the update server. Check your internet connection and try again.";
    case "invalidSignature":
      return "The update does not carry the right signature, so it was not installed.";
    case "notConfigured":
      return "Updates are not set up in this build.";
    case "badManifest":
      return "The update information could not be read, or has nothing for this computer.";
    case "busy":
      return "An update is being installed already.";
    case "manual":
      return "This copy of Patient Partridge cannot install an update by itself. Download the new version from the releases page.";
    default:
      return f.message ? `The update did not work: ${f.message}` : "The update did not work.";
  }
}

const asFailure = (e: unknown): UpdateFailure => {
  if (typeof e === "object" && e !== null && "kind" in e) {
    const f = e as Partial<UpdateFailure>;
    return { kind: f.kind ?? "other", message: String(f.message ?? ""), enginesStopped: f.enginesStopped === true };
  }
  return { kind: "other", message: typeof e === "string" ? e : e instanceof Error ? e.message : "", enginesStopped: false };
};

interface SheetOptions {
  title: string;
  text?: string;
  notes?: string;
  progress?: boolean;
  buttons?: { id: string; label: string; primary?: boolean }[];
  /** The id answered when Escape is pressed. Without it Escape does nothing. */
  escape?: string;
}

interface Sheet {
  answer: Promise<string>;
  setText(text: string): void;
  /** `total` null: an unknown length. */
  setProgress(done: number, total: number | null): void;
  close(): void;
}

let sheetCounter = 0;

function openSheet(layer: HTMLElement, options: SheetOptions): Sheet {
  const id = `update-sheet-${++sheetCounter}`;
  const previousFocus = document.activeElement as HTMLElement | null;
  const box = document.createElement("div");
  box.className = "sheet notice";
  box.setAttribute("role", "alertdialog");
  box.setAttribute("aria-modal", "true");
  box.setAttribute("aria-labelledby", `${id}-title`);
  box.setAttribute("aria-describedby", `${id}-text`);

  const title = document.createElement("h2");
  title.className = "sheet-title";
  title.id = `${id}-title`;
  title.textContent = options.title;
  const text = document.createElement("p");
  text.className = "sheet-text";
  text.id = `${id}-text`;
  text.textContent = options.text ?? "";
  box.append(title, text);

  if (options.notes) {
    const notes = document.createElement("div");
    notes.className = "update-notes";
    notes.tabIndex = 0;
    notes.setAttribute("role", "region");
    notes.setAttribute("aria-label", "What is new");
    // Text only: the notes came from the network.
    notes.textContent = options.notes;
    box.append(notes);
  }

  let bar: HTMLProgressElement | null = null;
  if (options.progress) {
    bar = document.createElement("progress");
    bar.className = "print-progress";
    bar.max = 1;
    bar.setAttribute("aria-labelledby", `${id}-text`);
    box.append(bar);
  }

  let resolve!: (id: string) => void;
  const answer = new Promise<string>((r) => (resolve = r));
  const buttons = new Map<string, HTMLButtonElement>();
  if (options.buttons?.length) {
    const foot = document.createElement("div");
    foot.className = "sheet-foot";
    for (const b of options.buttons) {
      const el = document.createElement("button");
      el.type = "button";
      el.className = b.primary ? "btn btn-primary" : "btn btn-secondary";
      el.textContent = b.label;
      el.addEventListener("click", () => {
        close();
        resolve(b.id);
      });
      buttons.set(b.id, el);
      foot.append(el);
    }
    box.append(foot);
  }

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && layer.lastElementChild === box) {
      e.preventDefault();
      e.stopPropagation();
      if (options.escape !== undefined) {
        close();
        resolve(options.escape);
      }
    }
  };
  function close(): void {
    layer.removeEventListener("keydown", onKey);
    box.remove();
    layer.hidden = layer.childElementCount === 0;
    previousFocus?.focus?.();
  }
  layer.addEventListener("keydown", onKey);
  layer.append(box);
  layer.hidden = false;
  const first = options.buttons?.find((b) => b.primary) ?? options.buttons?.[0];
  box.tabIndex = -1;
  const target = first ? buttons.get(first.id) : box.querySelector<HTMLElement>(".update-notes");
  (target ?? box).focus();

  return {
    answer,
    setText(t) {
      text.textContent = t;
    },
    setProgress(done, total) {
      if (!bar) return;
      if (total && total > 0) {
        bar.max = total;
        bar.value = Math.min(done, total);
      } else {
        bar.removeAttribute("value");
      }
    },
    close,
  };
}

type State =
  | { kind: "idle" }
  | { kind: "available"; info: UpdateInfo }
  | { kind: "working" }
  | { kind: "ready"; info: UpdateInfo };

export class Updater {
  private state: State = { kind: "idle" };
  private checking = false;
  private autoUpdate = true;
  private method: InstallMethod = "automatic";
  private timers: ReturnType<typeof setTimeout>[] = [];

  private readonly pill: HTMLButtonElement;
  private readonly pillLabel: HTMLElement;
  private readonly toggle: HTMLButtonElement;
  private readonly layer: HTMLElement;

  constructor(
    private readonly root: Document,
    private readonly deps: UpdaterDeps,
  ) {
    this.pill = byId(root, "update-pill");
    this.pillLabel = byId(root, "update-pill-label");
    this.toggle = byId(root, "auto-update");
    this.layer = byId(root, "notice-layer");
  }

  init(): void {
    this.pill.addEventListener("click", () => void this.pillClicked());
    byId(this.root, "check-updates").addEventListener("click", () => {
      this.deps.closeMenu();
      void this.check(true);
    });
    this.toggle.addEventListener("click", () => void this.toggleAuto());
    void this.start();
  }

  /** Stops the timers (for tests). */
  stop(): void {
    for (const t of this.timers) {
      clearTimeout(t);
      clearInterval(t);
    }
    this.timers = [];
  }

  private async start(): Promise<void> {
    try {
      const settings = await this.deps.api.getUpdateSettings();
      this.autoUpdate = settings.autoUpdate;
      this.method = settings.method;
    } catch {
      // The settings could not be read: stay with "on", which is the default.
    }
    this.showToggle();
    const tick = () => {
      if (this.autoUpdate) void this.check(false);
    };
    this.timers.push(setTimeout(tick, FIRST_CHECK_DELAY_MS), setInterval(tick, CHECK_INTERVAL_MS));
  }

  private showToggle(): void {
    this.toggle.setAttribute("aria-checked", String(this.autoUpdate));
  }

  private async toggleAuto(): Promise<void> {
    const wanted = !this.autoUpdate;
    this.autoUpdate = wanted;
    this.showToggle();
    try {
      await this.deps.api.setAutoUpdate(wanted);
    } catch {
      // Not saved: after the next start the app would check again. Say so, and show the truth.
      this.autoUpdate = !wanted;
      this.showToggle();
      await this.tell(
        "The setting was not saved",
        "Patient Partridge could not write the setting to this computer, so it stays as it was.",
      );
    }
  }

  private showPill(): void {
    const s = this.state;
    if (s.kind === "idle") {
      this.pill.hidden = true;
      return;
    }
    this.pill.hidden = false;
    if (s.kind === "ready") {
      this.pillLabel.textContent = "Restart to update";
      this.pill.title = "The update is installed. Restart Patient Partridge to use it.";
    } else if (s.kind === "working") {
      this.pillLabel.textContent = "Updating…";
      this.pill.title = "An update is being installed";
    } else {
      this.pillLabel.textContent = `Update ${s.info.version}`;
      this.pill.title = `Version ${s.info.version} is available`;
    }
  }

  private async pillClicked(): Promise<void> {
    const s = this.state;
    if (s.kind === "available") await this.offer(s.info);
    else if (s.kind === "ready") await this.askRestart();
  }

  /** Checks the update server. A person's check (`manual`) always tells the result. */
  async check(manual: boolean): Promise<void> {
    if (this.checking || this.state.kind === "working" || this.state.kind === "ready") {
      if (manual && this.state.kind === "ready") await this.askRestart();
      return;
    }
    this.checking = true;
    try {
      const info = await this.deps.api.checkUpdate(manual);
      if (!info) {
        this.state = { kind: "idle" };
        this.showPill();
        if (manual) await this.upToDate();
        return;
      }
      this.state = { kind: "available", info };
      this.showPill();
      if (manual) await this.offer(info);
    } catch (e) {
      const f = asFailure(e);
      if (f.kind === "disabled") return;
      // A check by itself stays quiet about trouble, except a wrong signature, which is worth a word.
      if (manual || f.kind === "invalidSignature") await this.tell("Could not check for updates", failureText(f));
    } finally {
      this.checking = false;
    }
  }

  private async tell(title: string, detail: string): Promise<void> {
    await showNotice(this.layer, { title, detail, buttons: [{ id: "ok", label: "OK", primary: true }] });
  }

  private async upToDate(): Promise<void> {
    let version = "";
    try {
      version = ` (version ${(await this.deps.api.getUpdateSettings()).currentVersion})`;
    } catch {
      // Not needed for the message.
    }
    await this.tell("Patient Partridge is up to date", `You have the newest version${version}.`);
  }

  /** The question: what is new, and Download and Install. A copy that cannot update itself gets the link. */
  private async offer(info: UpdateInfo): Promise<void> {
    const notes = notesToText(info.notes);
    const manual = info.method !== "automatic" || this.method !== "automatic";
    const sheet = openSheet(this.layer, {
      title: `Version ${info.version} is available`,
      text: manual
        ? `You have version ${info.currentVersion}. This copy of Patient Partridge was installed from a package, so it cannot update itself. Download the new version from the releases page and install it.`
        : `You have version ${info.currentVersion}. The update is checked and installed by Patient Partridge itself. The files you have open come back after the restart.`,
      notes: notes || undefined,
      buttons: manual
        ? [
            { id: "later", label: "Close" },
            { id: "releases", label: "Open the releases page", primary: true },
          ]
        : [
            { id: "later", label: "Later" },
            { id: "install", label: "Download and Install", primary: true },
          ],
      escape: "later",
    });
    const answer = await sheet.answer;
    if (answer === "releases") await this.deps.api.openReleasesPage().catch(() => undefined);
    else if (answer === "install") await this.install(info);
  }

  private async install(info: UpdateInfo): Promise<void> {
    this.state = { kind: "working" };
    this.showPill();
    const sheet = openSheet(this.layer, {
      title: `Downloading version ${info.version}`,
      text: "Starting the download…",
      progress: true,
    });
    sheet.setProgress(0, null);
    let received = 0;
    try {
      await this.deps.api.downloadUpdate((e) => {
        received += e.chunk;
        sheet.setProgress(received, e.total);
        sheet.setText(
          e.total ? `${Math.min(100, Math.floor((received / e.total) * 100))} % of ${mebibytes(e.total)}` : `${mebibytes(received)} so far`,
        );
      });
    } catch (e) {
      sheet.close();
      await this.installFailed(info, asFailure(e));
      return;
    }
    // The download is done and checked. Only now is the reader's place saved and the file in view
    // noted: a file opened or closed during the download counts.
    sheet.setProgress(0, null);
    sheet.setText("Installing. The open documents are closed for a moment.");
    const active = this.deps.openPaths()[0] ?? null;
    try {
      await this.deps.api.installUpdate(active);
    } catch (e) {
      sheet.close();
      await this.installFailed(info, asFailure(e));
      return;
    }
    sheet.close();
    this.state = { kind: "ready", info };
    this.showPill();
    await this.askRestart();
  }

  private async installFailed(info: UpdateInfo, f: UpdateFailure): Promise<void> {
    if (f.enginesStopped) {
      // The documents were closed for the install and it failed. They cannot be used any more;
      // a restart brings them back.
      this.state = { kind: "ready", info };
      this.showPill();
      const answer = await openSheet(this.layer, {
        title: "The update could not be installed",
        text: `${failureText(f)} The open documents were closed to make room for it. Restart Patient Partridge to open them again.`,
        buttons: [{ id: "restart", label: "Restart", primary: true }],
      }).answer;
      if (answer === "restart") await this.deps.api.restartApp();
      return;
    }
    this.state = { kind: "available", info };
    this.showPill();
    await this.tell("The update was not installed", failureText(f));
  }

  /** After an install the engines are stopped, so the app has to restart; there is no "later". */
  private async askRestart(): Promise<void> {
    const answer = await openSheet(this.layer, {
      title: "Restart to update",
      text: "The update is installed. Restart Patient Partridge to use it. The files you have open come back.",
      buttons: [{ id: "restart", label: "Restart to Update", primary: true }],
    }).answer;
    if (answer === "restart") await this.deps.api.restartApp();
  }
}

function mebibytes(bytes: number): string {
  return `${(bytes / 1048576).toFixed(1)} MiB`;
}

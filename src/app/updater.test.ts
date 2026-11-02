// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { UpdateFailure, UpdateInfo, UpdateSettings } from "../lib/api";
import { CHECK_INTERVAL_MS, FIRST_CHECK_DELAY_MS, Updater, failureText, notesToText, type UpdaterDeps } from "./updater";

function installPage(): void {
  const html = readFileSync("index.html", "utf8");
  const body = /<body>([\s\S]*)<\/body>/.exec(html)?.[1] ?? "";
  document.body.innerHTML = body.replace(/<script[\s\S]*?<\/script>/g, "");
}

const info = (over: Partial<UpdateInfo> = {}): UpdateInfo => ({
  version: "0.2.0",
  currentVersion: "0.1.0",
  notes: "## 0.2.0\n\n* **viewer:** faster pages ([#12](https://example.com/12)) ([abcdef0](https://example.com/c))",
  method: "automatic",
  ...over,
});

type Fakes = {
  [K in keyof UpdaterDeps["api"]]: ReturnType<typeof vi.fn>;
};

function make(opts: { settings?: Partial<UpdateSettings>; found?: UpdateInfo | null; paths?: string[] } = {}) {
  const settings: UpdateSettings = { autoUpdate: true, method: "automatic", currentVersion: "0.1.0", ...opts.settings };
  const api: Fakes = {
    getUpdateSettings: vi.fn(async () => settings),
    setAutoUpdate: vi.fn(async () => undefined),
    checkUpdate: vi.fn(async () => (opts.found === undefined ? info() : opts.found)),
    downloadUpdate: vi.fn(async () => undefined),
    installUpdate: vi.fn(async () => undefined),
    restartApp: vi.fn(async () => undefined),
    openReleasesPage: vi.fn(async () => undefined),
  };
  const openPaths = vi.fn(() => opts.paths ?? ["/docs/a.pdf", "/docs/b.pdf"]);
  const closeMenu = vi.fn();
  const updater = new Updater(document, { api: api as unknown as UpdaterDeps["api"], openPaths, closeMenu });
  updater.init();
  return { api, updater, openPaths, closeMenu };
}

const flush = () => vi.advanceTimersByTimeAsync(0);
const pill = () => document.getElementById("update-pill") as HTMLButtonElement;
const layer = () => document.getElementById("notice-layer") as HTMLElement;
const sheetText = () => layer().textContent ?? "";
const button = (label: string) =>
  [...layer().querySelectorAll("button")].find((b) => b.textContent === label) as HTMLButtonElement | undefined;
const click = async (el: Element | undefined | null) => {
  if (!el) throw new Error("no such element");
  (el as HTMLElement).click();
  await flush();
};

beforeEach(() => {
  installPage();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
});

describe("notesToText", () => {
  it("turns the changelog into readable lines and drops links and commit hashes", () => {
    const md =
      "## [0.2.0](https://example.com/compare) (2026-11-02)\n\n### Features\n\n* **viewer:** faster pages ([#12](https://example.com/12)) ([abcdef0](https://example.com/c))\n- plain item";
    expect(notesToText(md)).toBe(
      "0.2.0 (2026-11-02)\n\nFeatures\n\n• viewer: faster pages (#12)\n• plain item",
    );
  });

  it("removes tags and keeps the text", () => {
    expect(notesToText("a <script>alert(1)</script> b <img src=x onerror=alert(1)>")).toBe("a alert(1) b");
    expect(notesToText("")).toBe("");
  });
});

describe("notesToText on a real changelog (Nimble Newt v0.3.0, written by release-please)", () => {
  const real = JSON.parse(readFileSync("scripts/fixtures/nimble-newt-v0.3.0/latest.json", "utf8")) as { notes: string };
  it("drops the links and the commit and issue marks, and keeps the headings, the warning and the text", () => {
    const text = notesToText(real.notes);
    expect(text).toContain("0.3.0 (2026-10-28)");
    expect(text).toContain("BREAKING CHANGES");
    expect(text).toContain("• extension code now runs in a Web Worker");
    expect(text).toContain("run extensions in a worker so they have no way out (NN-02) (#30)");
    expect(text).not.toContain("https://");
    expect(text).not.toContain("](");
    expect(text).not.toContain("cd2449d");
  });
});

describe("failureText", () => {
  it("has a sentence for every kind", () => {
    const kinds = ["offline", "invalidSignature", "notConfigured", "badManifest", "busy", "manual", "location", "other"] as const;
    for (const kind of kinds) {
      const f: UpdateFailure = { kind, message: "detail", enginesStopped: false };
      expect(failureText(f).length).toBeGreaterThan(20);
    }
    expect(failureText({ kind: "other", message: "disk full", enginesStopped: false })).toContain("disk full");
  });
});

describe("checking", () => {
  it("checks a few seconds after the start and then every six hours", async () => {
    const { api } = make();
    await flush();
    expect(api.checkUpdate).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    expect(api.checkUpdate).toHaveBeenCalledTimes(1);
    expect(api.checkUpdate).toHaveBeenLastCalledWith(false);
    await vi.advanceTimersByTimeAsync(CHECK_INTERVAL_MS);
    expect(api.checkUpdate).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(CHECK_INTERVAL_MS);
    expect(api.checkUpdate).toHaveBeenCalledTimes(3);
  });

  it("shows a small pill and nothing else when an automatic check finds an update", async () => {
    make();
    expect(pill().hidden).toBe(true);
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    expect(pill().hidden).toBe(false);
    expect(pill().textContent).toContain("Update 0.2.0");
    expect(layer().hidden).toBe(true);
  });

  it("hides the pill again when the next check finds nothing", async () => {
    const { api } = make();
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    expect(pill().hidden).toBe(false);
    api.checkUpdate.mockResolvedValueOnce(null);
    await vi.advanceTimersByTimeAsync(CHECK_INTERVAL_MS);
    expect(pill().hidden).toBe(true);
  });

  it("does not check by itself when the setting is off, but the menu entry still checks", async () => {
    const { api, closeMenu } = make({ settings: { autoUpdate: false }, found: null });
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS + CHECK_INTERVAL_MS * 2);
    expect(api.checkUpdate).not.toHaveBeenCalled();
    expect(document.getElementById("auto-update")?.getAttribute("aria-checked")).toBe("false");

    await click(document.getElementById("check-updates"));
    expect(closeMenu).toHaveBeenCalled();
    expect(api.checkUpdate).toHaveBeenCalledWith(true);
    expect(sheetText()).toContain("Patient Partridge is up to date");
    expect(sheetText()).toContain("version 0.1.0");
  });

  it("the switch in the menu saves the choice and takes effect at once", async () => {
    const { api } = make();
    await flush();
    const toggle = document.getElementById("auto-update") as HTMLButtonElement;
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    await click(toggle);
    expect(api.setAutoUpdate).toHaveBeenLastCalledWith(false);
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS + CHECK_INTERVAL_MS);
    expect(api.checkUpdate).not.toHaveBeenCalled();
    await click(toggle);
    expect(api.setAutoUpdate).toHaveBeenLastCalledWith(true);
    await vi.advanceTimersByTimeAsync(CHECK_INTERVAL_MS);
    expect(api.checkUpdate).toHaveBeenCalledTimes(1);
  });

  it("stays quiet about trouble in an automatic check, except a wrong signature", async () => {
    const { api } = make();
    api.checkUpdate.mockRejectedValueOnce({ kind: "offline", message: "x", enginesStopped: false });
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    expect(layer().hidden).toBe(true);
    api.checkUpdate.mockRejectedValueOnce({ kind: "disabled", message: "off", enginesStopped: false });
    await vi.advanceTimersByTimeAsync(CHECK_INTERVAL_MS);
    expect(layer().hidden).toBe(true);
    api.checkUpdate.mockRejectedValueOnce({ kind: "invalidSignature", message: "x", enginesStopped: false });
    await vi.advanceTimersByTimeAsync(CHECK_INTERVAL_MS);
    expect(sheetText()).toContain("signature");
  });

  it("tells once, calmly, that the app cannot update from where it is, and never shows an I/O error", async () => {
    const { api } = make();
    const location = { kind: "location", message: "the app was started through a symbolic link", enginesStopped: false };
    api.checkUpdate.mockRejectedValue(location);
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    expect(sheetText()).toContain(
      "Automatic updates aren't available from this location. Move Patient Partridge to the Applications folder.",
    );
    expect(sheetText()).not.toMatch(/I\/O|symbolic link|did not work/);
    await click(button("OK"));
    expect(layer().hidden).toBe(true);
    // The next automatic checks are quiet.
    await vi.advanceTimersByTimeAsync(CHECK_INTERVAL_MS * 2);
    expect(api.checkUpdate).toHaveBeenCalledTimes(3);
    expect(layer().hidden).toBe(true);
    // A person who asks is told again.
    await click(document.getElementById("check-updates"));
    expect(sheetText()).toContain("Move Patient Partridge to the Applications folder");
  });

  it("tells a person who asked what went wrong", async () => {
    const { api } = make({ settings: { autoUpdate: false } });
    await flush();
    api.checkUpdate.mockRejectedValueOnce({ kind: "offline", message: "x", enginesStopped: false });
    await click(document.getElementById("check-updates"));
    expect(sheetText()).toContain("could not reach the update server");
  });

  it("does not start a second check while one is running", async () => {
    const { api } = make({ settings: { autoUpdate: false } });
    await flush();
    let release!: (v: UpdateInfo | null) => void;
    api.checkUpdate.mockReturnValueOnce(new Promise((r) => (release = r)));
    (document.getElementById("check-updates") as HTMLElement).click();
    (document.getElementById("check-updates") as HTMLElement).click();
    await flush();
    expect(api.checkUpdate).toHaveBeenCalledTimes(1);
    release(null);
    await flush();
  });
});

describe("the question", () => {
  it("shows the release notes as text, with Later and Download and Install", async () => {
    make({ found: info({ notes: "Fixes <img src=x onerror=alert(1)> and more" }) });
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    await click(pill());
    expect(sheetText()).toContain("Version 0.2.0 is available");
    expect(sheetText()).toContain("You have version 0.1.0");
    expect(layer().querySelector(".update-notes")?.textContent).toBe("Fixes  and more");
    expect(layer().querySelector("img")).toBeNull();
    expect(button("Later")).toBeDefined();
    expect(button("Download and Install")).toBeDefined();
    expect(button("Open the releases page")).toBeUndefined();
  });

  it("Later and Escape both leave the pill so the person can come back to it", async () => {
    const { api } = make();
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    await click(pill());
    await click(button("Later"));
    expect(layer().hidden).toBe(true);
    expect(pill().hidden).toBe(false);
    await click(pill());
    layer().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flush();
    expect(layer().hidden).toBe(true);
    expect(api.installUpdate).not.toHaveBeenCalled();
  });

  it("a manual check that finds an update opens the question at once", async () => {
    make({ settings: { autoUpdate: false } });
    await flush();
    await click(document.getElementById("check-updates"));
    expect(sheetText()).toContain("Version 0.2.0 is available");
  });
});

describe("installing", () => {
  it("downloads with progress, takes the open documents when the download is done, then asks for the restart", async () => {
    const { api, openPaths } = make();
    let report!: Parameters<UpdaterDeps["api"]["downloadUpdate"]>[0];
    let finishDownload!: () => void;
    api.downloadUpdate.mockImplementationOnce(
      (on: typeof report) =>
        new Promise<void>((resolve) => {
          report = on;
          finishDownload = resolve;
        }),
    );
    let finishInstall!: () => void;
    api.installUpdate.mockImplementationOnce(() => new Promise<void>((resolve) => (finishInstall = resolve)));
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    await click(pill());
    await click(button("Download and Install"));

    // During the download nothing is asked of the open files: a file opened now still counts.
    expect(openPaths).not.toHaveBeenCalled();
    expect(api.installUpdate).not.toHaveBeenCalled();
    expect(sheetText()).toContain("Downloading version 0.2.0");
    expect(pill().textContent).toContain("Updating");
    const bar = layer().querySelector("progress") as HTMLProgressElement;
    expect(bar.hasAttribute("value")).toBe(false);

    report({ event: "progress", chunk: 1048576, total: 4194304 });
    expect(bar.max).toBe(4194304);
    expect(bar.value).toBe(1048576);
    expect(sheetText()).toContain("25 % of 4.0 MiB");
    report({ event: "progress", chunk: 3145728, total: 4194304 });
    expect(sheetText()).toContain("100 % of 4.0 MiB");

    finishDownload();
    await flush();
    // Now, right before the install, the reader's place is saved and the file in view is named.
    expect(openPaths).toHaveBeenCalledTimes(1);
    expect(api.installUpdate).toHaveBeenCalledWith("/docs/a.pdf");
    expect(sheetText()).toContain("Installing");

    finishInstall();
    await flush();
    expect(layer().querySelector("progress")).toBeNull();
    expect(sheetText()).toContain("Restart to update");
    expect(pill().textContent).toContain("Restart to update");
    expect(api.restartApp).not.toHaveBeenCalled();
    await click(button("Restart to Update"));
    expect(api.restartApp).toHaveBeenCalledTimes(1);
    expect(layer().hidden).toBe(true);
  });

  it("with no document open, the install is told there is none in view", async () => {
    const { api } = make({ paths: [] });
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    await click(pill());
    await click(button("Download and Install"));
    expect(api.installUpdate).toHaveBeenCalledWith(null);
  });

  it("the restart question cannot be dismissed with Escape", async () => {
    const { api } = make();
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    await click(pill());
    await click(button("Download and Install"));
    layer().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await flush();
    expect(button("Restart to Update")).toBeDefined();
    expect(api.restartApp).not.toHaveBeenCalled();
  });

  it("a download that fails leaves the update available and the documents alone", async () => {
    const { api, openPaths } = make();
    api.downloadUpdate.mockRejectedValueOnce({ kind: "offline", message: "x", enginesStopped: false });
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    await click(pill());
    await click(button("Download and Install"));
    expect(sheetText()).toContain("The update was not installed");
    expect(sheetText()).toContain("could not reach the update server");
    expect(button("Restart")).toBeUndefined();
    expect(api.installUpdate).not.toHaveBeenCalled();
    expect(openPaths).not.toHaveBeenCalled();
    await click(button("OK"));
    expect(pill().hidden).toBe(false);
    expect(pill().textContent).toContain("Update 0.2.0");
    expect(api.restartApp).not.toHaveBeenCalled();
  });

  it("an install that fails after the engines were stopped offers a restart to get the documents back", async () => {
    const { api } = make();
    api.installUpdate.mockRejectedValueOnce({ kind: "other", message: "no room", enginesStopped: true });
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    await click(pill());
    await click(button("Download and Install"));
    expect(sheetText()).toContain("The update could not be installed");
    expect(sheetText()).toContain("no room");
    expect(sheetText()).toContain("open documents were closed");
    await click(button("Restart"));
    expect(api.restartApp).toHaveBeenCalledTimes(1);
  });
});

describe("the setting that turns the check off", () => {
  it("tells the person when it could not be saved, and shows the switch as it was", async () => {
    const { api } = make();
    await flush();
    const toggle = document.getElementById("auto-update") as HTMLButtonElement;
    api.setAutoUpdate.mockRejectedValueOnce({ code: "internal", message: "disk full" });
    await click(toggle);
    expect(sheetText()).toContain("The setting was not saved");
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    // The automatic check goes on, as the switch says.
    await click(button("OK"));
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    expect(api.checkUpdate).toHaveBeenCalledTimes(1);
  });
});

describe("a copy that cannot update itself (deb)", () => {
  it("gets a notice and a link to the releases page instead of an install button", async () => {
    const { api } = make({ settings: { method: "manual" }, found: info({ method: "manual" }) });
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    expect(pill().hidden).toBe(false);
    await click(pill());
    expect(sheetText()).toContain("cannot update itself");
    expect(button("Download and Install")).toBeUndefined();
    await click(button("Open the releases page"));
    expect(api.openReleasesPage).toHaveBeenCalledTimes(1);
    expect(api.installUpdate).not.toHaveBeenCalled();
  });

  it("is treated as manual when either the settings or the answer say so", async () => {
    make({ settings: { method: "manual" }, found: info({ method: "automatic" }) });
    await vi.advanceTimersByTimeAsync(FIRST_CHECK_DELAY_MS);
    await click(pill());
    expect(button("Download and Install")).toBeUndefined();
  });
});

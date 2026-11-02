// What the page tools do to the document in front: ask the app process to change it, record the
// change in the document's history so that it can be taken back, tell the view and the side panel
// that the pages are different, and deal with what can go wrong on the way (a password, a signature,
// a stop). The page panel and the crop dialog decide what to change; this decides how.
//
// Every change goes to the engine of the document and is saved with the same save as any other
// change: nothing here writes a file, except the copy of some pages, which the app process writes
// through the save path.

import { isEngineLost, toAppError, type AppErrorLike } from "../lib/errors";
import {
  MAX_EDIT_PAGES,
  TOOL_NOTES,
  describePages,
  pagesWord,
  placesAfterMove,
} from "../lib/pages";
import { describePageError } from "../lib/pageerrors";
import type { SavedFile } from "../lib/api";
import type { InsertOptions, PageToolsApi } from "../lib/pagesapi";
import type { PageSizePt } from "../viewer/layout";
import { showBusyBox, type BusyBox } from "./busybox";
import type { Edit, EditHistory } from "./history";
import type { NoticeOptions, NoticeResult } from "./notice";

/** The document in front, as far as the page tools care. */
export interface PageTarget {
  doc: number;
  name: string;
  history: EditHistory;
  /** The size of every page now. */
  pages: PageSizePt[];
}

export interface PageHost {
  target(): PageTarget | null;
  /** The pages of a document changed: the view and the side panel lay out again. */
  applyPages(doc: number, pages: PageSizePt[], show?: number): void;
  notice(options: NoticeOptions): Promise<NoticeResult>;
  /** The engine of a document was lost, and the document with it. */
  lost(doc: number, error: AppErrorLike): Promise<void>;
  /** The system's dialog for picking PDF files; empty when it is cancelled. */
  pickFiles(): Promise<string[]>;
  /** Says something to a screen reader. */
  announce(text: string): void;
  /** Where the box with the progress bar goes. */
  layer: HTMLElement;
}

export interface PageOpsHooks {
  /** Told when a tool starts and stops working. */
  busyChanged?(busy: boolean): void;
  /** How long a tool may work before the box with the progress bar shows, in milliseconds. */
  busyDelayMs?: number;
}

/** What a tool did, for the panel: the pages to pick afterwards and the page to show. */
export interface OpResult {
  select: number[];
  show?: number;
}

const OK = [{ id: "ok", label: "OK", primary: true }];

export class PageOps {
  private working = false;
  private readonly idleWaiters: (() => void)[] = [];
  private box: BusyBox | null = null;
  private readonly delay: number;

  constructor(
    private readonly api: PageToolsApi,
    private readonly host: PageHost,
    private readonly hooks: PageOpsHooks = {},
  ) {
    this.delay = hooks.busyDelayMs ?? 400;
  }

  /** True while a tool (or taking one back) is working. Only one works at a time. */
  get busy(): boolean {
    return this.working;
  }

  /** Resolves when no tool is working (at once if none is). A save waits for this. */
  whenIdle(): Promise<void> {
    if (!this.working) return Promise.resolve();
    return new Promise((resolve) => this.idleWaiters.push(resolve));
  }

  // ---- the tools -------------------------------------------------------------------------------

  /** Removes pages (counted from 0). */
  async remove(pages: number[]): Promise<OpResult | null> {
    const t = await this.start(pages);
    if (!t) return null;
    if (pages.length >= t.pages.length) {
      await this.host.notice({
        title: "A document needs at least one page",
        detail: "Pick the pages to delete, and leave at least one.",
        buttons: OK,
      });
      return null;
    }
    const sorted = [...new Set(pages)].sort((a, b) => a - b);
    return this.run(t, `Deleting ${pagesWord(sorted.length)}`, async () => {
      const r = await this.api.pagesDelete(t.doc, sorted);
      t.history.push(this.snapshotEdit(t, `Delete ${pagesWord(sorted.length)}`, r.snapshot));
      const show = Math.min(sorted[0], r.pages.length - 1);
      this.host.applyPages(t.doc, r.pages, show);
      this.host.announce(`Deleted ${describePages(sorted)}. ${pagesWord(r.pages.length)} left.`);
      if (r.snapshot === 0) void this.host.notice({ title: "This cannot be taken back", detail: TOOL_NOTES.undoGone, buttons: OK });
      if (r.formFields > 0) {
        void this.host.notice({ title: "Form fields stay in the file", detail: TOOL_NOTES.deleteForms, buttons: OK });
      }
      return { select: [show], show };
    });
  }

  /** Puts the pages in a new order (for every place, the page that goes there). */
  async reorder(order: number[], moved: number[]): Promise<OpResult | null> {
    const t = await this.start(order);
    if (!t) return null;
    return this.run(t, "Moving pages", async () => {
      const r = await this.api.pagesReorder(t.doc, order);
      t.history.push(this.snapshotEdit(t, `Move ${pagesWord(moved.length)}`, r.snapshot));
      const places = placesAfterMove(order, moved);
      this.host.applyPages(t.doc, r.pages, places[0]);
      this.host.announce(`Moved ${pagesWord(moved.length)} to ${describePages(places)}.`);
      if (r.snapshot === 0) void this.host.notice({ title: "This cannot be taken back", detail: TOOL_NOTES.undoGone, buttons: OK });
      return { select: places, show: places[0] };
    });
  }

  /** Turns pages clockwise by 1 to 3 quarter turns. */
  async rotate(pages: number[], turns: 1 | 2 | 3): Promise<OpResult | null> {
    const t = await this.start(pages);
    if (!t) return null;
    const sorted = [...new Set(pages)].sort((a, b) => a - b);
    return this.run(t, "Turning pages", async () => {
      const r = await this.api.pagesRotate(t.doc, sorted, turns);
      const back = (4 - turns) as 1 | 2 | 3;
      t.history.push({
        label: `Turn ${pagesWord(sorted.length)}`,
        undo: () => this.exclusive(async () => this.applyOnly(t.doc, await this.api.pagesRotate(t.doc, sorted, back))),
        redo: () => this.exclusive(async () => this.applyOnly(t.doc, await this.api.pagesRotate(t.doc, sorted, turns))),
      });
      this.host.applyPages(t.doc, r.pages);
      this.host.announce(`Turned ${describePages(sorted)}.`);
      return { select: sorted };
    });
  }

  /** Crops pages: `margins` are left, top, right, bottom in points on the page as it is drawn. */
  async crop(pages: number[], margins: [number, number, number, number]): Promise<OpResult | null> {
    const t = await this.start(pages);
    if (!t) return null;
    const sorted = [...new Set(pages)].sort((a, b) => a - b);
    return this.run(t, "Cropping pages", async () => {
      const r = await this.api.pagesCrop(t.doc, sorted, margins);
      t.history.push({
        label: `Crop ${pagesWord(sorted.length)}`,
        undo: () => this.exclusive(async () => this.applyOnly(t.doc, await this.api.pagesSetBoxes(t.doc, sorted, r.before))),
        redo: () => this.exclusive(async () => this.applyOnly(t.doc, await this.api.pagesSetBoxes(t.doc, sorted, r.after))),
      });
      this.host.applyPages(t.doc, r.pages);
      this.host.announce(`Cropped ${describePages(sorted)}. The edges are hidden, not removed.`);
      return { select: sorted };
    });
  }

  /** Adds an empty page of the size given (points) so that it becomes page `at`. */
  async insertBlank(at: number, width: number, height: number): Promise<OpResult | null> {
    const t = await this.start([]);
    if (!t) return null;
    return this.run(t, "Adding a page", async () => {
      const r = await this.api.pagesInsertBlank(t.doc, at, width, height);
      t.history.push(this.snapshotEdit(t, "Add a blank page", r.snapshot));
      this.host.applyPages(t.doc, r.pages, at);
      this.host.announce(`Added a blank page as page ${at + 1}.`);
      if (r.snapshot === 0) void this.host.notice({ title: "This cannot be taken back", detail: TOOL_NOTES.undoGone, buttons: OK });
      return { select: [at], show: at };
    });
  }

  /**
   * Asks for PDF files and inserts all the pages of each, in the order the system gives them, so
   * that the first becomes page `at`. A file that needs a password asks for it; a protected or a
   * signed file asks whether its pages may come without that.
   */
  async insertFiles(at: number, paths?: string[]): Promise<OpResult | null> {
    const t = await this.start([]);
    if (!t) return null;
    const files = paths ?? (await this.host.pickFiles());
    if (files.length === 0) return null;
    const first = at;
    let where = at;
    let count = 0;
    const result = await this.run(t, "Adding pages", async () => {
      for (const path of files) {
        const inserted = await this.insertOne(t, where, path);
        if (inserted === null) break;
        where += inserted;
        count += inserted;
      }
      return count > 0 ? { select: Array.from({ length: count }, (_, i) => first + i), show: first } : null;
    });
    return result;
  }

  /** Copies pages into a new file the user chooses. Resolves with the file, or null when stopped. */
  async extract(pages: number[]): Promise<SavedFile | null> {
    const t = await this.start(pages);
    if (!t) return null;
    const sorted = [...new Set(pages)].sort((a, b) => a - b);
    const saved = await this.run(t, `Copying ${pagesWord(sorted.length)}`, async () => {
      let options = { allowSigned: false, allowUnprotected: false };
      for (;;) {
        try {
          // The dialog of the system is open while this waits: the box with the bar is not needed.
          return await this.api.pagesExtract(t.doc, sorted, options, (d, n) => this.box?.progress(d, n));
        } catch (e) {
          const error = toAppError(e);
          if (error.code === "save_signed" && !options.allowSigned) {
            if (!(await this.confirm(`"${t.name}" ${TOOL_NOTES.signedCopy}`, "Copy the pages anyway", "The pages come without the signature"))) return null;
            options = { ...options, allowSigned: true };
          } else if (error.code === "save_unprotected" && !options.allowUnprotected) {
            if (!(await this.confirm(`"${t.name}" ${TOOL_NOTES.unprotectedCopy}`, "Copy the pages anyway", "The new file will not be protected"))) return null;
            options = { ...options, allowUnprotected: true };
          } else {
            throw e;
          }
        }
      }
    });
    if (saved) {
      this.host.announce(`Saved ${pagesWord(sorted.length)} to ${saved.name}.`);
      void this.host.notice({
        title: "The pages were copied",
        detail: `${pagesWord(sorted.length)} of "${t.name}" are in "${saved.name}". ${TOOL_NOTES.extract}`,
        buttons: OK,
      });
    }
    return saved ?? null;
  }

  // ---- inserting one file ----------------------------------------------------------------------

  private async insertOne(t: PageTarget, at: number, path: string): Promise<number | null> {
    const name = path.split(/[\\/]/).pop() ?? path;
    let options: InsertOptions = {};
    for (;;) {
      try {
        const r = await this.api.pagesInsertPdf(t.doc, at, path, options, (d, n) => this.box?.progress(d, n));
        t.history.push(this.snapshotEdit(t, `Add the pages of ${name}`, r.changed.snapshot));
        this.host.applyPages(t.doc, r.changed.pages, at);
        this.host.announce(`Added ${pagesWord(r.inserted)} from ${name}.`);
        if (r.changed.snapshot === 0) {
          void this.host.notice({ title: "This cannot be taken back", detail: TOOL_NOTES.undoGone, buttons: OK });
        }
        return r.inserted;
      } catch (e) {
        const error = toAppError(e);
        if (error.code === "password_required" || error.code === "password_incorrect") {
          const password = await this.askPassword(name, error.code === "password_incorrect");
          if (password === null) return null;
          options = { ...options, password };
        } else if (error.code === "insert_protected" && !options.allowProtected) {
          if (!(await this.confirm(`"${name}" ${TOOL_NOTES.protectedSource}`, "Add the pages anyway", "The pages join without protection"))) return null;
          options = { ...options, allowProtected: true };
        } else if (error.code === "insert_signed" && !options.allowSigned) {
          if (!(await this.confirm(`"${name}" ${TOOL_NOTES.signedSource}`, "Add the pages anyway", "The pages come without the signature"))) return null;
          options = { ...options, allowSigned: true };
        } else {
          throw e;
        }
      }
    }
  }

  private async askPassword(name: string, wrong: boolean): Promise<string | null> {
    this.box?.hide();
    try {
      const answer = await this.host.notice({
        title: wrong ? "That password is not correct" : "This PDF is password protected",
        detail: `"${name}" needs a password before its pages can be added.`,
        password: true,
        buttons: [
          { id: "cancel", label: "Cancel" },
          { id: "open", label: "Add the pages", primary: true },
        ],
      });
      return answer.button === "open" && answer.password ? answer.password : null;
    } finally {
      this.box?.show();
    }
  }

  private async confirm(detail: string, yes: string, title: string): Promise<boolean> {
    this.box?.hide();
    try {
      const answer = await this.host.notice({
        title,
        detail,
        buttons: [
          { id: "cancel", label: "Cancel", safe: true },
          { id: "yes", label: yes, primary: true },
        ],
      });
      return answer.button === "yes";
    } finally {
      this.box?.show();
    }
  }

  // ---- going back and forward -------------------------------------------------------------------

  /**
   * An edit for a change that needed a copy of the document: taking it back loads the copy, and
   * the engine keeps the state it leaves under another number, which making the change again
   * loads. So going back and going forward are the same step, with the number in hand each time.
   */
  private snapshotEdit(t: PageTarget, label: string, first: number): Edit {
    let other = first;
    const step = () =>
      this.exclusive(async () => {
        if (other === 0) throw { code: "undo_unavailable", message: TOOL_NOTES.undoGone };
        const r = await this.api.pagesRestore(t.doc, other);
        other = r.snapshot;
        this.applyOnly(t.doc, r);
      });
    return { label, undo: step, redo: step };
  }

  private applyOnly(doc: number, r: { pages: PageSizePt[] }): void {
    this.host.applyPages(doc, r.pages);
  }

  /** Runs taking a change back or making it again, so that nothing else works at the same time. */
  private async exclusive<T>(work: () => Promise<T>): Promise<T> {
    if (this.working) throw { code: "busy", message: "another page tool is working" };
    this.setWorking(true);
    try {
      return await work();
    } catch (e) {
      const error = toAppError(e);
      const t = this.host.target();
      if (t && isEngineLost(error)) void this.host.lost(t.doc, error);
      throw e;
    } finally {
      this.setWorking(false);
    }
  }

  // ---- running a tool ---------------------------------------------------------------------------

  /** Checks that a tool may start: a document, nothing else working, a list that fits. */
  private async start(pages: number[]): Promise<PageTarget | null> {
    const t = this.host.target();
    if (!t || this.working) return null;
    if (pages.length > MAX_EDIT_PAGES || t.pages.length > MAX_EDIT_PAGES) {
      await this.host.notice({
        ...describePageError({ code: "too_many_pages", message: "" }, t.name),
        buttons: OK,
      });
      return null;
    }
    return t;
  }

  private async run<T>(t: PageTarget, title: string, work: () => Promise<T>): Promise<T | null> {
    // Something else started while a dialog was open (the file dialog of an insert, say).
    if (this.working) return null;
    this.setWorking(true);
    const timer = setTimeout(() => {
      this.box = showBusyBox(this.host.layer, {
        title,
        detail: "Patient Partridge is working on the pages.",
        cancelLabel: "Stop",
        onCancel: () => void this.stop(t),
      });
    }, this.delay);
    try {
      return await work();
    } catch (e) {
      await this.fail(t, e);
      return null;
    } finally {
      clearTimeout(timer);
      this.box?.close();
      this.box = null;
      this.setWorking(false);
    }
  }

  private async stop(t: PageTarget): Promise<void> {
    try {
      await this.api.pagesCancel(t.doc);
    } catch {
      // The work fails or finishes by itself; there is nothing to add.
    }
  }

  private async fail(t: PageTarget, error: unknown): Promise<void> {
    const app = toAppError(error);
    if (app.code === "cancelled") {
      this.host.announce("Stopped. The document is as it was.");
      return;
    }
    if (isEngineLost(app)) {
      await this.host.lost(t.doc, app);
      return;
    }
    const text = describePageError(app, t.name);
    await this.host.notice({ title: text.title, detail: text.detail, buttons: OK });
  }

  private setWorking(on: boolean): void {
    if (this.working === on) return;
    this.working = on;
    this.hooks.busyChanged?.(on);
    if (!on) for (const resolve of this.idleWaiters.splice(0)) resolve();
  }
}

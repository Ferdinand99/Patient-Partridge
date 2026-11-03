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
import type { DesignOp } from "../lib/formdesign";
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
  /**
   * The document with this number, whether or not it is the one in front. A change that was made for
   * a document is made to that document: a tab that was left a moment ago still gets its changes.
   * Without it only the document in front can be found.
   */
  targetOf?(doc: number): PageTarget | null;
  /** The pages of a document changed: the view and the side panel lay out again. */
  applyPages(doc: number, pages: PageSizePt[], show?: number): void;
  notice(options: NoticeOptions): Promise<NoticeResult>;
  /** The engine of a document was lost, and the document with it. */
  lost(doc: number, error: AppErrorLike): Promise<void>;
  /**
   * Gets what is under way on a document done before a tool changes its pages (text typed in a field
   * is sent, the box of a note is saved, running changes are made). Resolves with false, after saying
   * why, when the tool must not begin.
   */
  settle?(doc: number, options?: { copies: boolean }): Promise<boolean>;
  /**
   * Asked before a step back or forward changes the pages: resolves with false, after saying why,
   * when text a field refused would be lost with the pages it was typed on.
   */
  keepsRefusedText?(doc: number): Promise<boolean>;
  /** The system's dialog for picking PDF files; empty when it is cancelled. */
  pickFiles(): Promise<string[]>;
  /** Says something to a screen reader. */
  announce(text: string): void;
  /** Where the box with the progress bar goes: a strip that covers nothing, apart from the message boxes. */
  busyLayer: HTMLElement;
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

/** The code of the quiet refusal of a step back or forward while a field holds refused text. */
export const REFUSED_FIELD_CODE = "refused_field";

/**
 * What became of a batch of changes to the form. `done`: the engine made them. `refused`: the engine or
 * the checks before it said no, the reader was told why, and the document is as it was. `busy`: another
 * tool was working, nothing was sent, and the batch can be sent again. `unsettled`: what comes before a
 * change to the document did not finish (the reader was told why), nothing was sent, and the batch can
 * be sent again when the reader has dealt with it. `gone`: the document is not open any more.
 */
export type DesignResult = "done" | "refused" | "busy" | "unsettled" | "gone";

/** How a tool runs: whether the user may stop it, and whether the box waits for a dialog. */
interface RunOptions {
  /** The tool changes the fields of the form, not the pages (only the words of the box differ). */
  form?: boolean;
  /** The work can be stopped without a cost (bringing pages in, copying pages out). */
  stoppable?: boolean;
  /** The box is held back until `release()` (a dialog of the system is open). */
  held?: boolean;
  /** The tool only reads the document (copying pages out): nothing in it is changed. */
  copies?: boolean;
}

/** What failed, besides the error: the other file of an insert, and what had been added before. */
interface FailContext {
  /** The name of the file whose pages were being added. */
  source?: string;
  /** How many pages of the files before it were added and stay. */
  added?: number;
}

export class PageOps {
  private working = false;
  /** The document the running tool (or a step back or forward) is changing. */
  private workingDoc: number | null = null;
  /** The running tool only copies pages out of the document. */
  private copying = false;
  private readonly idleWaiters: (() => void)[] = [];
  private box: BusyBox | null = null;
  private readonly delay: number;
  /** What the running tool shows in its box, and whether the delay has run out. */
  private running: { title: string; stoppable: boolean; doc: number; name: string; form: boolean } | null = null;
  private due = false;
  /** The settling that comes before a tool is going on (the designer sends its changes then). */
  private settling = false;
  /** A Stop was asked for during this run, whether or not it could end the step in hand. */
  private stopAsked = false;
  private holds = 0;
  /** The notes already shown for a document, so that each is said once. */
  private readonly noted = new Set<string>();

  constructor(
    private readonly api: PageToolsApi,
    private readonly host: PageHost,
    private readonly hooks: PageOpsHooks = {},
  ) {
    this.delay = hooks.busyDelayMs ?? 400;
  }

  /**
   * True while what comes before a tool is going on (the designer sends its changes then). A change the
   * designer is asked for at that time is made inside the work of that tool.
   */
  get isSettling(): boolean {
    return this.settling;
  }

  /** The document that a tool is changing right now (a step back or forward too), or null. */
  get busyDoc(): number | null {
    return this.working ? this.workingDoc : null;
  }

  /** True while a tool is changing the document `doc`. A tool on another document is not in the way. */
  busyOn(doc: number): boolean {
    return this.working && this.workingDoc === doc;
  }

  /** True while pages are being copied out of the document `doc`, which changes nothing in it. */
  copyingOn(doc: number): boolean {
    return this.busyOn(doc) && this.copying;
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
      this.tellWhatStays(t, r.formFields);
      return { select: [show], show };
    });
  }

  /**
   * What a delete leaves in the file, said the first time a page is deleted from a document, and
   * what the form fields on the pages leave, said each time there were any.
   */
  private tellWhatStays(t: PageTarget, formFields: number): void {
    const first = this.firstTime(t.doc, "delete");
    if (!first && formFields === 0) return;
    void this.host.notice({
      title: formFields > 0 ? "Form fields stay in the file" : "What a delete leaves in the file",
      detail: [first ? TOOL_NOTES.delete : "", formFields > 0 ? TOOL_NOTES.deleteForms : ""].filter((s) => s !== "").join(" "),
      buttons: OK,
    });
  }

  private firstTime(doc: number, what: string): boolean {
    const key = `${doc}:${what}`;
    if (this.noted.has(key)) return false;
    this.noted.add(key);
    return true;
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
        undo: () => this.exclusive(t.doc, async () => this.applyOnly(t.doc, await this.api.pagesRotate(t.doc, sorted, back))),
        redo: () => this.exclusive(t.doc, async () => this.applyOnly(t.doc, await this.api.pagesRotate(t.doc, sorted, turns))),
      });
      this.host.applyPages(t.doc, r.pages);
      this.host.announce(`Turned ${describePages(sorted)}.`);
      return { select: sorted };
    });
  }

  /** Crops pages: `margins` are left, top, right, bottom in points on the page as it is drawn. */
  async crop(pages: number[], margins: [number, number, number, number], doc?: number): Promise<OpResult | null> {
    const t = await this.start(pages);
    // The box that asked for the margins was open on a document; it is cropped only if it is still
    // the one in front.
    if (!t || (doc !== undefined && t.doc !== doc)) return null;
    const sorted = [...new Set(pages)].sort((a, b) => a - b);
    return this.run(t, "Cropping pages", async () => {
      const r = await this.api.pagesCrop(t.doc, sorted, margins);
      t.history.push({
        label: `Crop ${pagesWord(sorted.length)}`,
        undo: () => this.exclusive(t.doc, async () => this.applyOnly(t.doc, await this.api.pagesSetBoxes(t.doc, sorted, r.before))),
        redo: () => this.exclusive(t.doc, async () => this.applyOnly(t.doc, await this.api.pagesSetBoxes(t.doc, sorted, r.after))),
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
    // The dialog of the system was open: the document in front may be another one now, or gone.
    if (this.host.target()?.doc !== t.doc) return null;
    const first = at;
    let where = at;
    let count = 0;
    return this.run(
      t,
      "Adding pages",
      async () => {
        for (const path of files) {
          let inserted: number | null;
          // A Stop that came too late for the file before ends the list after it, and so does one
          // that came between two files: it is not forgotten.
          if (this.stopAsked) {
            this.host.announce(
              count > 0
                ? `Stopped. The ${pagesWord(count)} added before stay; the rest were not added.`
                : "Stopped. The document is as it was.",
            );
            break;
          }
          // A Stop that came too late ended with the file before; the next one can be stopped again.
          this.box?.canStop();
          const name = path.split(/[\\/]/).pop() ?? path;
          try {
            inserted = await this.insertOne(t, where, path);
          } catch (e) {
            // What came before stays; the files after the one that failed are not tried.
            await this.fail(t, e, { source: name, added: count });
            break;
          }
          if (inserted === null) break;
          where += inserted;
          count += inserted;
        }
        return count > 0 ? { select: Array.from({ length: count }, (_, i) => first + i), show: first } : null;
      },
      { stoppable: true },
    );
  }

  /** True when the app process has the form designer. */
  get canDesign(): boolean {
    return this.api.designApply !== undefined;
  }

  /** The document with this number: the one the changes were made for, wherever the reader is now. */
  private targetFor(doc: number): PageTarget | null {
    if (this.host.targetOf) return this.host.targetOf(doc);
    const t = this.host.target();
    return t && t.doc === doc ? t : null;
  }

  /**
   * Makes a batch of changes to the form fields of the document `doc`, with one reload of the document
   * and one step in the history of that document. The changes are made to that document and to no
   * other: it does not matter which tab is in front when they are sent. See `DesignResult`.
   */
  async applyDesign(doc: number, ops: DesignOp[], label: string): Promise<DesignResult> {
    const apply = this.api.designApply;
    if (!apply || ops.length === 0) return "refused";
    // Another tool is about to begin and asks the designer to send what it holds first: the
    // change is made inside the work of that tool, which is already marked as working.
    if (this.settling) return this.applyDesignInside(apply, doc, ops, label);
    // The document is found now, before anything is waited for.
    const t = this.targetFor(doc);
    if (!t) return "gone";
    if (this.working) return "busy";
    const ready = await this.start([], doc);
    if (!ready) return "refused";
    // Something started while the check of the size of the document was waiting.
    if (this.working) return "busy";
    let reached = false;
    const done = await this.run(
      ready,
      "Changing the form",
      async () => {
        reached = true;
        const r = await apply.call(this.api, ready.doc, ops);
        ready.history.push(this.snapshotEdit(ready, label, r.snapshot));
        this.host.applyPages(ready.doc, r.pages);
        if (r.snapshot === 0) void this.host.notice({ title: "This cannot be taken back", detail: TOOL_NOTES.undoGone, buttons: OK });
        return true;
      },
      { form: true },
    );
    if (done === true) return "done";
    return reached ? "refused" : "unsettled";
  }

  private async applyDesignInside(
    apply: NonNullable<PageToolsApi["designApply"]>,
    doc: number,
    ops: DesignOp[],
    label: string,
  ): Promise<DesignResult> {
    const t = this.targetFor(doc);
    if (!t) return "gone";
    try {
      const r = await apply.call(this.api, t.doc, ops);
      t.history.push(this.snapshotEdit(t, label, r.snapshot));
      this.host.applyPages(t.doc, r.pages);
      return "done";
    } catch (e) {
      await this.fail(t, e);
      return "refused";
    }
  }

  /** Copies pages into a new file the user chooses. Resolves with the file, or null when stopped. */
  async extract(pages: number[]): Promise<SavedFile | null> {
    const t = await this.start(pages);
    if (!t) return null;
    const sorted = [...new Set(pages)].sort((a, b) => a - b);
    const saved = await this.run(
      t,
      `Copying ${pagesWord(sorted.length)}`,
      async () => {
        let options = { allowSigned: false, allowUnprotected: false };
        for (;;) {
          try {
            // The dialog of the system is open while this waits, and the box with the bar is held
            // back until the first piece of the new file moves.
            let moved = false;
            return await this.api.pagesExtract(t.doc, sorted, options, (d, n) => {
              if (!moved) {
                moved = true;
                this.release();
              }
              this.box?.progress(d, n);
            });
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
      },
      { stoppable: true, held: true, copies: true },
    );
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
        if (this.firstTime(t.doc, "insert")) {
          void this.host.notice({ title: "What comes with added pages", detail: TOOL_NOTES.insert, buttons: OK });
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
    this.hold();
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
      this.release();
    }
  }

  private async confirm(detail: string, yes: string, title: string): Promise<boolean> {
    this.hold();
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
      this.release();
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
      this.exclusive(t.doc, async () => {
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

  /**
   * Runs taking a change back or making it again for the document `doc`, so that nothing else works
   * at the same time. When the engine of that document is lost, that document is closed (not the
   * one in front, which the user may have gone to while this ran), and the error is passed on so
   * that the caller knows there is nothing more to say about it.
   */
  private async exclusive<T>(doc: number, work: () => Promise<T>): Promise<T> {
    if (this.working) throw { code: "busy", message: "another page tool is working" };
    // The new pages replace the layer of the form, and with it the text a field refused. The reader
    // was told why the step did not happen, so the caller has nothing more to say.
    if (this.host.keepsRefusedText && !(await this.host.keepsRefusedText(doc))) {
      throw { code: REFUSED_FIELD_CODE, message: "a field holds text that was refused" };
    }
    if (this.working) throw { code: "busy", message: "another page tool is working" };
    this.setWorking(true, doc);
    try {
      return await work();
    } catch (e) {
      const error = toAppError(e);
      if (isEngineLost(error)) void this.host.lost(doc, error);
      throw e;
    } finally {
      this.setWorking(false);
    }
  }

  // ---- running a tool ---------------------------------------------------------------------------

  /** Checks that a tool may start: a document, nothing else working, a list that fits. */
  private async start(pages: number[], doc?: number): Promise<PageTarget | null> {
    const t = doc === undefined ? this.host.target() : this.targetFor(doc);
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

  private async run<T>(t: PageTarget, title: string, work: () => Promise<T>, options: RunOptions = {}): Promise<T | null> {
    // Something else started while a dialog was open (the file dialog of an insert, say).
    if (this.working) return null;
    this.setWorking(true, t.doc);
    this.copying = options.copies === true;
    this.running = { title, stoppable: options.stoppable === true, doc: t.doc, name: t.name, form: options.form === true };
    this.due = false;
    this.stopAsked = false;
    this.holds = options.held ? 1 : 0;
    const timer = setTimeout(() => {
      this.due = true;
      this.showBox();
    }, this.delay);
    try {
      if (this.host.settle) {
        this.settling = true;
        let ready: boolean;
        try {
          ready = await this.host.settle(t.doc, { copies: options.copies === true });
        } finally {
          this.settling = false;
        }
        if (!ready) return null;
      }
      return await work();
    } catch (e) {
      await this.fail(t, e);
      return null;
    } finally {
      clearTimeout(timer);
      this.box?.close();
      this.box = null;
      this.running = null;
      this.due = false;
      this.holds = 0;
      this.setWorking(false);
    }
  }

  /** The box with the bar shows when the delay has run out and nothing is held back. */
  private showBox(): void {
    const run = this.running;
    if (!run || !this.due || this.holds > 0 || this.box) return;
    const what = run.form ? "the form" : "the pages";
    this.box = showBusyBox(
      this.host.busyLayer,
      run.stoppable
        ? {
            title: run.title,
            detail: `Patient Partridge is working on ${what} of "${run.name}".`,
            cancelLabel: "Stop",
            onCancel: () => void this.stop(run.doc),
          }
        : {
            title: run.title,
            detail: `Patient Partridge is working on ${what} of "${run.name}". ${run.form ? TOOL_NOTES.workingForm : TOOL_NOTES.working}`,
          },
    );
  }

  /** Holds the box back while a question or a dialog of the system is open. */
  private hold(): void {
    this.holds += 1;
    this.box?.hide();
  }

  private release(): void {
    if (this.holds === 0) return;
    this.holds -= 1;
    if (this.holds > 0) return;
    if (this.box) this.box.show();
    else this.showBox();
  }

  /** Stops what can be stopped without a cost. A change that is under way is left to finish. */
  private async stop(doc: number): Promise<void> {
    this.stopAsked = true;
    try {
      const result = await this.api.pagesCancel(doc);
      if (result === "working") this.box?.cannotStop(TOOL_NOTES.stopTooLate);
    } catch {
      // The work fails or finishes by itself; there is nothing to add.
    }
  }

  private async fail(t: PageTarget, error: unknown, context: FailContext = {}): Promise<void> {
    const app = toAppError(error);
    const earlier = context.added ?? 0;
    if (app.code === "cancelled") {
      this.host.announce(
        earlier > 0
          ? `Stopped. The ${pagesWord(earlier)} added before stay; the rest were not added.`
          : "Stopped. The document is as it was.",
      );
      return;
    }
    if (isEngineLost(app)) {
      await this.host.lost(t.doc, app);
      return;
    }
    const text = describePageError(app, t.name, context.source);
    const detail =
      earlier > 0
        ? `${text.detail.replace(/ "[^"]*" was not changed\.$/, "")} The ${pagesWord(earlier)} from the files before it were added and stay in "${t.name}".`
        : text.detail;
    await this.host.notice({ title: text.title, detail, buttons: OK });
  }

  private setWorking(on: boolean, doc: number | null = null): void {
    if (this.working === on) return;
    this.working = on;
    this.workingDoc = on ? doc : null;
    if (!on) this.copying = false;
    this.hooks.busyChanged?.(on);
    if (!on) for (const resolve of this.idleWaiters.splice(0)) resolve();
  }
}

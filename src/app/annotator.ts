// The annotations of one document: the tool in use, the colour, what is picked, and every change
// (add, move, recolour, change the text, remove) as an edit that the history of the document can
// take back and make again.
//
// The engine makes the annotations; here nothing is drawn but the line of the pen while it moves.
// Each change asks the engine first, and only then is it put in the history, so the history never
// holds a change that did not happen. Changes, undo and redo run one after the other in one queue;
// the box for the text of a note is not in the queue, it is waited for outside it. A change that
// fails in the middle leaves the document marked as changed, whatever the history says. A change that was taken back is put back by the engine with
// the same id (`restoreAnnotation`), so the edits can name annotations by id for as long as the
// document is open.

import {
  type Annotation,
  type Rgba,
  type StampName,
  type Tool,
  PALETTE,
  PEN_WIDTHS,
  clampToBytes,
  isMarkupTool,
  keepOnPage,
  kindName,
  stampSize,
} from "../lib/annotations";
import { isEngineLost, toAppError } from "../lib/errors";
import type { AnnotLayerEvents } from "../viewer/annotlayer";
import { MAX_SELECTION_RANGES } from "../viewer/overlays";
import type { DocumentView } from "../viewer/viewer";
import type { EditHistory, Edit } from "./history";
import type { NoticeOptions, NoticeResult } from "./notice";
import { openNoteEditor, type NoteEditor } from "./noteeditor";

export interface AnnotatorApi {
  addMarkup(
    doc: number,
    page: number,
    kind: "highlight" | "underline" | "strikeout",
    color: Rgba,
    start: number,
    end: number,
  ): Promise<Annotation>;
  addNote(doc: number, page: number, x: number, y: number, color: Rgba, text: string): Promise<Annotation>;
  addInk(doc: number, page: number, color: Rgba, width: number, strokes: [number, number][][]): Promise<Annotation>;
  addStamp(doc: number, page: number, x: number, y: number, stamp: StampName): Promise<Annotation>;
  updateAnnotation(
    doc: number,
    page: number,
    id: number,
    change: { text?: string; color?: Rgba; dx?: number; dy?: number },
  ): Promise<Annotation>;
  removeAnnotation(doc: number, page: number, id: number): Promise<void>;
  restoreAnnotation(doc: number, page: number, id: number): Promise<Annotation>;
}

export interface AnnotatorHost {
  /** Where the box for the text of a note is put. */
  popovers: HTMLElement;
  /** Says something to a screen reader. */
  announce(text: string): void;
  /** Asks the user a question. */
  ask(options: NoticeOptions): Promise<NoticeResult>;
  /** A change failed: the user is told (and a lost engine ends the tab). */
  failed(error: unknown): void;
  /** The toolbar has to look at the state again. */
  changed(): void;
  /** The picture of a page changed (the thumbnail of it has to be drawn again). */
  pageChanged(page: number): void;
}

/** Answers of the engine that say it changed nothing. */
export const CLEAN_REFUSALS = new Set(["annotation_refused", "no_such_annotation", "annotations_not_allowed"]);

/** The colour of the icon of a note: the engine draws every note the same, whatever its colour. */
const NOTE_COLOR: Rgba = PALETTE[0].rgba;

/** Distance, in points, an arrow key moves a picked note; with Shift it moves further. */
const NUDGE = 4;
const NUDGE_FAR = 24;

export class Annotator implements AnnotLayerEvents {
  tool: Tool = "select";
  color: Rgba = PALETTE[0].rgba;
  penWidth: number = PEN_WIDTHS[1].width;
  stamp: StampName = "approved";
  selected: { page: number; id: number } | null = null;

  private chain: Promise<unknown> = Promise.resolve();
  /** How many changes (and undo and redo steps) are waiting or running. */
  private inflight = 0;
  private editor: NoteEditor | null = null;
  /** The whole of what happens to the open box: the wait for it and the change it leads to. */
  private flow: Promise<void> | null = null;
  private destroyed = false;
  private blocked = false;
  /** Changes wait for this before they start (a save is being written). */
  private gate: Promise<unknown> = Promise.resolve();

  constructor(
    private readonly doc: number,
    private readonly view: DocumentView,
    private readonly history: EditHistory,
    private readonly api: AnnotatorApi,
    private readonly host: AnnotatorHost,
  ) {
    this.syncLayer();
  }

  destroy(): void {
    this.destroyed = true;
    this.editor?.close();
  }

  /** The annotation that is picked, if it is on a page that is on screen. */
  get current(): Annotation | null {
    const s = this.selected;
    return s ? (this.view.annotationLayer.find(s.page, s.id) ?? null) : null;
  }

  /** Resolves when the changes asked for so far have been made. */
  idle(): Promise<void> {
    return this.chain.then(
      () => undefined,
      () => undefined,
    );
  }

  /**
   * True while there is something the history does not know yet: a change that is running or
   * waiting, or text typed in the box of a note that has not been saved.
   */
  get unsaved(): boolean {
    return this.inflight > 0 || (this.editor?.dirty ?? false);
  }

  /**
   * Gets everything that is under way done: the box of a note that has text in it is saved as if
   * Save was pressed (one with no new text is closed), and the changes are waited for. A save, a
   * close and a quit call this first, so that they act on the document as it is.
   */
  async flush(): Promise<void> {
    const editor = this.editor;
    if (editor) {
      if (editor.dirty) editor.commit();
      else editor.close();
    }
    await this.flow;
    await this.idle();
  }

  /** Why the annotations cannot be changed, if the document says so; otherwise null. */
  get blockedReason(): string | null {
    return this.blocked ? "This PDF does not allow annotations." : null;
  }

  /** A short line about the annotation that is picked when it cannot be changed, or null. */
  get pickedReason(): string | null {
    const a = this.current;
    if (!a) return null;
    if (a.can.locked) return "This annotation is locked in the file, so it cannot be changed.";
    if (!a.can.editText && a.text !== "" && a.can.remove && a.kind !== "stamp") {
      return "The text of this annotation is too long or is locked in the file, so it cannot be edited here.";
    }
    return null;
  }

  /** Reading mode: the boxes over the pages take no clicks and no focus, so the pages read as before. */
  setReading(reading: boolean): void {
    this.view.annotationLayer.setReading(reading);
  }

  /**
   * Holds back every change asked for from now on until the returned function is called, so that
   * the engine is not changed in the middle of a save (its check would refuse the file). The
   * changes are not lost: they run when it lets go, and count as unsaved meanwhile.
   */
  hold(): () => void {
    let release: () => void = () => undefined;
    this.gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    return release;
  }

  /**
   * Puts work in the queue. The count of what is waiting is kept for the dirty state. A failure is
   * dealt with (`failed`) before the next piece of work starts and before anyone who waits for the
   * queue goes on, and is then passed on.
   */
  private enqueue<T>(work: () => Promise<T>, failed: (error: unknown) => void): Promise<T> {
    this.inflight++;
    this.host.changed();
    const run = Promise.all([this.chain, this.gate])
      .then(() => work())
      .catch((error: unknown) => {
        failed(error);
        throw error;
      })
      .finally(() => {
        this.inflight--;
        this.host.changed();
      });
    this.chain = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  /**
   * Runs undo or redo in turn with the changes: what was asked for before it has been made and is
   * in the history when it starts, and what is asked for later waits. A failure is passed on to the
   * caller, and the document is marked as changed.
   */
  sequenced<T>(work: () => Promise<T>): Promise<T> {
    return this.enqueue(work, (error) => this.touched(error));
  }

  /** Runs changes one after the other, so that two clicks never make their requests tangle. */
  private queued(work: () => Promise<void>): Promise<void> {
    return this.enqueue(work, (error) => {
      this.touched(error);
      if (!this.destroyed) this.host.failed(error);
    }).catch(() => undefined);
  }

  /**
   * A change failed. Unless the engine refused it cleanly, nothing is known of what it did, so the
   * document counts as changed until it is saved.
   */
  private touched(error: unknown): void {
    const code = toAppError(error).code;
    if (code === "annotations_not_allowed") {
      this.blocked = true;
      this.host.changed();
    }
    if (!CLEAN_REFUSALS.has(code)) this.history.markUnsaved();
  }

  private push(edit: Edit): void {
    try {
      this.history.push(edit);
    } catch (error) {
      this.host.failed(error);
    }
  }

  /** Draws a page afresh, and tells whoever shows it elsewhere. */
  private refresh(page: number): void {
    this.view.invalidatePage(page);
    this.host.pageChanged(page);
  }

  private syncLayer(): void {
    this.view.setAnnotationTool(this.tool);
    this.view.annotationLayer.setInk(this.color, this.penWidth);
    this.view.annotationLayer.setSelected(this.selected);
  }

  // ---- what the bar sets ---------------------------------------------------------

  setTool(tool: Tool): void {
    if (this.tool === tool) return;
    this.view.annotationLayer.abort();
    this.tool = tool;
    this.syncLayer();
    this.host.changed();
    // Text that is selected already is marked at once; the person need not select it again.
    if (isMarkupTool(tool) && this.view.hasSelection) void this.markSelection(tool);
    else if (isMarkupTool(tool)) this.host.announce(`${kindName(tool)}: select some text to mark it.`);
    else if (tool === "pen") this.host.announce("Pen: draw on the page with the pointer.");
    else if (tool === "note") this.host.announce("Note: click on the page to place a note.");
    else if (tool === "stamp") this.host.announce("Stamp: click on the page to place the stamp.");
  }

  setColor(color: Rgba): void {
    this.color = color;
    this.view.annotationLayer.setInk(this.color, this.penWidth);
    this.host.changed();
    // A picked annotation that has a colour takes the new one.
    const current = this.current;
    if (current?.can.recolour && this.selected) void this.recolour(this.selected.page, current, color);
  }

  setPenWidth(width: number): void {
    this.penWidth = width;
    this.view.annotationLayer.setInk(this.color, this.penWidth);
    this.host.changed();
  }

  setStamp(stamp: StampName): void {
    this.stamp = stamp;
    this.setTool("stamp");
    this.host.changed();
  }

  select(page: number, id: number | null): void {
    const next = id === null ? null : { page, id };
    if (this.selected?.page === next?.page && this.selected?.id === next?.id) return;
    this.selected = next;
    this.view.annotationLayer.setSelected(next);
    this.host.changed();
    const a = this.current;
    if (a) {
      this.host.announce(`${kindName(a.kind)} selected on page ${page + 1}.${a.can.locked ? " It is locked in the file." : ""}`);
    }
  }

  // ---- layer events -----------------------------------------------------------------

  onSelect(page: number, id: number | null): void {
    this.select(page, id);
  }

  onPlace(page: number, x: number, y: number, client: { x: number; y: number }): void {
    if (this.tool === "note") void this.placeNote(page, x, y, client);
    else if (this.tool === "stamp") void this.placeStamp(page, x, y);
  }

  onStroke(page: number, points: [number, number][]): void {
    void this.drawStroke(page, points);
  }

  onMove(page: number, id: number, dx: number, dy: number): void {
    void this.moveBy(page, id, dx, dy);
  }

  onEdit(page: number, id: number): void {
    void this.editText(page, id);
  }

  /** A list of annotations could not be had. Only a lost engine is worth telling the person. */
  onLoadFailed(_page: number, error: unknown): void {
    if (!this.destroyed && isEngineLost(toAppError(error))) this.host.failed(error);
  }

  /** The text was selected (the pointer was let go): a markup tool marks it at once. */
  selectionSettled(): void {
    if (isMarkupTool(this.tool) && this.view.hasSelection) void this.markSelection(this.tool);
  }

  // ---- markup of the selected text -------------------------------------------------------

  /** Marks the text that is selected. Several pages of it make one edit. */
  async markSelection(kind: "highlight" | "underline" | "strikeout"): Promise<void> {
    const { ranges, truncated } = await this.view.selectedRanges();
    if (truncated) {
      await this.host.ask({
        title: "Too much text is selected",
        detail: `Text on more than ${MAX_SELECTION_RANGES} pages is selected. Select a smaller part and mark it again.`,
        buttons: [{ id: "ok", label: "OK", primary: true }],
      });
      return;
    }
    if (ranges.length === 0) {
      this.host.announce("Select some text first.");
      return;
    }
    const color = this.color;
    await this.queued(async () => {
      const made: { page: number; id: number }[] = [];
      let failure: unknown = null;
      for (const range of ranges) {
        try {
          const a = await this.api.addMarkup(this.doc, range.page, kind, color, range.start, range.end);
          made.push({ page: range.page, id: a.id });
          this.refresh(range.page);
        } catch (error) {
          failure = error;
          break;
        }
      }
      if (made.length > 0) {
        this.view.clearSelection();
        this.record(kindName(kind), made, "remove");
        this.host.announce(`${kindName(kind)} added on ${pagesText(made.map((m) => m.page))}.`);
      }
      if (failure) throw failure;
    });
  }

  /** Records that annotations were added: undoing takes them off, redoing puts them back. */
  private record(label: string, made: { page: number; id: number }[], undoes: "remove" | "restore"): void {
    // A step that fails in the middle of several pages can be tried again: the pages done are not
    // done twice (the engine would refuse them).
    let removed = 0;
    let restored = 0;
    const off = async () => {
      const order = [...made].reverse();
      for (; removed < order.length; removed++) {
        await this.api.removeAnnotation(this.doc, order[removed].page, order[removed].id);
      }
      removed = 0;
      for (const m of made) this.refresh(m.page);
    };
    const on = async () => {
      for (; restored < made.length; restored++) {
        await this.api.restoreAnnotation(this.doc, made[restored].page, made[restored].id);
      }
      restored = 0;
      for (const m of made) this.refresh(m.page);
    };
    this.push(
      undoes === "remove"
        ? { label, undo: off, redo: on }
        : // A removal: undoing puts back.
          { label, undo: on, redo: off },
    );
  }

  // ---- notes, stamps, drawings ------------------------------------------------------------

  private placeNote(page: number, x: number, y: number, client: { x: number; y: number }): Promise<void> {
    if (this.editor) return Promise.resolve();
    // The note starts where it was clicked, its middle under the pointer.
    const at = { x: x - 10, y: y - 10 };
    const editor = openNoteEditor(this.host.popovers, {
      title: "New note",
      text: "",
      near: client,
      onChange: () => this.host.changed(),
    });
    return this.holding(editor, async (text) => {
      const a = await this.api.addNote(this.doc, page, at.x, at.y, NOTE_COLOR, clampToBytes(text));
      this.refresh(page);
      this.record("Note", [{ page, id: a.id }], "remove");
      this.host.announce(`Note added on page ${page + 1}.`);
      this.finishPlacing(page, a.id);
    });
  }

  /**
   * Keeps `editor` as the open box and waits for what the person does with it, outside the queue
   * (a box that stays open must not hold up the undo or the save). When the text is saved, `save`
   * runs in the queue with it.
   */
  private holding(editor: NoteEditor, save: (text: string) => Promise<void>): Promise<void> {
    this.editor = editor;
    this.host.changed();
    const flow = (async () => {
      const text = await editor.result;
      this.editor = null;
      this.host.changed();
      if (text === null || this.destroyed) return;
      await this.queued(() => save(text));
    })();
    this.flow = flow;
    void flow.finally(() => {
      if (this.flow === flow) this.flow = null;
    });
    return flow;
  }

  private placeStamp(page: number, x: number, y: number): Promise<void> {
    const size = stampSize(this.stamp);
    const pageSize = this.view.pageSizeOf(page);
    const [left, top] = keepOnPage(x - size.width / 2, y - size.height / 2, size, pageSize);
    const stamp = this.stamp;
    return this.queued(async () => {
      const a = await this.api.addStamp(this.doc, page, left, top, stamp);
      this.refresh(page);
      this.record("Stamp", [{ page, id: a.id }], "remove");
      this.host.announce(`Stamp added on page ${page + 1}.`);
      this.finishPlacing(page, a.id);
    });
  }

  /** A note or a stamp is placed: the pointer goes back to picking, with the new one picked. */
  private finishPlacing(page: number, id: number): void {
    this.tool = "select";
    this.syncLayer();
    this.selected = { page, id };
    this.view.annotationLayer.setSelected(this.selected);
    this.host.changed();
  }

  /** Puts a note or a stamp in the middle of what is shown (the keyboard's way to place one). */
  placeAtCentre(): void {
    const centre = this.view.centreOfView();
    if (!centre) return;
    if (this.tool === "note") {
      void this.placeNote(centre.page, centre.x, centre.y, { x: window.innerWidth / 2 - 144, y: window.innerHeight / 3 });
    } else if (this.tool === "stamp") {
      void this.placeStamp(centre.page, centre.x, centre.y);
    }
  }

  private drawStroke(page: number, points: [number, number][]): Promise<void> {
    const { color, penWidth } = this;
    return this.queued(async () => {
      try {
        const a = await this.api.addInk(this.doc, page, color, penWidth, [points]);
        this.refresh(page);
        this.record("Drawing", [{ page, id: a.id }], "remove");
        this.host.announce(`Drawing added on page ${page + 1}.`);
        // The line stays until the page shows the drawing, so that it does not blink.
        await this.view.whenRedrawn(page);
      } finally {
        this.view.annotationLayer.endStroke(page);
      }
    });
  }

  // ---- changing what is there ---------------------------------------------------------------

  /** Moves a note by `dx`, `dy` points on the page as it is drawn. */
  moveBy(page: number, id: number, dx: number, dy: number): Promise<void> {
    const a = this.view.annotationLayer.find(page, id);
    if (!a?.can.move || (dx === 0 && dy === 0)) return Promise.resolve();
    return this.queued(async () => {
      await this.api.updateAnnotation(this.doc, page, id, { dx, dy });
      this.refresh(page);
      this.push({
        label: "Move",
        undo: async () => {
          await this.api.updateAnnotation(this.doc, page, id, { dx: -dx, dy: -dy });
          this.refresh(page);
        },
        redo: async () => {
          await this.api.updateAnnotation(this.doc, page, id, { dx, dy });
          this.refresh(page);
        },
      });
    });
  }

  /** Opens the box for the text of an annotation and saves what the person writes. */
  editText(page: number, id: number): Promise<void> {
    const a = this.view.annotationLayer.find(page, id);
    if (a && !a.can.editText) {
      this.host.announce(
        a.can.locked
          ? "This annotation is locked in the file, so it cannot be changed."
          : "The text of this annotation cannot be edited here: it is too long, or the file locks it.",
      );
    }
    if (!a?.can.editText || this.editor) return Promise.resolve();
    const box = this.view.annotationLayer.boxElement(page, id)?.getBoundingClientRect();
    const near = box ? { x: box.left, y: box.bottom } : { x: window.innerWidth / 2 - 144, y: window.innerHeight / 3 };
    const before = a.text;
    const editor = openNoteEditor(this.host.popovers, {
      title: a.kind === "note" ? "Note" : "Comment",
      text: before,
      near,
      onChange: () => this.host.changed(),
    });
    return this.holding(editor, async (text) => {
      if (text === before) return;
      await this.api.updateAnnotation(this.doc, page, id, { text });
      this.refresh(page);
      this.push({
        label: "Edit text",
        undo: async () => {
          await this.api.updateAnnotation(this.doc, page, id, { text: before });
          this.refresh(page);
        },
        redo: async () => {
          await this.api.updateAnnotation(this.doc, page, id, { text });
          this.refresh(page);
        },
      });
      this.host.announce("Text saved.");
    });
  }

  private recolour(page: number, a: Annotation, color: Rgba): Promise<void> {
    const before = a.color;
    if (before[0] === color[0] && before[1] === color[1] && before[2] === color[2]) return Promise.resolve();
    const id = a.id;
    return this.queued(async () => {
      await this.api.updateAnnotation(this.doc, page, id, { color });
      this.refresh(page);
      this.push({
        label: "Change colour",
        undo: async () => {
          await this.api.updateAnnotation(this.doc, page, id, { color: before });
          this.refresh(page);
        },
        redo: async () => {
          await this.api.updateAnnotation(this.doc, page, id, { color });
          this.refresh(page);
        },
      });
    });
  }

  /** Takes the picked annotation off the page. One that cannot be put back asks first. */
  async removeSelected(): Promise<void> {
    const sel = this.selected;
    const a = this.current;
    if (!sel || !a) {
      this.host.announce("Nothing is selected.");
      return;
    }
    if (!a.can.remove) {
      this.host.announce(
        a.can.locked ? "This annotation is locked in the file, so it cannot be removed." : "This annotation cannot be removed.",
      );
      return;
    }
    if (!a.can.restore) {
      const answer = await this.host.ask({
        title: "Delete this annotation?",
        detail:
          "This annotation came with the file, and Undo cannot bring it back. If you delete it and save, it is gone from the file.",
        buttons: [
          { id: "cancel", label: "Cancel", safe: true },
          { id: "delete", label: "Delete", primary: true },
        ],
      });
      if (answer.button !== "delete") return;
    }
    const { page, id } = sel;
    await this.queued(async () => {
      await this.api.removeAnnotation(this.doc, page, id);
      this.refresh(page);
      this.select(page, null);
      this.record(`Delete ${kindName(a.kind).toLowerCase()}`, [{ page, id }], "restore");
      this.host.announce(`${kindName(a.kind)} deleted.`);
    });
  }

  // ---- the keyboard ---------------------------------------------------------------------------

  /** Keys for the annotation that is picked. Returns true when the key was used. */
  handleKey(e: KeyboardEvent): boolean {
    if (e.key === "Escape") {
      if (this.view.annotationLayer.abort()) return true;
      if (this.editor) {
        this.editor.close();
        return true;
      }
      if (this.selected) {
        this.select(this.selected.page, null);
        return true;
      }
      if (this.tool !== "select") {
        this.setTool("select");
        return true;
      }
      return false;
    }
    if ((this.tool === "note" || this.tool === "stamp") && e.key === "Enter") {
      this.placeAtCentre();
      return true;
    }
    const a = this.current;
    if (!a || !this.selected) return false;
    const { page, id } = this.selected;
    switch (e.key) {
      case "Delete":
      case "Backspace":
        void this.removeSelected();
        return true;
      case "Enter":
        void this.editText(page, id);
        return a.can.editText;
      case "ArrowLeft":
      case "ArrowRight":
      case "ArrowUp":
      case "ArrowDown": {
        if (!a.can.move) return false;
        const step = e.shiftKey ? NUDGE_FAR : NUDGE;
        const dx = e.key === "ArrowLeft" ? -step : e.key === "ArrowRight" ? step : 0;
        const dy = e.key === "ArrowUp" ? -step : e.key === "ArrowDown" ? step : 0;
        void this.moveBy(page, id, dx, dy);
        return true;
      }
      default:
        return false;
    }
  }

  /** True when a key press belongs to the box for the text of a note. */
  get editing(): boolean {
    return this.editor !== null;
  }
}

function pagesText(pages: number[]): string {
  const unique = [...new Set(pages)].map((p) => p + 1);
  return unique.length === 1 ? `page ${unique[0]}` : `pages ${unique.join(", ")}`;
}

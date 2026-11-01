// The undo and redo history of one document, and the answer to "does it have changes that are not
// saved?". It knows nothing about PDFs: a feature that changes a document applies the change and
// then pushes an `Edit` that can take the change back and make it again. The history only keeps
// the order, and where the last save was.

export interface Edit {
  /** Short words for a menu or a message, such as "Highlight". */
  label: string;
  /** Takes the change back. May be asynchronous (it may ask the engine). */
  undo(): void | Promise<void>;
  /** Makes the change again. */
  redo(): void | Promise<void>;
}

/** The most edits kept. The oldest are forgotten first. */
export const HISTORY_LIMIT = 200;

export class EditHistory {
  private edits: Edit[] = [];
  /** How many edits are in effect: `edits[0..position]`. The rest can be redone. */
  private pos = 0;
  /**
   * The position the saved file corresponds to. Null when no position of the history matches
   * the saved file any more (the edit that was saved was forgotten, or was undone and then
   * replaced by a new edit): the document then counts as changed whatever the position.
   */
  private saved: number | null = 0;
  private busy = false;
  private readonly listeners = new Set<() => void>();

  constructor(private readonly limit = HISTORY_LIMIT) {}

  /** Number of edits in effect. Changes with every push, undo and redo. */
  get position(): number {
    return this.pos;
  }

  get canUndo(): boolean {
    return !this.busy && this.pos > 0;
  }

  get canRedo(): boolean {
    return !this.busy && this.pos < this.edits.length;
  }

  /** What undo would take back, for a menu: null when there is nothing. */
  get undoLabel(): string | null {
    return this.pos > 0 ? this.edits[this.pos - 1].label : null;
  }

  get redoLabel(): string | null {
    return this.pos < this.edits.length ? this.edits[this.pos].label : null;
  }

  /** True when the document differs from the file that was last saved. */
  get dirty(): boolean {
    return this.saved !== this.pos;
  }

  /** Tells `listener` after every change of the history or of the saved position. Returns a stop function. */
  onChange(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /**
   * Records an edit that has just been made. What could be redone is gone. When the saved file
   * was one of the states that could be redone, nothing matches it any more.
   */
  push(edit: Edit): void {
    // A change made while an undo is running would be lost in the shuffle: that is a mistake of
    // the caller, and is reported as one.
    if (this.busy) throw new Error("an undo or redo is still running");
    this.edits.length = this.pos;
    if (this.saved !== null && this.saved > this.pos) this.saved = null;
    this.edits.push(edit);
    this.pos = this.edits.length;
    if (this.edits.length > this.limit) {
      const excess = this.edits.length - this.limit;
      this.edits.splice(0, excess);
      this.pos -= excess;
      if (this.saved !== null) {
        this.saved -= excess;
        // The state before the oldest edit that is kept cannot be reached any more.
        if (this.saved < 0) this.saved = null;
      }
    }
    this.changed();
  }

  /**
   * Takes back the last edit. False when there is nothing to take back or another undo or redo
   * is still running. When the edit fails to undo, the error is passed on and the history stays
   * where it was.
   */
  async undo(): Promise<boolean> {
    if (!this.canUndo) return false;
    const edit = this.edits[this.pos - 1];
    return this.step(() => edit.undo(), -1);
  }

  async redo(): Promise<boolean> {
    if (!this.canRedo) return false;
    const edit = this.edits[this.pos];
    return this.step(() => edit.redo(), 1);
  }

  /**
   * Says that the document was saved as it was at `position` (what `position` returned when the
   * save began). An edit made while the save was running therefore still counts as unsaved.
   */
  markSaved(position: number = this.pos): void {
    this.saved = position;
    this.changed();
  }

  /** Forgets everything, for a document that was reloaded from the file. The document is clean. */
  reset(): void {
    this.edits = [];
    this.pos = 0;
    this.saved = 0;
    this.changed();
  }

  private async step(run: () => void | Promise<void>, by: 1 | -1): Promise<boolean> {
    this.busy = true;
    try {
      await run();
      this.pos += by;
    } finally {
      this.busy = false;
      this.changed();
    }
    return true;
  }

  private changed(): void {
    for (const listener of [...this.listeners]) listener();
  }
}

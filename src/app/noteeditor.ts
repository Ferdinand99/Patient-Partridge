// A small box for the text of a note, drawn next to the note. The text is what the person types:
// it is read from and written to a text field as plain text, never as markup. Enter adds a line,
// Ctrl+Enter (or Cmd+Enter) saves, Escape leaves the text as it was.

import { MAX_NOTE_BYTES, clampToBytes, utf8Length } from "../lib/annotations";

export interface NoteEditorOptions {
  /** The name of the box for a screen reader, and its heading. */
  title: string;
  /** The text that is there now. */
  text: string;
  /**
   * The text the box counts as unchanged. It is the text the box starts with unless the box comes
   * back with text that was typed before (and refused): then the start is the draft and this is
   * the text the annotation holds.
   */
  baseline?: string;
  /** A place on the screen the box is put next to. */
  near: { x: number; y: number };
  saveLabel?: string;
  /** Called when the text in the box changes. */
  onChange?: () => void;
}

export interface NoteEditor {
  /** The text that was saved, or null when the box was left with Escape or Cancel. */
  result: Promise<string | null>;
  /** Closes the box as if it was cancelled (the document went away, say). */
  close(): void;
  /** Closes the box as if Save was pressed, with the text that is in it now. */
  commit(): void;
  /** True when the text in the box differs from the text the box started with. */
  readonly dirty: boolean;
}

const WIDTH = 288;
const HEIGHT = 196;

export function openNoteEditor(host: HTMLElement, options: NoteEditorOptions): NoteEditor {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  let finish: (value: string | null) => void = () => undefined;
  const result = new Promise<string | null>((resolve) => {
    finish = resolve;
  });

  const form = document.createElement("form");
  form.className = "annot-editor";
  form.setAttribute("role", "dialog");
  form.setAttribute("aria-label", options.title);
  const heading = document.createElement("p");
  heading.className = "annot-editor-title";
  heading.textContent = options.title;
  const area = document.createElement("textarea");
  area.className = "field-input annot-editor-text";
  area.setAttribute("aria-label", "Text of the note");
  area.rows = 5;
  area.spellcheck = true;
  area.value = clampToBytes(options.text, MAX_NOTE_BYTES);
  const hint = document.createElement("span");
  hint.className = "annot-editor-hint muted";
  hint.id = `annot-editor-hint-${Math.random().toString(36).slice(2, 8)}`;
  hint.textContent = "Ctrl+Enter saves";
  area.setAttribute("aria-describedby", hint.id);
  const foot = document.createElement("div");
  foot.className = "annot-editor-foot";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "btn btn-secondary";
  cancel.textContent = "Cancel";
  const save = document.createElement("button");
  save.type = "submit";
  save.className = "btn btn-primary";
  save.textContent = options.saveLabel ?? "Save";
  foot.append(hint, cancel, save);
  form.append(heading, area, foot);

  const close = (value: string | null) => {
    if (!form.isConnected) return;
    form.remove();
    finish(value);
    previous?.focus({ preventScroll: true });
  };
  const started = options.baseline === undefined ? area.value : clampToBytes(options.baseline, MAX_NOTE_BYTES);
  area.addEventListener("input", () => {
    // The limit is in bytes, as the engine counts. A paste that is too long is cut, not refused.
    if (utf8Length(area.value) > MAX_NOTE_BYTES) area.value = clampToBytes(area.value, MAX_NOTE_BYTES);
    options.onChange?.();
  });
  form.addEventListener("submit", (e) => {
    e.preventDefault();
    close(area.value);
  });
  cancel.addEventListener("click", () => close(null));
  form.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close(null);
    } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      e.stopPropagation();
      close(area.value);
    } else if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "s") {
      // Save belongs to the window: it saves the text of this box first, then the document.
    } else {
      // Keys typed here are for the text: the shortcuts of the window must not see them.
      e.stopPropagation();
    }
  });

  // Next to the place, and inside the window.
  const maxLeft = Math.max(8, (window.innerWidth || WIDTH + 16) - WIDTH - 8);
  const maxTop = Math.max(8, (window.innerHeight || HEIGHT + 16) - HEIGHT - 8);
  form.style.left = `${Math.min(Math.max(8, options.near.x + 12), maxLeft)}px`;
  form.style.top = `${Math.min(Math.max(8, options.near.y + 12), maxTop)}px`;
  host.append(form);
  area.focus();
  area.setSelectionRange(area.value.length, area.value.length);

  return {
    result,
    close: () => close(null),
    commit: () => close(area.value),
    get dirty() {
      return form.isConnected && area.value !== started;
    },
  };
}

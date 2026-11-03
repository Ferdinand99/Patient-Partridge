// The annotation bar: a small toolbar with the tools, the colours, and the buttons to edit, delete,
// undo and redo. It holds no state of its own: `render` shows the state it is given, and a press
// goes to the handlers. It works with the keyboard like any toolbar: one stop in the Tab order,
// the arrow keys move along the buttons, Home and End go to the ends.

import {
  PALETTE,
  PEN_WIDTHS,
  STAMPS,
  TOOLS,
  type Rgba,
  type StampName,
  type Tool,
  sameColor,
} from "../lib/annotations";
import { MenuController } from "./menu";

export interface AnnotBarHandlers {
  setTool(tool: Tool): void;
  setColor(color: Rgba): void;
  setPenWidth(width: number): void;
  setStamp(stamp: StampName): void;
  editText(): void;
  remove(): void;
  undo(): void;
  redo(): void;
}

export interface AnnotBarState {
  /** The bar is shown. */
  open: boolean;
  /** There is a document to annotate. */
  enabled: boolean;
  tool: Tool;
  color: Rgba;
  penWidth: number;
  stamp: StampName;
  /** Why the annotations cannot be changed (or the picked one), in a line; null when nothing stands in the way. */
  note: string | null;
  /** The document does not allow annotations: only picking is left. */
  blocked: boolean;
  /** The picked annotation can have its text changed. */
  canEditText: boolean;
  canRemove: boolean;
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
}

function byId<T extends HTMLElement>(root: Document, id: string): T {
  const el = root.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
}

export class AnnotBar {
  private readonly bar: HTMLElement;
  private readonly toggle: HTMLButtonElement;
  private readonly toolButtons = new Map<Tool, HTMLButtonElement>();
  private readonly swatches: HTMLButtonElement[] = [];
  private readonly widths: HTMLButtonElement[] = [];
  private readonly penGroup: HTMLElement;
  private readonly noteLine: HTMLElement;
  private readonly editButton: HTMLButtonElement;
  private readonly deleteButton: HTMLButtonElement;
  private readonly undoButton: HTMLButtonElement;
  private readonly redoButton: HTMLButtonElement;
  private readonly stampMenu: HTMLElement;
  private readonly stamps = new Map<StampName, HTMLButtonElement>();
  private readonly stampMenuControl: MenuController;
  /** The button that takes the Tab stop. */
  private stop: HTMLButtonElement | null = null;

  constructor(
    root: Document,
    private readonly handlers: AnnotBarHandlers & { toggle(open: boolean): void },
  ) {
    this.bar = byId(root, "annotbar");
    this.toggle = byId(root, "annotate-button");
    this.penGroup = byId(root, "pen-widths");
    this.noteLine = byId(root, "annot-note");
    this.editButton = byId(root, "annot-edit");
    this.deleteButton = byId(root, "annot-delete");
    this.undoButton = byId(root, "annot-undo");
    this.redoButton = byId(root, "annot-redo");
    this.stampMenu = byId(root, "stamp-menu");

    for (const info of TOOLS) {
      const button = byId<HTMLButtonElement>(root, `tool-${info.id}`);
      this.toolButtons.set(info.id, button);
      if (info.id === "stamp") continue;
      button.addEventListener("click", () => this.handlers.setTool(info.id));
    }

    const colours = byId(root, "swatches");
    for (const swatch of PALETTE) {
      const b = root.createElement("button");
      b.type = "button";
      b.className = `swatch swatch-${swatch.id}`;
      b.setAttribute("role", "radio");
      b.setAttribute("aria-label", swatch.label);
      b.title = swatch.label;
      b.addEventListener("click", () => this.handlers.setColor(swatch.rgba));
      colours.append(b);
      this.swatches.push(b);
    }

    for (const w of PEN_WIDTHS) {
      const b = root.createElement("button");
      b.type = "button";
      b.className = `width width-${w.id}`;
      b.setAttribute("aria-label", `${w.label} line`);
      b.title = `${w.label} line`;
      b.setAttribute("aria-pressed", "false");
      b.addEventListener("click", () => this.handlers.setPenWidth(w.width));
      const dot = root.createElement("span");
      dot.className = "width-dot";
      dot.setAttribute("aria-hidden", "true");
      b.append(dot);
      this.penGroup.append(b);
      this.widths.push(b);
    }

    // The stamp button opens a menu of the stamps; the choice is the tool.
    const stampButton = this.toolButtons.get("stamp") as HTMLButtonElement;
    stampButton.setAttribute("aria-haspopup", "true");
    stampButton.setAttribute("aria-expanded", "false");
    stampButton.setAttribute("aria-controls", "stamp-menu");
    for (const stamp of STAMPS) {
      const item = root.createElement("button");
      item.type = "button";
      item.className = "menu-item";
      item.setAttribute("role", "menuitemradio");
      item.setAttribute("aria-checked", "false");
      const label = root.createElement("span");
      label.textContent = stamp.label;
      item.append(label);
      item.addEventListener("click", () => {
        this.stampMenuControl.close();
        this.handlers.setStamp(stamp.id);
      });
      this.stampMenu.append(item);
      this.stamps.set(stamp.id, item);
    }
    // The menu opens under the button (the menu of the settings opens at the right).
    stampButton.addEventListener("click", () => {
      this.stampMenu.style.setProperty("--menu-left", `${Math.round(stampButton.getBoundingClientRect().left)}px`);
    });
    this.stampMenuControl = new MenuController(stampButton, this.stampMenu);

    this.editButton.addEventListener("click", () => this.handlers.editText());
    this.deleteButton.addEventListener("click", () => this.handlers.remove());
    this.undoButton.addEventListener("click", () => this.handlers.undo());
    this.redoButton.addEventListener("click", () => this.handlers.redo());
    this.toggle.addEventListener("click", () => this.handlers.toggle(this.bar.hidden !== false));
    this.bar.addEventListener("keydown", (e) => this.onKey(e));
    this.bar.addEventListener("focusin", (e) => {
      if (e.target instanceof HTMLButtonElement && this.buttons().includes(e.target)) this.setStop(e.target);
    });
  }

  /** Opens the menu of stamps (the keyboard shortcut for the stamp tool does not need it). */
  openStampMenu(): void {
    this.stampMenuControl.open();
  }

  /** The buttons that can be reached with the arrow keys, in the order of the bar. */
  private buttons(): HTMLButtonElement[] {
    return [...this.bar.querySelectorAll<HTMLButtonElement>("button")].filter(
      (b) => !b.disabled && !b.hidden && b.closest("[hidden]") === null,
    );
  }

  private setStop(button: HTMLButtonElement | null): void {
    this.stop = button;
    for (const b of this.bar.querySelectorAll<HTMLButtonElement>("button")) {
      b.tabIndex = b === button ? 0 : -1;
    }
  }

  private onKey(e: KeyboardEvent): void {
    const keys = ["ArrowLeft", "ArrowRight", "Home", "End"];
    if (!keys.includes(e.key) || e.ctrlKey || e.metaKey || e.altKey) return;
    const list = this.buttons();
    if (list.length === 0) return;
    const at = list.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? list.length - 1
          : (at + (e.key === "ArrowRight" ? 1 : -1) + list.length) % list.length;
    e.preventDefault();
    e.stopPropagation();
    list[next].focus();
  }

  /** Shows the state. */
  render(state: AnnotBarState): void {
    this.bar.hidden = !state.open || !state.enabled;
    this.toggle.disabled = !state.enabled;
    this.toggle.setAttribute("aria-pressed", String(state.open && state.enabled));
    for (const [tool, button] of this.toolButtons) {
      button.setAttribute("aria-pressed", String(state.tool === tool));
      button.disabled = !state.enabled || (state.blocked && tool !== "select");
    }
    this.swatches.forEach((b, i) => {
      const on = sameColor(PALETTE[i].rgba, state.color);
      b.setAttribute("aria-checked", String(on));
    });
    this.penGroup.hidden = state.tool !== "pen";
    this.widths.forEach((b, i) => b.setAttribute("aria-pressed", String(PEN_WIDTHS[i].width === state.penWidth)));
    for (const [id, item] of this.stamps) item.setAttribute("aria-checked", String(id === state.stamp));
    this.noteLine.textContent = state.note ?? "";
    this.noteLine.hidden = state.note === null;
    this.editButton.disabled = !state.canEditText;
    this.deleteButton.disabled = !state.canRemove;
    this.undoButton.disabled = !state.canUndo;
    this.redoButton.disabled = !state.canRedo;
    this.undoButton.title = state.undoLabel ? `Undo ${state.undoLabel.toLowerCase()} (Ctrl+Z)` : "Undo (Ctrl+Z)";
    this.undoButton.setAttribute("aria-label", state.undoLabel ? `Undo ${state.undoLabel.toLowerCase()}` : "Undo");
    this.redoButton.title = state.redoLabel ? `Redo ${state.redoLabel.toLowerCase()} (Ctrl+Y)` : "Redo (Ctrl+Y)";
    this.redoButton.setAttribute("aria-label", state.redoLabel ? `Redo ${state.redoLabel.toLowerCase()}` : "Redo");
    // One stop in the Tab order: the pressed tool, until the person moves along the bar.
    const stop = this.stop && this.buttons().includes(this.stop) ? this.stop : (this.toolButtons.get(state.tool) ?? null);
    this.setStop(stop && !stop.disabled ? stop : (this.buttons()[0] ?? null));
  }

  /** Puts the keyboard focus on the bar (for the keyboard shortcut that opens it). */
  focusBar(): void {
    (this.stop && !this.stop.disabled ? this.stop : this.buttons()[0])?.focus();
  }
}

// The bar of the form designer: the tools that place a field, the buttons to delete, undo and redo.
// It holds no state of its own: `render` shows the state it is given, and a press goes to the
// handlers. It works with the keyboard like any toolbar: one stop in the Tab order, the arrow keys
// move along the buttons, Home and End go to the ends.

import { KIND_LABELS, type DesignKind } from "../lib/formdesign";
import type { DesignTool } from "../viewer/designlayer";

export interface DesignBarHandlers {
  toggle(open: boolean): void;
  /** `viaKeyboard`: the button was pressed with Enter or Space, so there is no click on a page to come. */
  setTool(tool: DesignTool, viaKeyboard: boolean): void;
  remove(): void;
  undo(): void;
  redo(): void;
}

export interface DesignBarState {
  /** The app process has the designer, so the button shows. */
  available: boolean;
  open: boolean;
  /** There is a document the designer can work on. */
  enabled: boolean;
  tool: DesignTool;
  /** Something is picked, and it can be deleted. */
  canRemove: boolean;
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
  /** A line about the tool or the document, or null. */
  note: string | null;
}

/** The tools of the bar, in order, with the id of the button and the icon. */
export const DESIGN_TOOLS: readonly { tool: DesignTool; id: string; label: string; key: string }[] = [
  { tool: "select", id: "dtool-select", label: "Select", key: "V" },
  { tool: "text", id: "dtool-text", label: "Text", key: "" },
  { tool: "checkbox", id: "dtool-checkbox", label: "Check box", key: "" },
  { tool: "radio", id: "dtool-radio", label: "Radio", key: "" },
  { tool: "combo", id: "dtool-combo", label: "Drop-down", key: "" },
  { tool: "list", id: "dtool-list", label: "List", key: "" },
  { tool: "signature", id: "dtool-signature", label: "Signature", key: "" },
];

export function toolTitle(tool: DesignTool): string {
  return tool === "select" ? "Select, move and resize fields" : `Place a ${KIND_LABELS[tool as DesignKind].toLowerCase()}`;
}

function byId<T extends HTMLElement>(root: Document, id: string): T {
  const el = root.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
}

export class DesignBar {
  private readonly bar: HTMLElement;
  private readonly toggle: HTMLButtonElement;
  private readonly tools = new Map<DesignTool, HTMLButtonElement>();
  private readonly noteLine: HTMLElement;
  private readonly deleteButton: HTMLButtonElement;
  private readonly undoButton: HTMLButtonElement;
  private readonly redoButton: HTMLButtonElement;
  private stop: HTMLButtonElement | null = null;

  constructor(
    root: Document,
    private readonly handlers: DesignBarHandlers,
  ) {
    this.bar = byId(root, "designbar");
    this.toggle = byId(root, "design-button");
    this.noteLine = byId(root, "design-note");
    this.deleteButton = byId(root, "design-delete");
    this.undoButton = byId(root, "design-undo");
    this.redoButton = byId(root, "design-redo");
    for (const info of DESIGN_TOOLS) {
      const button = byId<HTMLButtonElement>(root, info.id);
      button.title = toolTitle(info.tool);
      this.tools.set(info.tool, button);
      // A button pressed with the keyboard places a field of the default size in the middle of the page
      // (there is no pointer to click on the page with); one pressed with the pointer takes the tool.
      let viaKey = false;
      button.addEventListener("keydown", (e) => {
        if (e.key === "Enter" || e.key === " ") viaKey = true;
      });
      button.addEventListener("pointerdown", () => (viaKey = false));
      button.addEventListener("blur", () => (viaKey = false));
      button.addEventListener("click", () => {
        const keyboard = viaKey;
        viaKey = false;
        this.handlers.setTool(info.tool, keyboard);
      });
    }
    this.deleteButton.addEventListener("click", () => this.handlers.remove());
    this.undoButton.addEventListener("click", () => this.handlers.undo());
    this.redoButton.addEventListener("click", () => this.handlers.redo());
    this.toggle.addEventListener("click", () => this.handlers.toggle(Boolean(this.bar.hidden)));
    this.bar.addEventListener("keydown", (e) => this.onKey(e));
    this.bar.addEventListener("focusin", (e) => {
      if (e.target instanceof HTMLButtonElement && this.buttons().includes(e.target)) this.setStop(e.target);
    });
  }

  private buttons(): HTMLButtonElement[] {
    return [...this.bar.querySelectorAll<HTMLButtonElement>("button")].filter(
      (b) => !b.disabled && !b.hidden && b.closest("[hidden]") === null,
    );
  }

  private setStop(button: HTMLButtonElement | null): void {
    this.stop = button;
    for (const b of this.bar.querySelectorAll<HTMLButtonElement>("button")) b.tabIndex = b === button ? 0 : -1;
  }

  private onKey(e: KeyboardEvent): void {
    const keys = ["ArrowLeft", "ArrowRight", "Home", "End"];
    if (!keys.includes(e.key) || e.ctrlKey || e.metaKey || e.altKey) return;
    const list = this.buttons();
    if (list.length === 0) return;
    const at = list.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      e.key === "Home" ? 0 : e.key === "End" ? list.length - 1 : (at + (e.key === "ArrowRight" ? 1 : -1) + list.length) % list.length;
    e.preventDefault();
    e.stopPropagation();
    list[next].focus();
  }

  render(state: DesignBarState): void {
    this.bar.hidden = !state.available || !state.open || !state.enabled;
    this.toggle.hidden = !state.available;
    this.toggle.disabled = !state.enabled;
    this.toggle.setAttribute("aria-pressed", String(state.open && state.enabled));
    for (const [tool, button] of this.tools) {
      button.setAttribute("aria-pressed", String(state.tool === tool));
      button.disabled = !state.enabled;
    }
    this.noteLine.textContent = state.note ?? "";
    this.noteLine.hidden = state.note === null;
    this.deleteButton.disabled = !state.canRemove;
    this.undoButton.disabled = !state.canUndo;
    this.redoButton.disabled = !state.canRedo;
    this.undoButton.title = state.undoLabel ? `Undo ${state.undoLabel.toLowerCase()} (Ctrl+Z)` : "Undo (Ctrl+Z)";
    this.undoButton.setAttribute("aria-label", state.undoLabel ? `Undo ${state.undoLabel.toLowerCase()}` : "Undo");
    this.redoButton.title = state.redoLabel ? `Redo ${state.redoLabel.toLowerCase()} (Ctrl+Y)` : "Redo (Ctrl+Y)";
    this.redoButton.setAttribute("aria-label", state.redoLabel ? `Redo ${state.redoLabel.toLowerCase()}` : "Redo");
    const stop = this.stop && this.buttons().includes(this.stop) ? this.stop : (this.tools.get(state.tool) ?? null);
    this.setStop(stop && !stop.disabled ? stop : (this.buttons()[0] ?? null));
  }

  focusBar(): void {
    (this.stop && !this.stop.disabled ? this.stop : this.buttons()[0])?.focus();
  }
}

// The panel of the form designer: what the picked field is called and holds (name, tooltip, required,
// read only, the longest text, the choices, the default value) and the order the Tab key walks the
// fields of the page in. It holds no state of the document: `render` shows the state it is given, and
// a change goes to the handlers, which answer with what is wrong with it, if anything.
//
// The words of the fields (names, choices) are put in the controls as values and text, never as markup.

import {
  KIND_LABELS,
  describeWidget,
  type DesignKind,
  type DesignProps,
  type DesignWidgetInfo,
} from "../lib/formdesign";

export interface PanelState {
  /** The picked widget, or null when none is picked. */
  widget: DesignWidgetInfo | null;
  /** Every widget of the picked field, in order (the buttons of a radio group). */
  group: DesignWidgetInfo[];
  /** The widgets of the page in the order Tab walks them. */
  order: DesignWidgetInfo[];
  /** The page, counted from 1, for the heading of the order. */
  pageNumber: number;
  /** A line about the document or the picked field that the reader should know, or null. */
  note: string | null;
  /** Another tool is working on the pages: nothing can be changed until it is done. */
  busy?: boolean;
}

export interface PanelHandlers {
  /** The reader changed the field. Returns what is wrong with the change, or null when it was made. */
  update(newName: string, props: DesignProps, exports: string[] | undefined): string | null;
  remove(): void;
  pick(name: string, widget: number): void;
  move(from: number, to: number): void;
}

const FONT_SIZES = [0, 8, 9, 10, 11, 12, 14, 18, 24];

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
}

let counter = 0;

export class DesignPanel {
  private readonly title = el("h2", "design-title", "Field");
  private readonly empty = el("p", "muted design-empty");
  private readonly note = el("p", "design-note muted");
  private readonly form = el("form", "design-form");
  private readonly error = el("p", "design-error");
  private readonly orderTitle = el("h3", "design-subtitle");
  private readonly orderList = el("ol", "design-order");
  private readonly inputs = new Map<string, HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>();
  private readonly groups = new Map<string, HTMLElement>();
  private readonly removeButton = el("button", "btn btn-plain design-remove", "Delete the field");
  private state: PanelState | null = null;
  /** The panel is being drawn: the controls change by themselves, which is not a change by the reader. */
  private rendering = false;

  constructor(
    private readonly root: HTMLElement,
    private readonly handlers: PanelHandlers,
  ) {
    this.root.setAttribute("aria-label", "Field properties");
    this.form.noValidate = true;
    this.form.addEventListener("submit", (e) => e.preventDefault());
    this.removeButton.type = "button";
    this.removeButton.addEventListener("click", () => this.handlers.remove());
    this.error.setAttribute("role", "alert");
    this.error.hidden = true;
    this.empty.textContent = "Pick a field to change what it is, or choose a tool and click on the page to make one.";
    this.note.hidden = true;

    const id = ++counter;
    const row = (key: string, label: string, control: HTMLElement, group?: string, hint?: string): HTMLElement => {
      const wrap = el("div", "design-row");
      const labelEl = el("label", "design-label", label);
      control.id = `dp${id}-${key}`;
      labelEl.htmlFor = control.id;
      wrap.append(labelEl, control);
      if (hint) {
        const h = el("span", "design-hint muted", hint);
        h.id = `dp${id}-${key}-hint`;
        control.setAttribute("aria-describedby", h.id);
        wrap.append(h);
      }
      if (group) this.group(group).append(wrap);
      else this.form.append(wrap);
      return wrap;
    };
    const check = (key: string, label: string, group?: string): void => {
      const input = el("input");
      input.type = "checkbox";
      input.id = `dp${id}-${key}`;
      const wrap = el("div", "design-row design-check");
      const labelEl = el("label", "design-label");
      labelEl.append(input, document.createTextNode(` ${label}`));
      wrap.append(labelEl);
      this.inputs.set(key, input);
      (group ? this.group(group) : this.form).append(wrap);
    };
    const text = (key: string, label: string, group?: string, hint?: string, type = "text"): void => {
      const input = el("input", "field-input");
      input.type = type;
      input.autocomplete = "off";
      input.spellcheck = false;
      this.inputs.set(key, input);
      row(key, label, input, group, hint);
    };

    text("name", "Name", undefined, "Every field needs a name of its own. No dots.");
    text("tooltip", "Tooltip");
    check("required", "Required");
    check("readOnly", "Read only");

    // Text.
    check("multiline", "Several lines", "text");
    check("password", "Hide what is typed (a password)", "text");
    check("comb", "One box for each character", "text");
    text("maxLen", "Longest text (0 for no limit)", "text", undefined, "number");
    const size = el("select", "select");
    for (const s of FONT_SIZES) {
      const o = el("option", undefined, s === 0 ? "Fit the box" : `${s} points`);
      o.value = String(s);
      size.append(o);
    }
    this.inputs.set("fontSize", size);
    row("fontSize", "Size of the text", size, "textual");
    text("defaultText", "Starts with", "text");

    // Check box.
    text("exportValue", "Value when checked", "checkbox");
    check("startsOn", "Starts checked", "checkbox");

    // Radio button.
    text("buttonValue", "Value of this button", "radio", "Each button of a group has a value of its own.");
    check("buttonOn", "This button starts picked", "radio");

    // Choices.
    const choices = el("textarea", "field-input design-choices");
    choices.rows = 5;
    choices.spellcheck = false;
    this.inputs.set("choices", choices);
    row("choices", "Choices", choices, "choice", "One on each line.");
    check("editable", "The reader may type another value", "combo");
    check("multiselect", "The reader may pick several", "list");
    const pick = el("select", "select");
    this.inputs.set("defaultChoice", pick);
    row("defaultChoice", "Starts with", pick, "choice");

    this.form.addEventListener("change", (e) => {
      if (this.rendering) return;
      // The choices change the list of the default: it is rebuilt first, and the choice that was
      // picked stays picked when it is still in the list.
      if (e.target === choices) {
        this.fillDefaultChoice(this.valuesOfChoices(), this.input<HTMLSelectElement>("defaultChoice").value);
      }
      this.send(e.target instanceof HTMLElement ? e.target : null);
    });

    this.root.append(this.title, this.empty, this.note, this.form, this.error, this.removeButton);
    const order = el("section", "design-order-section");
    this.orderTitle.id = `dp${id}-order-title`;
    this.orderList.setAttribute("aria-labelledby", this.orderTitle.id);
    order.append(this.orderTitle, this.orderList);
    this.root.append(order);
    this.render({ widget: null, group: [], order: [], pageNumber: 1, note: null });
  }

  private group(key: string): HTMLElement {
    let g = this.groups.get(key);
    if (!g) {
      g = el("div", `design-group design-group-${key}`);
      this.groups.set(key, g);
      this.form.append(g);
    }
    return g;
  }

  private input<T extends HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>(key: string): T {
    return this.inputs.get(key) as T;
  }

  private valuesOfChoices(): string[] {
    return this.input<HTMLTextAreaElement>("choices")
      .value.split("\n")
      .map((l) => l.replace(/\r$/, ""))
      .filter((l) => l.trim() !== "");
  }

  private fillDefaultChoice(choices: string[], picked: string): void {
    const select = this.input<HTMLSelectElement>("defaultChoice");
    select.replaceChildren();
    const none = el("option", undefined, "Nothing picked");
    none.value = "";
    select.append(none);
    for (const c of choices) {
      const o = el("option");
      o.value = c;
      o.textContent = c;
      select.append(o);
    }
    select.value = choices.includes(picked) ? picked : "";
  }

  /** Reads the controls into the properties of the field, and the new name. */
  private read(w: DesignWidgetInfo): { name: string; props: DesignProps; exports: string[] | undefined } {
    const checked = (key: string) => this.input<HTMLInputElement>(key).checked;
    const value = (key: string) => this.input<HTMLInputElement>(key).value;
    const p: DesignProps = { ...w.props, choices: [...w.props.choices] };
    p.tooltip = value("tooltip");
    p.required = checked("required");
    p.readOnly = checked("readOnly");
    let exports: string[] | undefined;
    switch (w.kind) {
      case "text":
        p.multiline = checked("multiline");
        p.password = checked("password");
        p.comb = checked("comb");
        p.maxLen = Math.max(0, Math.trunc(Number(value("maxLen")) || 0));
        p.fontSize = Number(value("fontSize")) || 0;
        p.defaultValue = value("defaultText");
        break;
      case "checkbox":
        p.exportValue = value("exportValue");
        p.defaultValue = checked("startsOn") ? "on" : "";
        break;
      case "radio": {
        exports = this.state?.group.map((g) => (g.widget === w.widget ? value("buttonValue") : g.export)) ?? [];
        const mine = value("buttonValue");
        const was = this.state?.group.find((g) => g.widget === w.widget)?.export ?? "";
        if (checked("buttonOn")) p.defaultValue = mine;
        else if (w.props.defaultValue === was) p.defaultValue = "";
        else p.defaultValue = w.props.defaultValue;
        // The value that was picked follows a button that was renamed.
        if (!checked("buttonOn") && w.props.defaultValue !== "" && w.props.defaultValue !== was) {
          p.defaultValue = w.props.defaultValue;
        }
        break;
      }
      case "combo":
      case "list": {
        p.choices = this.valuesOfChoices();
        p.fontSize = Number(value("fontSize")) || 0;
        p.editable = w.kind === "combo" && checked("editable");
        p.multiselect = w.kind === "list" && checked("multiselect");
        const picked = value("defaultChoice");
        p.defaultValue = p.choices.includes(picked) ? picked : "";
        break;
      }
      case "signature":
        break;
    }
    return { name: value("name"), props: p, exports };
  }

  private send(changed: HTMLElement | null): void {
    const w = this.state?.widget;
    if (!w || !w.editable) return;
    const { name, props, exports } = this.read(w);
    const problem = this.handlers.update(name, props, exports);
    this.error.textContent = problem ?? "";
    this.error.hidden = problem === null;
    // What was refused is not kept in the control: when the reader leaves it, it shows what the field
    // has, and the words that say why stay until the next change.
    if (problem !== null && changed && (changed instanceof HTMLInputElement || changed instanceof HTMLTextAreaElement || changed instanceof HTMLSelectElement)) {
      changed.addEventListener(
        "blur",
        () => {
          if (this.state) this.render(this.state);
        },
        { once: true },
      );
    }
  }

  /** Shows the state. Controls the reader is in are left alone, so that typing is not interrupted. */
  render(state: PanelState): void {
    this.state = state;
    this.rendering = true;
    try {
      const w = state.widget;
      this.empty.hidden = w !== null;
      this.title.textContent = w ? KIND_LABELS[w.kind] : "Field";
      this.note.textContent = state.note ?? "";
      this.note.hidden = state.note === null;
      this.form.hidden = w === null;
      this.removeButton.hidden = w === null;
      this.removeButton.disabled = !w?.editable || state.busy === true;
      this.removeButton.textContent = w && w.widgets > 1 ? "Delete this button" : "Delete the field";
      if (!w) {
        this.error.hidden = true;
      } else {
        this.fill(w, state);
      }
      this.renderOrder(state);
    } finally {
      this.rendering = false;
    }
  }

  private fill(w: DesignWidgetInfo, state: PanelState): void {
    const set = (key: string, value: string | boolean) => {
      const control = this.inputs.get(key);
      if (!control || document.activeElement === control) return;
      if (control instanceof HTMLInputElement && control.type === "checkbox") control.checked = value === true;
      else control.value = String(value);
    };
    const p = w.props;
    set("name", w.name);
    set("tooltip", p.tooltip);
    set("required", p.required);
    set("readOnly", p.readOnly);
    set("multiline", p.multiline);
    set("password", p.password);
    set("comb", p.comb);
    set("maxLen", String(p.maxLen));
    // A size another program chose is shown as it is (and kept when nothing else about it changes).
    const size = this.input<HTMLSelectElement>("fontSize");
    for (const o of [...size.options]) if (o.dataset.custom === "1" && o.value !== String(p.fontSize)) o.remove();
    if (!FONT_SIZES.includes(p.fontSize) && ![...size.options].some((o) => o.value === String(p.fontSize))) {
      const custom = el("option", undefined, `${p.fontSize} points`);
      custom.value = String(p.fontSize);
      custom.dataset.custom = "1";
      size.append(custom);
    }
    set("fontSize", String(p.fontSize));
    set("defaultText", p.defaultValue);
    set("exportValue", p.exportValue);
    set("startsOn", p.defaultValue === "on");
    set("buttonValue", w.export);
    set("buttonOn", p.defaultValue !== "" && p.defaultValue === w.export);
    set("choices", p.choices.join("\n"));
    set("editable", p.editable);
    set("multiselect", p.multiselect);
    if (document.activeElement !== this.inputs.get("defaultChoice")) this.fillDefaultChoice(p.choices, p.defaultValue);
    const showing: Record<string, boolean> = {
      text: w.kind === "text",
      textual: w.kind === "text" || w.kind === "combo" || w.kind === "list",
      checkbox: w.kind === "checkbox",
      radio: w.kind === "radio",
      choice: w.kind === "combo" || w.kind === "list",
      combo: w.kind === "combo",
      list: w.kind === "list",
    };
    for (const [key, group] of this.groups) group.hidden = !showing[key];
    for (const input of this.inputs.values()) input.disabled = !w.editable || state.busy === true;
    // A comb needs a length; the field for it says so.
    this.input<HTMLInputElement>("maxLen").min = "0";
    void state;
  }

  private renderOrder(state: PanelState): void {
    // The list is made again at every change; the button the reader is on is found again after.
    const active = document.activeElement;
    const was =
      active instanceof HTMLElement && this.orderList.contains(active)
        ? {
            name: active.closest<HTMLElement>(".design-order-item")?.dataset.name ?? "",
            widget: active.closest<HTMLElement>(".design-order-item")?.dataset.widget ?? "0",
            role: active.dataset.role ?? "pick",
          }
        : null;
    this.orderTitle.textContent = `Tab order on page ${state.pageNumber}`;
    this.orderList.replaceChildren();
    state.order.forEach((item, i) => {
      const li = el("li", "design-order-item");
      li.dataset.name = item.name;
      li.dataset.widget = String(item.widget);
      const pick = el("button", "btn btn-plain design-order-pick");
      pick.dataset.role = "pick";
      pick.type = "button";
      pick.textContent = item.widgets > 1 ? `${item.name} (${item.widget + 1})` : item.name;
      pick.setAttribute("aria-label", describeWidget(item));
      if (state.widget && state.widget.name === item.name && state.widget.widget === item.widget) {
        pick.setAttribute("aria-current", "true");
      }
      pick.addEventListener("click", () => this.handlers.pick(item.name, item.widget));
      const up = el("button", "btn btn-icon btn-plain design-order-up");
      up.dataset.role = "up";
      up.type = "button";
      up.textContent = "↑";
      up.setAttribute("aria-label", `Move ${item.name} earlier in the Tab order`);
      up.disabled = i === 0 || !item.editable || state.busy === true;
      up.addEventListener("click", () => this.handlers.move(i, i - 1));
      const down = el("button", "btn btn-icon btn-plain design-order-down");
      down.dataset.role = "down";
      down.type = "button";
      down.textContent = "↓";
      down.setAttribute("aria-label", `Move ${item.name} later in the Tab order`);
      down.disabled = i === state.order.length - 1 || !item.editable || state.busy === true;
      down.addEventListener("click", () => this.handlers.move(i, i + 1));
      li.append(pick, up, down);
      this.orderList.append(li);
    });
    if (state.order.length === 0) {
      const li = el("li", "muted", "No fields on this page yet.");
      this.orderList.append(li);
    }
    if (was) {
      const li = [...this.orderList.querySelectorAll<HTMLElement>(".design-order-item")].find(
        (x) => x.dataset.name === was.name && x.dataset.widget === was.widget,
      );
      const again = li?.querySelector<HTMLButtonElement>(`[data-role="${was.role}"]`);
      // A button that cannot be pressed any more (the field is at the end of the list) gives the
      // focus to the name of the field, which is still there.
      (again && !again.disabled ? again : li?.querySelector<HTMLButtonElement>('[data-role="pick"]'))?.focus();
    }
  }

  /** Puts the keyboard focus on the first control of the panel. */
  focus(): void {
    const first = this.inputs.get("name");
    if (first && !this.form.hidden) first.focus();
  }

  /** Says what is wrong, for an error found outside the controls. */
  showError(message: string | null): void {
    this.error.textContent = message ?? "";
    this.error.hidden = message === null;
  }
}

export type { DesignKind };

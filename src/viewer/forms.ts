// The fields of a form, as controls over the drawn pages.
//
// The engine draws the page, fields included, the way PDFium draws it. On top of every page that has
// fields there is a layer with one control for each field: an input, a text box, a drop-down list, a
// list box, a check box or a radio button. At rest a control is a see-through patch (a faint tint
// so that the reader sees where the fields are); the picture of the field below it shows. When the
// reader uses a control, it covers its field and shows what is typed or picked. A value is sent to
// the engine when the reader leaves the field (or presses Enter), the engine writes it into the
// document the way a PDF reader must, and the page is drawn again. So what the reader sees at rest
// is always what the engine drew, and what is saved is always what the engine wrote.
//
// Boxes are placed with percentages of the page and text is sized from one number, `--form-k` (pixels
// per point), so the layer is right at every zoom without being placed again. The engine gives the
// boxes of the page as it is drawn, so a turned page needs nothing but the text turning with it.
//
// Names, tooltips, values and the labels of choices come from the PDF: they are put in the controls
// as text (`value`, `textContent`, attribute values) and never as markup.

import { isEngineLost, toAppError } from "../lib/errors";
import type { PageSizePt } from "./layout";

export type FormKindName = "none" | "acro" | "xfa";

export interface FormInfo {
  kind: FormKindName;
  /** The form has scripts or actions that are never run. */
  actions: boolean;
}

export type FormFieldKindName = "text" | "checkbox" | "radio" | "combo" | "list";

export interface FormOptionInfo {
  selected: boolean;
  label: string;
}

export interface FormFieldInfo {
  index: number;
  kind: FormFieldKindName;
  readOnly: boolean;
  required: boolean;
  multiline: boolean;
  password: boolean;
  editable: boolean;
  multiselect: boolean;
  checked: boolean;
  valueCut: boolean;
  noToggleOff: boolean;
  comb: boolean;
  action: boolean;
  /** The field has an action that reaches a reset or a hide: it is left alone (and read only). */
  actionBlocked: boolean;
  /** No field of the form can be filled in: the reader cannot tell which fields its actions touch. */
  formBlocked: boolean;
  /** Quarter turns clockwise. */
  turns: number;
  /** Left, top, right and bottom in points from the top left of the page as it is drawn. */
  rect: [number, number, number, number];
  /** Points, or 0 when the field fits the text to its height. */
  fontSize: number;
  name: string;
  tooltip: string;
  value: string;
  maxLen: number;
  options: FormOptionInfo[];
}

export type FormValueIn =
  | { kind: "text"; text: string }
  | { kind: "checked"; on: boolean }
  | { kind: "choice"; picked: number[] };

export interface FormsApi {
  formInfo(doc: number): Promise<FormInfo>;
  getFormFields(doc: number, page: number): Promise<FormFieldInfo[]>;
  setFormField(
    doc: number,
    page: number,
    index: number,
    name: string,
    value: FormValueIn,
  ): Promise<FormFieldInfo[]>;
}

/** A change that can be taken back and made again (the shape of an `Edit` of the edit history). */
export interface FormEdit {
  label: string;
  undo(): void | Promise<void>;
  redo(): void | Promise<void>;
}

export interface FormsEvents {
  /** A field was changed. The receiver keeps the edit for undo and redo and marks the document changed. */
  onEdit(edit: FormEdit): void;
  /** The picture of this page is out of date: draw it again. */
  onRedraw(page: number): void;
  /**
   * Whether the layer holds input that is not in the document yet: text typed and not sent, or a
   * change on its way. Told each time the answer changes.
   */
  onPending?(pending: boolean): void;
  /** The engine of the document was lost while a field was changed: the document is gone. */
  onLost?(error: unknown): void;
}

/** What the form bar says about a form that cannot be filled in. */
export const XFA_NOTICE =
  "This is an XFA form, which Patient Partridge cannot fill in. The pages are shown as they are. Open the file in a PDF reader that supports XFA to fill it in.";

/** What the form bar says about scripts and actions. */
export const ACTIONS_NOTICE =
  "This form has scripts or actions. Patient Partridge does not run scripts, so fields that add up, check or format values will not update. The PDF library can run some other actions itself, so fields that could clear or hide other fields are left as they are.";

/** What the form bar says about fields that are left alone for the action they carry. */
export const BLOCKED_NOTICE =
  "Some fields in this form cannot be filled in here. Each of them may have an action that could clear or hide fields. Patient Partridge leaves these fields as they are, because the PDF library could otherwise run those actions and clear what you have typed. To fill them in, use another PDF reader.";

/** What the form bar says when no field of the form can be filled in. */
export const FORM_BLOCKED_NOTICE =
  "The fields in this form cannot be filled in here, because Patient Partridge cannot be sure that filling them in would not clear or hide other fields. It leaves them as they are. To fill them in, use another PDF reader.";

/** What the form bar says when the form of a document cannot be read. */
export const UNREADABLE_NOTICE = "The form of this document could not be read, so it cannot be filled in here.";

const CANNOT_CLEAR = "A drop-down list cannot be cleared once a choice is made. Pick another choice.";
const CANNOT_UNCHECK = "A radio button cannot be turned off once it is on. Pick another one.";
const CANNOT_CHANGE = "That field could not be changed.";
const TEXT_REFUSED =
  "That text is too long for the document, so it is not saved. It stays in the box. Shorten it or clear it before you save.";
/** What the box of a field says while its text is refused. */
const REFUSED_TITLE =
  "This text is too long for the document, so it is not saved. Shorten it, or press Escape to put back what the document has.";
const CANNOT_UNDO_RADIO = "A radio button cannot be turned off, so this choice cannot be taken back. Pick another one.";
const CANNOT_UNDO_LIST = "A drop-down list cannot be cleared, so this choice cannot be taken back. Pick another one.";

/** The longest `maxLength` a box takes: more makes the browser refuse it. */
const MAX_LENGTH = 0x7fffffff;

/** How long a short message stays in the bar, in milliseconds. */
const MESSAGE_MS = 6000;

/** How long a request to go to a field waits for its page to show it, in milliseconds. */
const WANTED_MS = 5000;

/** How long a field that was changed keeps covering its picture while the page is drawn again. */
const PENDING_MS = 4000;

interface FieldView {
  field: FormFieldInfo;
  /** The page the field is on. */
  page: number;
  el: HTMLElement;
  /** A part that sits next to the control: the suggestions of a drop-down list with a box. */
  extra?: HTMLElement;
  /** What was last sent for a text field, so that Enter and leaving the field do not send twice. */
  lastSent: string | null;
  /** The reader is typing in it: what the engine says does not overwrite the box. */
  editing: boolean;
  pendingTimer?: ReturnType<typeof setTimeout>;
  /** The title the box has when its text is not refused. */
  baseTitle?: string;
}

interface PageState {
  layer: HTMLElement;
  size: PageSizePt;
  list: FormFieldInfo[] | null;
  views: Map<number, FieldView>;
  loading: boolean;
}

function sameShape(a: FormFieldInfo[], b: FormFieldInfo[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i];
    const y = b[i];
    if (
      x.index !== y.index ||
      x.kind !== y.kind ||
      x.name !== y.name ||
      x.readOnly !== y.readOnly ||
      x.multiline !== y.multiline ||
      x.editable !== y.editable ||
      x.multiselect !== y.multiselect ||
      x.turns !== y.turns ||
      x.options.length !== y.options.length
    ) {
      return false;
    }
    for (let k = 0; k < x.options.length; k++) {
      if (x.options[k].label !== y.options[k].label) return false;
    }
  }
  return true;
}

function selectedOf(field: FormFieldInfo): number[] {
  const out: number[] = [];
  field.options.forEach((o, i) => {
    if (o.selected) out.push(i);
  });
  return out;
}

function isEmpty(field: FormFieldInfo): boolean {
  switch (field.kind) {
    case "text":
      return field.value === "";
    case "checkbox":
    case "radio":
      return !field.checked;
    case "combo":
      return field.editable ? field.value === "" : selectedOf(field).length === 0;
    default:
      return selectedOf(field).length === 0;
  }
}

/** The words that name a field for a screen reader: its tooltip, else its name. */
export function fieldLabel(field: FormFieldInfo): string {
  return field.tooltip || field.name || "Form field";
}

function percent(value: number, of: number): string {
  return `${((of > 0 ? value / of : 0) * 100).toFixed(3)}%`;
}

export class FormLayer {
  private readonly pages = new Map<number, PageState>();
  private enabled = false;
  private destroyed = false;
  private readonly told = new Set<string>();
  /** Requests to the engine go one after the other, in the order they were made. */
  private chain: Promise<unknown> = Promise.resolve();
  private bar: HTMLElement | null = null;
  private barText: HTMLElement | null = null;
  private readonly messages: { text: string; timer?: ReturnType<typeof setTimeout> }[] = [];
  private readonly observer: ResizeObserver | null;
  private groupCounter = 0;
  /** Boxes with text that was typed and not sent. */
  private readonly typed = new Set<FieldView>();
  /** Changes that are sent or waiting to be sent. */
  private inFlight = 0;
  private pendingTold = false;
  /**
   * Text the engine did not take, by page and field. It stays in its box, and the document counts
   * as changed, until the reader changes or clears it: it must not vanish at a save, a close or a
   * quit. It is kept by place, not by box, so that it survives a page that is unmounted.
   */
  private readonly refused = new Map<string, { text: string; label: string }>();
  /** A field the reader asked to be taken to, for a page that is not there yet. */
  private wanted: { page: number; index: number } | null = null;
  private wantedTimer?: ReturnType<typeof setTimeout>;

  constructor(
    private readonly doc: number,
    private readonly sizes: PageSizePt[],
    private readonly api: FormsApi,
    /** The scrolling element of the view, where the bar for notices sits. */
    private readonly host: HTMLElement,
    private readonly events: FormsEvents,
  ) {
    this.observer =
      typeof ResizeObserver !== "undefined"
        ? new ResizeObserver((entries) => {
            for (const entry of entries) {
              const layer = entry.target as HTMLElement;
              const page = Number(layer.dataset.page);
              this.measure(layer, page);
            }
          })
        : null;
  }

  /** Asks what kind of form the document has. Call once. */
  start(): void {
    this.api
      .formInfo(this.doc)
      .then((info) => {
        if (this.destroyed) return;
        if (info.kind === "xfa") {
          this.say("xfa", XFA_NOTICE);
        } else if (info.kind === "acro") {
          this.enabled = true;
          if (info.actions) this.say("actions", ACTIONS_NOTICE);
          for (const [page, state] of this.pages) {
            if (!state.list) this.load(page, state);
          }
        }
      })
      .catch(() => {
        // A form that cannot be read is not a document without a form.
        if (!this.destroyed) this.say("unreadable", UNREADABLE_NOTICE);
      });
  }

  /** True when the document has a form the reader can fill in. */
  get fillable(): boolean {
    return this.enabled;
  }

  /** True while text that was typed is not in the document yet, or a change is on its way. */
  get hasPendingInput(): boolean {
    return this.inFlight > 0 || this.typed.size > 0 || this.refused.size > 0;
  }

  /** True while a field holds text that the engine did not take. The document cannot be saved so. */
  get hasRefusedInput(): boolean {
    return this.refused.size > 0;
  }

  /** The fields that hold text the engine did not take, in the order of the document. */
  get refusedFields(): { page: number; index: number; label: string }[] {
    return [...this.refused]
      .map(([key, entry]) => {
        const [page, index] = key.split(":").map(Number);
        return { page, index, label: entry.label };
      })
      .sort((a, b) => a.page - b.page || a.index - b.index);
  }

  /**
   * Scrolls to a field and puts the focus in it. A page that is not on screen is asked for first;
   * the field is reached when its box is there.
   */
  reveal(page: number, index: number): void {
    this.wanted = { page, index };
    clearTimeout(this.wantedTimer);
    this.wantedTimer = setTimeout(() => (this.wanted = null), WANTED_MS);
    this.focusWanted();
  }

  private focusWanted(): void {
    const wanted = this.wanted;
    if (!wanted) return;
    const view = this.pages.get(wanted.page)?.views.get(wanted.index);
    if (!view || !view.el.isConnected) return;
    this.wanted = null;
    clearTimeout(this.wantedTimer);
    view.el.scrollIntoView({ block: "center", inline: "nearest" });
    view.el.focus({ preventScroll: true });
  }

  /** Marks a box whose text the engine did not take, so that it shows the text and says why. */
  private markRefused(view: FieldView, on: boolean): void {
    const el = view.el;
    el.classList.toggle("form-refused", on);
    if (on) {
      if (view.baseTitle === undefined) view.baseTitle = el.title;
      el.setAttribute("aria-invalid", "true");
      el.title = REFUSED_TITLE;
    } else if (view.baseTitle !== undefined) {
      el.removeAttribute("aria-invalid");
      if (view.baseTitle) el.title = view.baseTitle;
      else el.removeAttribute("title");
      view.baseTitle = undefined;
    }
  }

  private refusedKey(view: FieldView): string {
    return `${view.page}:${view.field.index}`;
  }

  /**
   * Sends the text that was typed in a box (the one that has the focus, if the reader did not leave
   * it) and waits until every change has been made. Call it before anything that looks at the
   * document: a save, a close, a quit, an update, a print.
   */
  async flush(): Promise<void> {
    for (const view of [...this.typed]) this.commitText(view.page, view);
    // A change that is on its way may be followed by another that was queued behind it.
    do {
      await this.chain;
    } while (this.inFlight > 0);
  }

  private updatePending(): void {
    const now = this.hasPendingInput;
    if (now === this.pendingTold) return;
    this.pendingTold = now;
    this.events.onPending?.(now);
  }

  // ---- the pages -------------------------------------------------------------------------------

  /** A page got an element: it gets a layer for its fields. */
  mount(page: number, pageEl: HTMLElement): void {
    if (this.destroyed || this.pages.has(page)) return;
    const layer = document.createElement("div");
    layer.className = "form-layer";
    layer.dataset.page = String(page);
    pageEl.append(layer);
    const state: PageState = {
      layer,
      size: this.sizes[page],
      list: null,
      views: new Map(),
      loading: false,
    };
    this.pages.set(page, state);
    this.measure(layer, page);
    this.observer?.observe(layer);
    // The fields are asked for once the picture is there, but a page that is mounted again after
    // the answer came does not have to wait for it.
    if (this.enabled && layer.isConnected) this.load(page, state);
  }

  unmount(page: number): void {
    const state = this.pages.get(page);
    if (!state) return;
    // What was typed in a box of this page is sent before the box goes: a page that scrolls out of
    // the range of the mounted pages must not take a value with it.
    for (const view of [...state.views.values()]) {
      if (this.typed.has(view)) this.commitText(page, view);
    }
    this.observer?.unobserve(state.layer);
    for (const view of state.views.values()) clearTimeout(view.pendingTimer);
    state.layer.remove();
    this.pages.delete(page);
  }

  /** A page has been drawn: the fields that covered their picture can let go of it. */
  pageDrawn(page: number): void {
    const state = this.pages.get(page);
    if (!state) return;
    for (const view of state.views.values()) {
      view.el.classList.remove("form-pending");
      clearTimeout(view.pendingTimer);
    }
    if (this.enabled && !state.list && !state.loading) this.load(page, state);
  }

  destroy(): void {
    this.destroyed = true;
    this.observer?.disconnect();
    for (const page of [...this.pages.keys()]) this.unmount(page);
    for (const message of this.messages) clearTimeout(message.timer);
    this.messages.length = 0;
    this.bar?.remove();
    this.bar = null;
  }

  /** Pixels per point of the page: text in the controls is sized with it. */
  private measure(layer: HTMLElement, page: number): void {
    const size = this.sizes[page];
    const width = layer.clientWidth;
    if (size && size.width > 0 && width > 0) layer.style.setProperty("--form-k", String(width / size.width));
  }

  private load(page: number, state: PageState): void {
    state.loading = true;
    this.api
      .getFormFields(this.doc, page)
      .then((list) => {
        if (this.destroyed || this.pages.get(page) !== state) return;
        state.loading = false;
        state.list = list;
        this.render(page, state);
        if (list.some((f) => f.action)) this.say("actions", ACTIONS_NOTICE);
        if (list.some((f) => f.formBlocked)) this.say("blocked", FORM_BLOCKED_NOTICE);
        else if (list.some((f) => f.actionBlocked)) this.say("blocked", BLOCKED_NOTICE);
      })
      .catch(() => {
        if (this.pages.get(page) === state) {
          state.loading = false;
          // A page that gives no list gets no controls, and is not asked again until it is mounted
          // again.
          state.list = [];
        }
      });
  }

  // ---- notices ---------------------------------------------------------------------------------

  /** Says something once in the bar over the pages. */
  private say(kind: string, text: string): void {
    if (this.told.has(kind)) return;
    this.told.add(kind);
    this.tell(text, false);
  }

  /** Shows a message in the bar. A short message goes away by itself. */
  private tell(text: string, transient = true): void {
    if (this.destroyed) return;
    // The message that is on show is not put up a second time.
    if (this.messages[this.messages.length - 1]?.text === text) return;
    const entry: { text: string; timer?: ReturnType<typeof setTimeout> } = { text };
    if (transient) {
      entry.timer = setTimeout(() => {
        const at = this.messages.indexOf(entry);
        if (at >= 0) this.messages.splice(at, 1);
        this.showBar();
      }, MESSAGE_MS);
    }
    this.messages.push(entry);
    this.showBar();
  }

  private showBar(): void {
    const last = this.messages[this.messages.length - 1];
    if (!last) {
      this.bar?.setAttribute("hidden", "");
      return;
    }
    if (!this.bar || !this.barText) {
      const bar = document.createElement("div");
      bar.className = "form-bar";
      bar.setAttribute("role", "status");
      const pill = document.createElement("div");
      pill.className = "form-bar-pill";
      const text = document.createElement("span");
      text.className = "form-bar-text";
      const close = document.createElement("button");
      close.type = "button";
      close.className = "form-bar-close";
      close.setAttribute("aria-label", "Dismiss this message");
      close.textContent = "×";
      close.addEventListener("click", () => {
        const dropped = this.messages.pop();
        if (dropped) clearTimeout(dropped.timer);
        this.showBar();
      });
      pill.append(text, close);
      bar.append(pill);
      this.host.prepend(bar);
      this.bar = bar;
      this.barText = text;
    }
    this.barText.textContent = last.text;
    this.bar.removeAttribute("hidden");
  }

  // ---- drawing the controls --------------------------------------------------------------------

  private render(page: number, state: PageState, force = false): void {
    const list = state.list ?? [];
    const old = [...state.views.values()].map((v) => v.field);
    if (state.views.size > 0 && sameShape(old, list)) {
      for (const field of list) {
        const view = state.views.get(field.index);
        if (view) this.sync(view, field, force);
      }
      this.updateRadioStops(state);
      this.focusWanted();
      return;
    }
    for (const view of state.views.values()) {
      clearTimeout(view.pendingTimer);
      this.typed.delete(view);
    }
    // Text that was refused stays for a field that is still there, and goes with a field that is not.
    for (const key of [...this.refused.keys()]) {
      const [at, index] = key.split(":").map(Number);
      if (at === page && !list.some((f) => f.index === index)) this.refused.delete(key);
    }
    state.layer.replaceChildren();
    state.views.clear();
    const groups = new Map<string, HTMLElement>();
    const members = new Map<string, FormFieldInfo[]>();
    for (const field of list) {
      if (field.kind === "radio") {
        const group = members.get(field.name) ?? [];
        group.push(field);
        members.set(field.name, group);
      }
    }
    for (const field of list) {
      const view = this.build(page, state, field, members.get(field.name) ?? []);
      state.views.set(field.index, view);
      let parent: HTMLElement = state.layer;
      if (field.kind === "radio") {
        let group = groups.get(field.name);
        if (!group) {
          group = document.createElement("div");
          group.className = "form-group";
          group.setAttribute("role", "radiogroup");
          group.setAttribute("aria-label", fieldLabel(field));
          groups.set(field.name, group);
          state.layer.append(group);
        }
        parent = group;
      }
      parent.append(view.el);
      if (view.extra) parent.append(view.extra);
    }
    this.updateRadioStops(state);
    this.focusWanted();
  }

  /** Puts the box of a field where the engine says, in percentages of the page. */
  private place(el: HTMLElement, field: FormFieldInfo, size: PageSizePt): void {
    const [left, top, right, bottom] = field.rect;
    const w = right - left;
    const h = bottom - top;
    if (field.turns === 1 || field.turns === 3) {
      // The control keeps the shape the field has on the unturned page and is turned about its
      // middle, so its text runs along the field.
      const cx = (left + right) / 2;
      const cy = (top + bottom) / 2;
      el.style.left = percent(cx - h / 2, size.width);
      el.style.top = percent(cy - w / 2, size.height);
      el.style.width = percent(h, size.width);
      el.style.height = percent(w, size.height);
    } else {
      el.style.left = percent(left, size.width);
      el.style.top = percent(top, size.height);
      el.style.width = percent(w, size.width);
      el.style.height = percent(h, size.height);
    }
    el.style.transform = field.turns === 0 ? "" : `rotate(${field.turns * 90}deg)`;
    const across = Math.min(w, h);
    const points =
      field.fontSize > 0
        ? field.fontSize
        : field.kind === "text" && !field.multiline
          ? Math.min(Math.max(across * 0.62, 6), 14)
          : 12;
    el.style.fontSize = `calc(var(--form-k, 1) * ${points.toFixed(2)}px)`;
  }

  private build(page: number, state: PageState, field: FormFieldInfo, group: FormFieldInfo[]): FieldView {
    let el: HTMLElement;
    switch (field.kind) {
      case "text":
        el = this.buildText(field);
        break;
      case "checkbox":
        el = this.buildToggle(field, "checkbox");
        break;
      case "radio":
        el = this.buildToggle(field, "radio");
        break;
      case "combo":
        el = field.editable ? this.buildEditableCombo(field) : this.buildSelect(field, false);
        break;
      default:
        el = this.buildSelect(field, true);
    }
    el.classList.add("form-field", `form-${field.kind}`);
    el.dataset.formIndex = String(field.index);
    const label = fieldLabel(field);
    if (field.kind === "radio") {
      const at = group.findIndex((f) => f.index === field.index);
      el.setAttribute("aria-label", `${label}, choice ${at + 1} of ${group.length}`);
      el.setAttribute("aria-posinset", String(at + 1));
      el.setAttribute("aria-setsize", String(group.length));
    } else {
      el.setAttribute("aria-label", label);
    }
    if (field.tooltip) el.title = field.tooltip;
    if (field.valueCut) {
      el.title = `${field.tooltip ? `${field.tooltip} - ` : ""}The value of this field is too long to change here.`;
    }
    if (field.actionBlocked) {
      const why = field.formBlocked
        ? "No field of this form can be filled in here: Patient Partridge cannot be sure that filling it in would not clear or hide other fields."
        : "This field cannot be filled in here: it may have an action that could clear or hide fields, so Patient Partridge leaves it as it is.";
      el.title = `${field.tooltip ? `${field.tooltip} - ` : ""}${why}`;
    }
    if (field.required) el.setAttribute("aria-required", "true");
    if (field.readOnly || field.valueCut) el.setAttribute("aria-readonly", "true");
    this.place(el, field, state.size);
    const view: FieldView = { field, page, el, lastSent: null, editing: false };
    if (field.kind === "combo" && field.editable) view.extra = this.buildSuggestions(el, field);
    this.attach(page, state, view);
    this.sync(view, field);
    return view;
  }

  private buildText(field: FormFieldInfo): HTMLElement {
    const locked = field.readOnly || field.valueCut;
    if (field.multiline) {
      const area = document.createElement("textarea");
      area.readOnly = locked;
      area.spellcheck = false;
      if (field.maxLen > 0) area.maxLength = Math.min(field.maxLen, MAX_LENGTH);
      return area;
    }
    const input = document.createElement("input");
    input.type = field.password ? "password" : "text";
    input.readOnly = locked;
    input.spellcheck = false;
    input.autocomplete = "off";
    if (field.maxLen > 0) input.maxLength = Math.min(field.maxLen, MAX_LENGTH);
    return input;
  }

  private buildEditableCombo(field: FormFieldInfo): HTMLElement {
    const input = document.createElement("input");
    input.type = "text";
    input.readOnly = field.readOnly || field.valueCut;
    input.spellcheck = false;
    input.autocomplete = "off";
    if (field.maxLen > 0) input.maxLength = Math.min(field.maxLen, MAX_LENGTH);
    return input;
  }

  /** The suggestions of a drop-down list with a box to type in. It sits next to the input. */
  private buildSuggestions(input: HTMLElement, field: FormFieldInfo): HTMLElement {
    const list = document.createElement("datalist");
    list.id = `form-${this.doc}-${++this.groupCounter}`;
    for (const option of field.options) {
      const item = document.createElement("option");
      item.value = option.label;
      list.append(item);
    }
    input.setAttribute("list", list.id);
    return list;
  }

  private buildSelect(field: FormFieldInfo, listBox: boolean): HTMLElement {
    const select = document.createElement("select");
    if (listBox) {
      select.multiple = field.multiselect;
      select.size = Math.max(2, Math.min(field.options.length, 12));
    } else {
      // A drop-down list with nothing chosen shows an empty line first, which cannot be chosen.
      const blank = document.createElement("option");
      blank.value = "-1";
      blank.disabled = true;
      blank.hidden = true;
      select.append(blank);
    }
    field.options.forEach((option, i) => {
      const item = document.createElement("option");
      item.value = String(i);
      item.textContent = option.label;
      select.append(item);
    });
    return select;
  }

  private buildToggle(field: FormFieldInfo, role: "checkbox" | "radio"): HTMLElement {
    const button = document.createElement("button");
    button.type = "button";
    button.setAttribute("role", role);
    button.setAttribute("aria-checked", String(field.checked));
    return button;
  }

  /** Brings a control up to date with the field the engine describes. */
  private sync(view: FieldView, field: FormFieldInfo, force = false): void {
    view.field = field;
    const el = view.el;
    el.classList.toggle("form-readonly", field.readOnly || field.valueCut);
    el.classList.toggle("form-required", field.required);
    el.classList.toggle("form-missing", field.required && isEmpty(field));
    switch (field.kind) {
      case "text":
      case "combo": {
        if (el instanceof HTMLSelectElement) {
          const blank = el.options[0];
          const picked = selectedOf(field);
          el.value = picked.length > 0 ? String(picked[0]) : "-1";
          if (blank) blank.selected = picked.length === 0;
        } else if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
          const kept = this.refused.get(this.refusedKey(view));
          if (kept !== undefined) {
            // The text the engine did not take stays where the reader put it.
            el.value = kept.text;
            view.lastSent = kept.text;
          } else if (force || !view.editing) {
            el.value = field.value;
            view.lastSent = null;
          }
          this.markRefused(view, kept !== undefined);
        }
        break;
      }
      case "list": {
        if (el instanceof HTMLSelectElement) {
          const picked = new Set(selectedOf(field));
          Array.from(el.options).forEach((option, i) => {
            option.selected = picked.has(i);
          });
        }
        break;
      }
      default:
        el.setAttribute("aria-checked", String(field.checked));
        el.classList.toggle("form-on", field.checked);
    }
  }

  /** Only the radio button that is on (or the first, when none is) is a stop for the Tab key. */
  private updateRadioStops(state: PageState): void {
    const groups = new Map<string, FieldView[]>();
    for (const view of state.views.values()) {
      if (view.field.kind !== "radio") continue;
      const name = view.field.name;
      groups.set(name, [...(groups.get(name) ?? []), view]);
    }
    for (const views of groups.values()) {
      const on = views.find((v) => v.field.checked) ?? views[0];
      for (const view of views) view.el.tabIndex = view === on ? 0 : -1;
    }
  }

  // ---- what the reader does --------------------------------------------------------------------

  private attach(page: number, state: PageState, view: FieldView): void {
    const { el, field } = view;
    if (field.actionBlocked) this.explainBlocked(view);
    if (field.kind === "checkbox" || field.kind === "radio") {
      el.addEventListener("click", (e) => {
        e.preventDefault();
        const now = view.field;
        if (now.readOnly) return;
        if (now.kind === "checkbox") {
          void this.commit(page, view, { kind: "checked", on: !now.checked });
        } else if (!now.checked) {
          void this.commit(page, view, { kind: "checked", on: true });
        }
      });
      if (field.kind === "radio") this.attachRadioKeys(state, view);
      return;
    }
    if (el instanceof HTMLSelectElement) {
      el.addEventListener("change", () => {
        const now = view.field;
        if (now.readOnly) {
          this.sync(view, now);
          return;
        }
        const picked = Array.from(el.selectedOptions)
          .map((o) => Number(o.value))
          .filter((n) => n >= 0);
        void this.commit(page, view, { kind: "choice", picked });
      });
      // A list that cannot be changed does not open.
      for (const type of ["mousedown", "keydown"]) {
        el.addEventListener(type, (e) => {
          if (view.field.readOnly && !(e instanceof KeyboardEvent && e.key === "Tab")) e.preventDefault();
        });
      }
      return;
    }
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
      el.addEventListener("focus", () => {
        view.editing = true;
      });
      el.addEventListener("blur", () => {
        view.editing = false;
        // Some web views (WebKit, when the window loses the focus) send a blur and no change, and
        // send the change only when the focus comes back, if at all. Text that was typed is sent
        // now; the change that may follow finds it sent and does nothing.
        if (this.typed.has(view)) {
          this.commitText(page, view);
          return;
        }
        // Nothing was typed: what the engine has is shown again.
        if (view.lastSent === null) {
          el.value = view.field.value;
          this.updatePending();
        }
      });
      el.addEventListener("input", () => {
        view.lastSent = null;
        if (this.refused.delete(this.refusedKey(view))) this.markRefused(view, false);
        const text = el.value.replace(/\r\n?/g, "\n");
        if (text !== view.field.value && !view.field.readOnly && !view.field.valueCut) this.typed.add(view);
        else this.typed.delete(view);
        this.updatePending();
      });
      el.addEventListener("change", () => this.commitText(page, view));
      el.addEventListener("keydown", (event) => {
        const e = event as KeyboardEvent;
        if (e.key === "Escape") {
          el.value = view.field.value;
          view.lastSent = null;
          this.typed.delete(view);
          if (this.refused.delete(this.refusedKey(view))) this.markRefused(view, false);
          this.updatePending();
        } else if (e.key === "Enter" && el instanceof HTMLInputElement) {
          // Enter ends the entry and keeps the focus where it is.
          e.preventDefault();
          this.commitText(page, view);
        }
      });
    }
  }

  /**
   * Says why a field that is left alone cannot be filled in, when the reader tries to use it, so
   * that it does not look like a fault. The words are those the bar gave when the page was shown.
   */
  private explainBlocked(view: FieldView): void {
    const notice = view.field.formBlocked ? FORM_BLOCKED_NOTICE : BLOCKED_NOTICE;
    const say = () => this.tell(notice);
    view.el.addEventListener("mousedown", say);
    view.el.addEventListener("click", say);
    view.el.addEventListener("focus", say);
    view.el.addEventListener("keydown", (e) => {
      if ((e as KeyboardEvent).key !== "Tab") say();
    });
  }

  private attachRadioKeys(state: PageState, view: FieldView): void {
    view.el.addEventListener("keydown", (e) => {
      const keys = ["ArrowRight", "ArrowDown", "ArrowLeft", "ArrowUp"];
      if (!keys.includes(e.key)) return;
      e.preventDefault();
      const group = [...state.views.values()].filter(
        (v) => v.field.kind === "radio" && v.field.name === view.field.name,
      );
      const at = group.indexOf(view);
      const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : -1;
      const next = group[(at + step + group.length) % group.length];
      if (!next || next === view) return;
      next.el.focus();
      next.el.click();
    });
  }

  private commitText(page: number, view: FieldView): void {
    const el = view.el as HTMLInputElement | HTMLTextAreaElement;
    this.typed.delete(view);
    if (view.field.readOnly || view.field.valueCut) {
      el.value = view.field.value;
      this.updatePending();
      return;
    }
    const text = el.value.replace(/\r\n?/g, "\n");
    if (text === view.field.value || text === view.lastSent) {
      this.updatePending();
      return;
    }
    view.lastSent = text;
    void this.commit(page, view, { kind: "text", text });
  }

  /** Sends a value, then shows what the engine made of it and records the change for undo. */
  private commit(page: number, view: FieldView, value: FormValueIn): Promise<void> {
    const before = view.field;
    const state = this.pages.get(page);
    const previous = state?.list ?? [];
    this.inFlight += 1;
    this.updatePending();
    return this.enqueue(async () => {
      view.el.classList.add("form-pending");
      clearTimeout(view.pendingTimer);
      view.pendingTimer = setTimeout(() => view.el.classList.remove("form-pending"), PENDING_MS);
      try {
        const list = await this.api.setFormField(this.doc, page, before.index, before.name, value);
        this.refused.delete(`${page}:${before.index}`);
        this.accept(page, list);
        const after = list.find((f) => f.index === before.index && f.name === before.name) ?? before;
        // A box the reader is still in shows what the engine kept, which can be less than was typed.
        const box = view.el;
        if (value.kind === "text" && (box instanceof HTMLInputElement || box instanceof HTMLTextAreaElement)) {
          const typed = box.value.replace(/\r\n?/g, "\n");
          if (typed === value.text && after.value !== value.text) box.value = after.value;
        }
        this.events.onEdit(this.makeEdit(page, before, after, previous));
        this.events.onRedraw(page);
        await this.refreshGroup(page, before);
      } catch (error) {
        view.el.classList.remove("form-pending");
        if (isEngineLost(toAppError(error))) {
          // The document is gone with its engine: the window ends the tab.
          this.events.onLost?.(error);
          return;
        }
        // Text that is too long for the document is not given up: it stays in its box, and the
        // document counts as changed, so that nothing is saved or closed over it unseen.
        const refusedText =
          value.kind === "text" && toAppError(error).code === "invalid_request" && this.stillInBox(view, value.text);
        if (refusedText) {
          this.refused.set(`${page}:${before.index}`, { text: value.text, label: fieldLabel(before) });
          this.markRefused(view, true);
        }
        await this.resync(page);
        this.tell(refusedText ? TEXT_REFUSED : this.explain(before, error));
      } finally {
        this.inFlight -= 1;
        this.updatePending();
      }
    });
  }

  /** True when the box still holds the text that was sent (or is gone with its page). */
  private stillInBox(view: FieldView, text: string): boolean {
    const el = view.el;
    if (!el.isConnected || !(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) return true;
    return el.value.replace(/\r\n?/g, "\n") === text;
  }

  private enqueue<T>(job: () => Promise<T>): Promise<T> {
    const run = this.chain.then(job, job);
    this.chain = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  /** The answer of the engine: the fields of the page as they are now. */
  private accept(page: number, list: FormFieldInfo[], force = false): void {
    const state = this.pages.get(page);
    if (!state) return;
    state.list = list;
    this.render(page, state, force);
  }

  /** Asks again what the page has, after something that did not work. */
  private async resync(page: number): Promise<void> {
    const state = this.pages.get(page);
    if (!state) return;
    try {
      this.accept(page, await this.api.getFormFields(this.doc, page), true);
    } catch {
      // The page keeps what it shows.
    }
  }

  /**
   * A field can have widgets on more than one page (a radio button turns off the others of its
   * group, and a text that is set shows in every widget of the field): the other pages that show
   * the field are asked again and drawn again.
   */
  private async refreshGroup(page: number, field: FormFieldInfo): Promise<void> {
    for (const [other, state] of this.pages) {
      if (other === page || !state.list?.some((f) => f.name === field.name && f.kind === field.kind)) continue;
      await this.resync(other);
      this.events.onRedraw(other);
    }
  }

  private explain(field: FormFieldInfo, error: unknown): string {
    const code = typeof error === "object" && error !== null ? (error as { code?: unknown }).code : undefined;
    if (code === "form_field_refused") {
      if (field.kind === "combo") return CANNOT_CLEAR;
      if (field.kind === "radio") return CANNOT_UNCHECK;
    }
    return CANNOT_CHANGE;
  }

  // ---- undo and redo ---------------------------------------------------------------------------

  private makeEdit(page: number, before: FormFieldInfo, after: FormFieldInfo, previous: FormFieldInfo[]): FormEdit {
    interface Step {
      index: number;
      name: string;
      value: FormValueIn;
    }
    const at = (field: FormFieldInfo, value: FormValueIn): Step => ({ index: field.index, name: field.name, value });
    let back: Step;
    let forward: Step;
    let cannotBack: string | null = null;
    switch (before.kind) {
      case "text":
        back = at(before, { kind: "text", text: before.value });
        forward = at(after, { kind: "text", text: after.value });
        break;
      case "checkbox":
        back = at(before, { kind: "checked", on: before.checked });
        forward = at(after, { kind: "checked", on: after.checked });
        break;
      case "radio": {
        forward = at(after, { kind: "checked", on: true });
        const was = previous.find((f) => f.kind === "radio" && f.name === before.name && f.checked);
        if (was && was.index !== before.index) {
          back = at(was, { kind: "checked", on: true });
        } else {
          // Nothing was chosen before: PDFium has no way to turn a radio button off.
          back = at(before, { kind: "checked", on: false });
          cannotBack = CANNOT_UNDO_RADIO;
        }
        break;
      }
      default:
        if (before.kind === "combo" && before.editable) {
          back = at(before, { kind: "text", text: before.value });
          forward = at(after, { kind: "text", text: after.value });
          if (before.value === "" && before.options.length > 0) cannotBack = CANNOT_UNDO_LIST;
        } else {
          back = at(before, { kind: "choice", picked: selectedOf(before) });
          forward = at(after, { kind: "choice", picked: selectedOf(after) });
          if (before.kind === "combo" && selectedOf(before).length === 0) cannotBack = CANNOT_UNDO_LIST;
        }
    }
    const run = (step: Step): Promise<void> =>
      this.enqueue(async () => {
        const list = await this.api.setFormField(this.doc, page, step.index, step.name, step.value);
        this.accept(page, list, true);
        this.events.onRedraw(page);
        await this.refreshGroup(page, before);
      });
    const label = `Fill in "${fieldLabel(before).slice(0, 60)}"`;
    return {
      label,
      undo: async () => {
        if (cannotBack) {
          this.tell(cannotBack);
          // The engine is not asked, so nothing changed: a clean refusal, as the engine's own.
          throw Object.assign(new Error(cannotBack), { code: "form_field_refused" });
        }
        await run(back);
      },
      redo: () => run(forward),
    };
  }
}

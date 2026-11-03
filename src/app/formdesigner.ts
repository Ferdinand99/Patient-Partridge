// The form designer: the "Edit form" mode. It turns the clicks and drags the layer on the pages
// reports, and the changes the panel reports, into changes of the document's form fields, and keeps
// what the window shows of the fields in step with the engine.
//
// The engine makes the fields (see crates/engine/src/author). The window keeps a model of the
// widgets of the pages it has shown, changes the model at once so that the page answers the pointer,
// and sends the changes to the engine in batches: after a short rest, and before anything looks at
// the document (a save, a print, a page tool, a step back). A batch is one reload of the document and
// one step in the history, so the usual Undo and Redo take it back and bring it again. When the
// engine refuses a batch, the reader is told why and the model is read again from the engine.
//
// Names, tooltips and choices came from a file or from the reader: they are used as text only.

import { toAppError } from "../lib/errors";
import {
  KIND_LABELS,
  applyLocal,
  cloneModel,
  freeName,
  moveEntry,
  namesIn,
  nameProblem,
  propsProblem,
  textBoxes,
  textLines,
  type DesignKind,
  type DesignModel,
  type DesignOp,
  type DesignProps,
  type DesignWidgetInfo,
  type Rect,
  type SnapLines,
  noLines,
} from "../lib/formdesign";
import { describePageError } from "../lib/pageerrors";
import type { PageToolsApi } from "../lib/pagesapi";
import type { PageSizePt } from "../viewer/layout";
import { DesignLayer, type DesignTool, type WidgetKey, sameKey } from "../viewer/designlayer";
import type { DocumentView } from "../viewer/viewer";
import { DesignPanel } from "./designpanel";
import type { EditHistory } from "./history";
import type { NoticeOptions, NoticeResult } from "./notice";
import type { PageOps } from "./pageops";

/** The document in front, as far as the designer cares. */
export interface DesignTarget {
  doc: number;
  name: string;
  view: DocumentView;
  history: EditHistory;
  pages: PageSizePt[];
}

export interface DesignHost {
  target(): DesignTarget | null;
  api: PageToolsApi;
  ops: PageOps;
  pageText(doc: number, page: number): Promise<{ text: string; rects: number[] }>;
  notice(options: NoticeOptions): Promise<NoticeResult>;
  announce(text: string): void;
  /** The bar and the button have to be drawn again. */
  changed(): void;
  /** Changes waiting to be sent appeared or were sent: the document counts as changed or not. */
  pendingChanged(doc: number): void;
}

/** How long the changes rest before they are sent, in milliseconds. */
export const BATCH_REST_MS = 450;

interface Pending {
  ops: DesignOp[];
  label: string;
}

interface Session {
  doc: number;
  view: DocumentView;
  model: DesignModel;
  /** The names the engine said the document has, at the last look. */
  names: Set<string>;
  /** Names that pending changes took away (removed or renamed) and the engine still has. */
  gone: Set<string>;
  /** The pages whose widgets were read from the engine since the document last changed. */
  fresh: Set<number>;
  selected: { page: number; key: WidgetKey } | null;
  pending: Pending | null;
  timer: ReturnType<typeof setTimeout> | undefined;
  /** The changes are being sent. */
  applying: boolean;
  flushing: Promise<boolean> | null;
  lines: Map<number, Promise<SnapLines>>;
}

function el<K extends keyof HTMLElementTagNameMap>(root: Document, id: string): HTMLElementTagNameMap[K] {
  const e = root.getElementById(id);
  if (!e) throw new Error(`missing element #${id}`);
  return e as HTMLElementTagNameMap[K];
}

export class FormDesigner {
  private readonly sessions = new Map<number, Session>();
  private readonly panel: DesignPanel;
  private readonly panelRoot: HTMLElement;
  private toolNow: DesignTool = "select";
  private isOpen = false;
  /** The group the next click of the radio tool adds a button to. */
  private radioGroup: string | null = null;
  private hintText = "";
  private working = false;

  constructor(
    root: Document,
    private readonly host: DesignHost,
  ) {
    this.panelRoot = el<"aside">(root, "design-panel");
    this.panel = new DesignPanel(this.panelRoot, {
      update: (newName, props, exports) => this.updateSelected(newName, props, exports),
      remove: () => this.removeSelected(),
      pick: (name, widget) => this.pick(name, widget),
      move: (from, to) => this.moveInOrder(from, to),
    });
  }

  // ---- state -------------------------------------------------------------------------------

  get open(): boolean {
    return this.isOpen;
  }

  get tool(): DesignTool {
    return this.toolNow;
  }

  get hint(): string {
    return this.hintText;
  }

  /** True when the picked widget can be deleted. */
  get canRemove(): boolean {
    const w = this.pickedWidget();
    return this.isOpen && !this.working && w !== null && w.editable;
  }

  /** The designer has changes it has not sent yet (or is sending them). */
  hasPending(doc: number): boolean {
    const s = this.sessions.get(doc);
    return s !== undefined && (s.pending !== null || s.applying);
  }

  private session(): Session | null {
    const t = this.host.target();
    return t ? (this.sessions.get(t.doc) ?? null) : null;
  }

  private layerOf(s: Session): DesignLayer | null {
    return s.view.designLayer;
  }

  private widgetAt(s: Session, key: WidgetKey): { page: number; item: DesignWidgetInfo } | null {
    for (const [page, list] of s.model) {
      const item = list.find((w) => w.name === key.name && w.widget === key.widget);
      if (item) return { page, item };
    }
    return null;
  }

  private pickedWidget(): DesignWidgetInfo | null {
    const s = this.session();
    if (!s || !s.selected) return null;
    return this.widgetAt(s, s.selected.key)?.item ?? null;
  }

  // ---- opening and closing -----------------------------------------------------------------

  /**
   * Opens the designer on the document in front. Asks the engine whether the document can take
   * fields; when it cannot, the reader is told why and the mode stays closed. Resolves with whether
   * the mode is open.
   */
  async openMode(): Promise<boolean> {
    const tab = this.host.target();
    const check = this.host.api.designCheck;
    if (!tab || !check || !this.host.ops.canDesign) return false;
    if (this.sessions.has(tab.doc)) {
      this.attach(tab, this.sessions.get(tab.doc) as Session);
      this.isOpen = true;
      this.host.changed();
      return true;
    }
    let names: string[];
    try {
      names = await check.call(this.host.api, tab.doc);
    } catch (e) {
      const text = describePageError(toAppError(e), tab.name);
      await this.host.notice({ title: text.title, detail: text.detail, buttons: [{ id: "ok", label: "OK", primary: true }] });
      return false;
    }
    // The reader may have gone to another document, or closed this one, while the engine worked.
    if (this.host.target()?.doc !== tab.doc) return false;
    const session: Session = {
      doc: tab.doc,
      view: tab.view,
      model: new Map(),
      names: new Set(names),
      gone: new Set(),
      fresh: new Set(),
      selected: null,
      pending: null,
      timer: undefined,
      applying: false,
      flushing: null,
      lines: new Map(),
    };
    this.sessions.set(tab.doc, session);
    this.attach(tab, session);
    this.isOpen = true;
    this.host.changed();
    return true;
  }

  private attach(tab: DesignTarget, s: Session): void {
    s.view = tab.view;
    tab.view.setDesign(
      (pages) => this.makeLayer(s, pages),
      { pending: () => this.hasPending(s.doc), flush: async () => void (await this.flush(s.doc)) },
    );
    this.layerOf(s)?.setTool(this.toolNow);
    this.refreshPanel();
  }

  /** Closes the designer: what is not sent is sent first. */
  async closeMode(): Promise<void> {
    const s = this.session();
    if (s) {
      await this.flush(s.doc);
      s.view.setDesign(null);
    }
    this.isOpen = false;
    this.toolNow = "select";
    this.radioGroup = null;
    this.hintText = "";
    this.refreshPanel();
    this.host.changed();
  }

  /** The tab in front changed (or was closed): the designer follows. Called by the app. */
  async followTab(): Promise<void> {
    for (const s of this.sessions.values()) {
      if (this.host.target()?.doc !== s.doc && s.view.designLayer) {
        await this.flush(s.doc);
        s.view.setDesign(null);
      }
    }
    if (!this.isOpen) return;
    const tab = this.host.target();
    if (!tab) {
      this.refreshPanel();
      return;
    }
    const known = this.sessions.get(tab.doc);
    if (known) {
      if (!tab.view.designLayer) this.attach(tab, known);
      this.host.changed();
      return;
    }
    const opened = await this.openMode();
    if (!opened) this.isOpen = false;
    this.host.changed();
  }

  /** A document was closed. */
  forget(doc: number): void {
    const s = this.sessions.get(doc);
    if (s) clearTimeout(s.timer);
    this.sessions.delete(doc);
    this.refreshPanel();
  }

  /** The tool of the bar. */
  setTool(tool: DesignTool): void {
    this.toolNow = tool;
    if (tool !== "radio") this.radioGroup = null;
    const s = this.session();
    if (s) this.layerOf(s)?.setTool(tool);
    if (tool !== "select") this.host.announce(`${tool === "radio" ? "Radio button" : KIND_LABELS[tool]} tool. Click on the page, or drag to size the box.`);
    this.host.changed();
  }

  /** The pages or the document changed under the designer (a step back, a page tool): read the fields again. */
  pagesChanged(doc: number): void {
    const s = this.sessions.get(doc);
    if (!s) return;
    s.fresh.clear();
    s.lines.clear();
    void this.refreshNames(s);
  }

  private async refreshNames(s: Session): Promise<void> {
    const check = this.host.api.designCheck;
    if (!check) return;
    try {
      s.names = new Set(await check.call(this.host.api, s.doc));
      s.gone.clear();
    } catch {
      // The names are asked for again by the next change.
    }
  }

  /** The tools wait while a change to the document is made. */
  setBusy(busy: boolean): void {
    this.working = busy;
    this.host.changed();
  }

  // ---- the layer ---------------------------------------------------------------------------

  private makeLayer(s: Session, pages: PageSizePt[]): DesignLayer {
    return new DesignLayer(pages, {
      pageReady: (page) => void this.load(s, page),
      place: (page, rect) => this.place(s, page, rect),
      select: (page, key) => this.select(s, page, key),
      transform: (page, key, rect) => this.transform(s, page, key, rect),
      remove: (page, key) => this.removeKey(s, page, key),
      edit: () => this.panel.focus(),
      textLines: (page) => this.textLinesOf(s, page),
      hint: (text) => {
        this.hintText = text;
        this.host.changed();
      },
    });
  }

  private textLinesOf(s: Session, page: number): Promise<SnapLines> {
    let known = s.lines.get(page);
    if (!known) {
      known = this.host
        .pageText(s.doc, page)
        .then(({ text, rects }) => {
          const boxes: Rect[] = [];
          for (let i = 0; i + 3 < rects.length; i += 4) boxes.push([rects[i], rects[i + 1], rects[i + 2], rects[i + 3]]);
          const { words, lines } = textBoxes(text, boxes);
          return textLines(words, lines);
        })
        .catch(() => noLines());
      s.lines.set(page, known);
    }
    return known;
  }

  /** Shows the widgets of a page, reading them from the engine when they are not fresh. */
  private async load(s: Session, page: number, force = false): Promise<void> {
    const list = this.host.api.designList;
    const layer = this.layerOf(s);
    if (!list || !layer) return;
    // What is known shows at once; the engine's word replaces it when it comes.
    if (s.model.has(page)) layer.show(page, s.model.get(page) as DesignWidgetInfo[]);
    if (s.fresh.has(page) && !force) {
      this.refreshPanel();
      return;
    }
    try {
      const items = await list.call(this.host.api, s.doc, page);
      // A change that is not sent yet is not in the engine's list: it is shown from the model.
      if (s.pending) return;
      s.model.set(page, items);
      s.fresh.add(page);
      this.layerOf(s)?.show(page, items);
      this.reconcileSelection(s);
      this.refreshPanel();
    } catch (e) {
      const error = toAppError(e);
      if (error.code === "author_unsupported" || error.code === "author_refused") {
        const tab = this.host.target();
        const text = describePageError(error, tab?.name ?? "");
        await this.host.notice({ title: text.title, detail: text.detail, buttons: [{ id: "ok", label: "OK", primary: true }] });
      }
    }
  }

  /** Drops the pick of a widget that is not there any more. */
  private reconcileSelection(s: Session): void {
    if (!s.selected) return;
    if (!this.widgetAt(s, s.selected.key)) {
      s.selected = null;
      this.layerOf(s)?.select(null, null);
    }
  }

  private showAll(s: Session): void {
    const layer = this.layerOf(s);
    if (!layer) return;
    for (const page of layer.mountedPages) layer.show(page, s.model.get(page) ?? []);
  }

  // ---- events from the layer ---------------------------------------------------------------

  private select(s: Session, page: number, key: WidgetKey | null): void {
    s.selected = key ? { page, key } : null;
    this.refreshPanel();
    this.host.changed();
  }

  private pick(name: string, widget: number): void {
    const s = this.session();
    if (!s) return;
    const found = this.widgetAt(s, { name, widget });
    if (!found) return;
    s.selected = { page: found.page, key: { name, widget } };
    this.layerOf(s)?.select(found.page, { name, widget });
    this.layerOf(s)?.focusPicked();
    this.refreshPanel();
    this.host.changed();
  }

  private taken(s: Session): Set<string> {
    const all = new Set(s.names);
    for (const g of s.gone) all.delete(g);
    for (const n of namesIn(s.model)) all.add(n);
    return all;
  }

  private place(s: Session, page: number, rect: Rect): void {
    if (this.working) return;
    const kind = this.toolNow;
    if (kind === "select") return;
    const taken = this.taken(s);
    let op: DesignOp;
    let key: WidgetKey;
    let label: string;
    const group = this.radioGroup;
    if (kind === "radio" && group && taken.has(group) && namesIn(s.model).has(group)) {
      const mine = [...s.model.values()].flat().filter((w) => w.name === group);
      const used = new Set(mine.map((w) => w.export));
      let n = mine.length + 1;
      while (used.has(`Choice${n}`)) n++;
      op = { op: "addWidget", name: group, widget: { page, rect, export: `Choice${n}` } };
      key = { name: group, widget: mine.length };
      label = "Add a radio button";
    } else {
      const name = freeName(kind, taken);
      const props: Partial<DesignProps> = {};
      if (kind === "combo" || kind === "list") props.choices = ["Option 1", "Option 2", "Option 3"];
      op = {
        op: "add",
        field: { name, kind, props, widgets: [{ page, rect, export: kind === "radio" ? "Choice1" : "" }] },
      };
      key = { name, widget: 0 };
      if (kind === "radio") this.radioGroup = name;
      label = `Add a ${KIND_LABELS[kind].toLowerCase()}`;
    }
    this.enqueue(s, [op], label);
    s.selected = { page, key };
    this.layerOf(s)?.select(page, key);
    if (kind !== "radio") this.setTool("select");
    this.layerOf(s)?.focusPicked();
    this.host.announce(`${label.replace(/^Add a/, "Added a")}. Its name is ${key.name}.`);
  }

  private transform(s: Session, page: number, key: WidgetKey, rect: Rect): void {
    if (this.working) return;
    this.enqueue(s, [{ op: "move", name: key.name, widget: key.widget, page, rect }], "Move or resize a field");
  }

  private removeKey(s: Session, page: number, key: WidgetKey): void {
    void page;
    if (this.working) return;
    const found = this.widgetAt(s, key);
    if (!found || !found.item.editable) return;
    const op: DesignOp =
      found.item.widgets > 1
        ? { op: "removeWidget", name: key.name, widget: key.widget }
        : { op: "remove", name: key.name };
    s.gone.add(key.name);
    s.selected = null;
    this.layerOf(s)?.select(null, null);
    this.enqueue(s, [op], found.item.widgets > 1 ? "Delete a radio button" : "Delete a field");
    this.host.announce(`Deleted ${key.name}.`);
  }

  private removeSelected(): void {
    const s = this.session();
    if (!s?.selected) return;
    this.removeKey(s, s.selected.page, s.selected.key);
  }

  /** The Delete button of the bar. */
  removeSelectedWidget(): void {
    this.removeSelected();
  }

  // ---- events from the panel ---------------------------------------------------------------

  private updateSelected(newName: string, props: DesignProps, exports: string[] | undefined): string | null {
    const s = this.session();
    if (!s?.selected || this.working) return null;
    const found = this.widgetAt(s, s.selected.key);
    if (!found || !found.item.editable) return null;
    const w = found.item;
    const problem =
      nameProblem(newName, this.taken(s), w.name) ??
      propsProblem(w.kind, props) ??
      this.exportsProblem(w.kind, exports);
    if (problem) return problem;
    const sameProps = JSON.stringify(props) === JSON.stringify(w.props);
    const sameExports = exports === undefined || exports.every((e, i) => e === (this.group(s, w.name)[i]?.export ?? ""));
    if (newName === w.name && sameProps && sameExports) return null;
    if (newName !== w.name) {
      s.gone.add(w.name);
      s.selected = { page: s.selected.page, key: { name: newName, widget: w.widget } };
      this.layerOf(s)?.select(s.selected.page, s.selected.key);
    }
    this.enqueue(
      s,
      [{ op: "update", name: w.name, newName: newName === w.name ? "" : newName, props, exports }],
      newName === w.name ? "Change a field" : "Rename a field",
    );
    return null;
  }

  private exportsProblem(kind: DesignKind, exports: string[] | undefined): string | null {
    if (kind !== "radio" || !exports) return null;
    for (const [i, e] of exports.entries()) {
      if (e.trim() === "") return "Give each radio button a value.";
      if (e === "Off") return "“Off” means a button that is not picked. Use another word.";
      if (exports.indexOf(e) !== i) return `Two radio buttons have the value “${e}”. Give each its own.`;
    }
    return null;
  }

  private group(s: Session, name: string): DesignWidgetInfo[] {
    return [...s.model.values()]
      .flat()
      .filter((w) => w.name === name)
      .sort((a, b) => a.widget - b.widget);
  }

  private moveInOrder(from: number, to: number): void {
    const s = this.session();
    if (!s || this.working) return;
    const page = s.selected?.page ?? this.host.target()?.view.currentPageIndex ?? 0;
    const list = s.model.get(page) ?? [];
    const next = moveEntry(list, from, to);
    if (next.every((w, i) => w === list[i])) return;
    this.enqueue(
      s,
      [{ op: "reorder", page, order: next.map((w) => ({ name: w.name, widget: w.widget })) }],
      "Change the Tab order",
    );
  }

  // ---- keys --------------------------------------------------------------------------------

  /** Escape: the tool goes back to picking, then the pick goes. Resolves with whether the key was used. */
  handleEscape(): boolean {
    if (!this.isOpen) return false;
    if (this.toolNow !== "select") {
      this.setTool("select");
      return true;
    }
    const s = this.session();
    if (s?.selected) {
      s.selected = null;
      this.layerOf(s)?.select(null, null);
      this.refreshPanel();
      this.host.changed();
      return true;
    }
    return false;
  }

  // ---- the batch ---------------------------------------------------------------------------

  private enqueue(s: Session, ops: DesignOp[], label: string): void {
    for (const op of ops) {
      applyLocal(s.model, op);
      // A move that follows a move of the same widget replaces it (a run of arrow keys is one change).
      const last = s.pending?.ops[s.pending.ops.length - 1];
      if (op.op === "move" && last?.op === "move" && last.name === op.name && last.widget === op.widget && last.page === op.page) {
        s.pending!.ops[s.pending!.ops.length - 1] = op;
      } else if (op.op === "update" && last?.op === "update" && last.name === op.name) {
        s.pending!.ops[s.pending!.ops.length - 1] = { ...op, name: last.name, newName: op.newName !== "" ? op.newName : last.newName };
      } else if (s.pending) {
        s.pending.ops.push(op);
        if (s.pending.label !== label) s.pending.label = "Edit the form";
      } else {
        s.pending = { ops: [op], label };
      }
    }
    this.showAll(s);
    this.refreshPanel();
    this.host.changed();
    this.host.pendingChanged(s.doc);
    clearTimeout(s.timer);
    s.timer = setTimeout(() => void this.flush(s.doc), BATCH_REST_MS);
  }

  /**
   * Sends what is waiting. Resolves with true when nothing is left that the engine did not make.
   * Called by the rest timer and by everything that looks at the document.
   */
  flush(doc: number): Promise<boolean> {
    const s = this.sessions.get(doc);
    if (!s) return Promise.resolve(true);
    // The engine's change calls this very function (through the settling before a page tool): the
    // change that is under way is the one that was asked for.
    if (s.applying) return Promise.resolve(true);
    const run = async (): Promise<boolean> => {
      if (s.flushing) await s.flushing.catch(() => false);
      clearTimeout(s.timer);
      const batch = s.pending;
      if (!batch) return true;
      s.pending = null;
      s.applying = true;
      let ok: boolean;
      try {
        ok = await this.host.ops.applyDesign(batch.ops, batch.label);
      } finally {
        s.applying = false;
      }
      s.gone.clear();
      this.host.pendingChanged(s.doc);
      if (!ok) {
        // The reader was told why. What the window showed is not what the document holds.
        s.fresh.clear();
        const layer = this.layerOf(s);
        for (const page of layer?.mountedPages ?? []) await this.load(s, page, true);
        s.selected = s.selected && this.widgetAt(s, s.selected.key) ? s.selected : null;
        this.refreshPanel();
      } else {
        await this.refreshNames(s);
      }
      this.host.changed();
      return ok;
    };
    const flushing = run();
    s.flushing = flushing;
    void flushing.finally(() => {
      if (s.flushing === flushing) s.flushing = null;
    });
    return flushing;
  }

  // ---- the panel ---------------------------------------------------------------------------

  private refreshPanel(): void {
    const s = this.session();
    this.panelRoot.hidden = !this.isOpen || !s;
    if (!this.isOpen || !s) return;
    const w = this.pickedWidget();
    const page = s.selected?.page ?? this.host.target()?.view.currentPageIndex ?? 0;
    this.panel.render({
      widget: w,
      group: w ? this.group(s, w.name) : [],
      order: s.model.get(page) ?? [],
      pageNumber: page + 1,
      note: w && !w.editable ? "This field was made with another program and has an action or a structure that cannot be changed here." : null,
    });
  }

  /** For the tests: the model of the document in front. */
  modelFor(doc: number): DesignModel | null {
    const s = this.sessions.get(doc);
    return s ? cloneModel(s.model) : null;
  }

  /** True when the key is the picked widget. */
  isPicked(key: WidgetKey): boolean {
    const s = this.session();
    return s?.selected ? sameKey(s.selected.key, key) : false;
  }
}

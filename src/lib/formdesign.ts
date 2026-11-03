// What the window knows about making form fields: the kinds, the properties, the changes it can ask
// for, and the small pure functions behind placing, moving, resizing and snapping a box. Nothing here
// touches the page, the engine or the document object model.
//
// Places are in points from the top left of the page as it is drawn, like the boxes of the form
// fields. The engine turns them into the page's own coordinates; the window never does.

export type DesignKind = "text" | "checkbox" | "radio" | "combo" | "list" | "signature";

/** `[left, top, right, bottom]`. */
export type Rect = [number, number, number, number];

export interface DesignProps {
  tooltip: string;
  required: boolean;
  readOnly: boolean;
  multiline: boolean;
  password: boolean;
  comb: boolean;
  /** 0 for no limit. */
  maxLen: number;
  editable: boolean;
  multiselect: boolean;
  /** 0 fits the text to the height of the box. */
  fontSize: number;
  /**
   * Text: the starting text. Choices: the picked labels, one on each line. Check box: "on" or "".
   * Radio: the value of the button that starts on.
   */
  defaultValue: string;
  choices: string[];
  /** Check box: the word it stands for when on. */
  exportValue: string;
}

export function emptyProps(): DesignProps {
  return {
    tooltip: "",
    required: false,
    readOnly: false,
    multiline: false,
    password: false,
    comb: false,
    maxLen: 0,
    editable: false,
    multiselect: false,
    fontSize: 0,
    defaultValue: "",
    choices: [],
    exportValue: "",
  };
}

/** A widget of a field as the engine lists it. */
export interface DesignWidgetInfo {
  name: string;
  kind: DesignKind;
  /** The place among the widgets of the field. */
  widget: number;
  /** How many widgets the field has. */
  widgets: number;
  rect: Rect;
  /** A radio button: the value it stands for. */
  export: string;
  /** The designer can change the field. */
  editable: boolean;
  props: DesignProps;
}

export interface DesignWidgetIn {
  page: number;
  rect: Rect;
  export?: string;
}

export interface DesignFieldIn {
  name: string;
  kind: DesignKind;
  props: Partial<DesignProps>;
  widgets: DesignWidgetIn[];
}

export interface WidgetRefIn {
  name: string;
  widget: number;
}

/** One change, as the app process takes it. */
export type DesignOp =
  | { op: "add"; field: DesignFieldIn }
  | { op: "remove"; name: string }
  | { op: "removeWidget"; name: string; widget: number }
  | { op: "addWidget"; name: string; widget: DesignWidgetIn }
  | { op: "move"; name: string; widget: number; page: number; rect: Rect }
  | { op: "update"; name: string; newName: string; props: DesignProps; exports?: string[] }
  | { op: "reorder"; page: number; order: WidgetRefIn[] };

export const KIND_LABELS: Record<DesignKind, string> = {
  text: "Text field",
  checkbox: "Check box",
  radio: "Radio button",
  combo: "Drop-down list",
  list: "List box",
  signature: "Signature field",
};

/** The size of a new box in points, when the tool is clicked and not dragged. */
export const DEFAULT_SIZES: Record<DesignKind, [number, number]> = {
  text: [160, 22],
  checkbox: [14, 14],
  radio: [14, 14],
  combo: [160, 22],
  list: [160, 80],
  signature: [200, 50],
};

/** The smallest side of a box in points (the engine holds the same). */
export const MIN_SIDE = 4;

/** The most widgets a field has (the engine holds the same). */
export const MAX_WIDGETS = 64;

const NAME_BASES: Record<DesignKind, string> = {
  text: "Text",
  checkbox: "Check box",
  radio: "Group",
  combo: "Drop-down",
  list: "List",
  signature: "Signature",
};

/** A name that nothing in `taken` has: the kind of the field and the first number that is free. */
export function freeName(kind: DesignKind, taken: ReadonlySet<string>): string {
  const base = NAME_BASES[kind];
  for (let n = 1; ; n++) {
    const name = `${base} ${n}`;
    if (!taken.has(name)) return name;
  }
}

/** Controls, and the characters that change the direction of the text around them or hide. */
function unsafeChar(c: number): boolean {
  return c < 0x20 || (c >= 0x7f && c <= 0x9f) || (c >= 0x200b && c <= 0x200f) || (c >= 0x2028 && c <= 0x202e) || (c >= 0x2066 && c <= 0x2069) || c === 0xfeff;
}

/** What is wrong with a name, in words for the reader, or null. `taken` holds every name in use. */
export function nameProblem(name: string, taken: ReadonlySet<string>, current?: string): string | null {
  if (name.trim() === "") return "Give the field a name.";
  if (name.includes(".")) {
    return "A field name cannot contain a dot, because a dot makes the field a part of another field. Use a dash or an underscore instead.";
  }
  if (name !== name.trim()) return "A field name cannot start or end with a space.";
  if (new TextEncoder().encode(name).length > 255) return "The field name is too long. Keep it under 255 bytes.";
  if ([...name].some((ch) => unsafeChar(ch.codePointAt(0) ?? 0))) {
    return "The field name has a character that cannot be used.";
  }
  if (name !== current && taken.has(name)) {
    return `There is already a field called “${name}”. Every field needs a name of its own.`;
  }
  return null;
}

/** True for a character the standard font of a field can draw (Latin-1, which has æ, ø and å). */
export function drawable(ch: string, newline: boolean): boolean {
  const c = ch.codePointAt(0) ?? 0;
  return (c >= 0x20 && c <= 0x7e) || (c >= 0xa0 && c <= 0xff) || (newline && c === 0x0a);
}

/** What is wrong with a text that is drawn in a field, in words for the reader, or null. */
export function textProblem(what: string, text: string, newline = false): string | null {
  for (const ch of text) {
    if (!drawable(ch, newline)) {
      const shown = ch.codePointAt(0)! < 0x20 ? `U+${ch.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")}` : `“${ch}”`;
      return `${what} has a character that cannot be drawn in a field yet (${shown}). Use letters from the Latin alphabet, such as æ, ø and å.`;
    }
  }
  return null;
}

/** The first thing wrong with a set of properties for a kind, or null. */
export function propsProblem(kind: DesignKind, p: DesignProps): string | null {
  if (kind === "text") {
    if (p.comb && (p.maxLen < 1 || p.maxLen > 200)) {
      return "A field with a box for each character needs a longest text between 1 and 200.";
    }
    if (p.comb && (p.multiline || p.password)) {
      return "A field with a box for each character cannot also be a password or have several lines.";
    }
    if (p.password && p.multiline) return "A password field has one line.";
    if (p.maxLen > 0 && [...p.defaultValue].length > p.maxLen) {
      return "The default value is longer than the longest text the field takes.";
    }
    return textProblem("The default value", p.defaultValue, p.multiline);
  }
  if (kind === "combo" || kind === "list") {
    if (p.choices.length === 0) return "Give the list at least one choice.";
    for (const [i, c] of p.choices.entries()) {
      if (c.trim() === "") return "A choice cannot be empty.";
      const bad = textProblem("A choice", c);
      if (bad) return bad;
      if (p.choices.indexOf(c) !== i) return `The choice “${c}” is there twice.`;
    }
    return null;
  }
  return null;
}

// ---- boxes -------------------------------------------------------------------------------------

export type Handle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";

export const HANDLES: readonly Handle[] = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];

export const HANDLE_LABELS: Record<Handle, string> = {
  nw: "top left corner",
  n: "top edge",
  ne: "top right corner",
  e: "right edge",
  se: "bottom right corner",
  s: "bottom edge",
  sw: "bottom left corner",
  w: "left edge",
};

export interface PageBox {
  width: number;
  height: number;
}

export function width(r: Rect): number {
  return r[2] - r[0];
}

export function height(r: Rect): number {
  return r[3] - r[1];
}

export function normalize(a: [number, number], b: [number, number]): Rect {
  return [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(Math.max(v, lo), hi);
}

/** The box moved so that it lies on the page, with its size kept. A box bigger than the page is cut to it. */
export function keepOnPage(r: Rect, page: PageBox): Rect {
  const w = Math.min(width(r), page.width);
  const h = Math.min(height(r), page.height);
  const left = clamp(r[0], 0, page.width - w);
  const top = clamp(r[1], 0, page.height - h);
  return [left, top, left + w, top + h];
}

/** A box for a new field: a click gives the default size around the point, a drag gives its own. */
export function placeBox(kind: DesignKind, from: [number, number], to: [number, number], page: PageBox): Rect {
  const dragged = Math.hypot(to[0] - from[0], to[1] - from[1]) > 6;
  if (!dragged) {
    const [w, h] = DEFAULT_SIZES[kind];
    return keepOnPage([from[0] - w / 2, from[1] - h / 2, from[0] + w / 2, from[1] + h / 2], page);
  }
  const r = normalize(from, to);
  // A drag that stays thin still makes a box that can be used.
  const w = Math.max(width(r), MIN_SIDE);
  const h = Math.max(height(r), MIN_SIDE);
  return keepOnPage([r[0], r[1], r[0] + w, r[1] + h], page);
}

/** A box moved by (`dx`, `dy`), kept on the page. */
export function moveBox(r: Rect, dx: number, dy: number, page: PageBox): Rect {
  return keepOnPage([r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy], page);
}

/** A box with one handle moved by (`dx`, `dy`): no smaller than `MIN_SIDE`, kept on the page. */
export function resizeBox(r: Rect, handle: Handle, dx: number, dy: number, page: PageBox): Rect {
  let [l, t, rt, b] = r;
  if (handle.includes("w")) l = clamp(l + dx, 0, rt - MIN_SIDE);
  if (handle.includes("e")) rt = clamp(rt + dx, l + MIN_SIDE, page.width);
  if (handle.includes("n")) t = clamp(t + dy, 0, b - MIN_SIDE);
  if (handle.includes("s")) b = clamp(b + dy, t + MIN_SIDE, page.height);
  return [l, t, rt, b];
}

// ---- snapping ----------------------------------------------------------------------------------

export type SnapSource = "page" | "field" | "text";

export interface SnapLine {
  value: number;
  source: SnapSource;
}

export interface SnapLines {
  xs: SnapLine[];
  ys: SnapLine[];
}

/** How near a line must be for the box to jump to it, in screen pixels. */
export const SNAP_PIXELS = 6;

export function noLines(): SnapLines {
  return { xs: [], ys: [] };
}

/** The edges and the middle of the page. */
export function pageLines(page: PageBox): SnapLines {
  return {
    xs: [0, page.width / 2, page.width].map((value) => ({ value, source: "page" as const })),
    ys: [0, page.height / 2, page.height].map((value) => ({ value, source: "page" as const })),
  };
}

/** The edges and the middle of other boxes. */
export function fieldLines(rects: readonly Rect[]): SnapLines {
  const out = noLines();
  for (const r of rects) {
    for (const value of [r[0], (r[0] + r[2]) / 2, r[2]]) out.xs.push({ value, source: "field" });
    for (const value of [r[1], (r[1] + r[3]) / 2, r[3]]) out.ys.push({ value, source: "field" });
  }
  return out;
}

/**
 * The boxes of the words of a page and of its lines, from the text layer: one box for every character
 * is joined into words (broken at a space or a gap) and the words into lines (broken when the next
 * character is not on the same level or is back to the left).
 */
export function textBoxes(text: string, rects: readonly Rect[]): { words: Rect[]; lines: Rect[] } {
  const words: Rect[] = [];
  const lines: Rect[] = [];
  let word: Rect | null = null;
  let line: Rect | null = null;
  const flushWord = () => {
    if (word) words.push(word);
    word = null;
  };
  const flushLine = () => {
    flushWord();
    if (line) lines.push(line);
    line = null;
  };
  const chars = [...text];
  for (let i = 0; i < chars.length && i < rects.length; i++) {
    const r = rects[i];
    if (/\s/.test(chars[i]) || r[2] <= r[0] || r[3] <= r[1]) {
      flushWord();
      continue;
    }
    const mid = (r[1] + r[3]) / 2;
    if (line && (mid < line[1] || mid > line[3] || r[0] < line[2] - (r[3] - r[1]) * 2)) flushLine();
    if (word && r[0] > word[2] + (r[3] - r[1]) * 0.6) flushWord();
    word = word ? [Math.min(word[0], r[0]), Math.min(word[1], r[1]), Math.max(word[2], r[2]), Math.max(word[3], r[3])] : [...r];
    line = line ? [Math.min(line[0], r[0]), Math.min(line[1], r[1]), Math.max(line[2], r[2]), Math.max(line[3], r[3])] : [...r];
  }
  flushLine();
  return { words, lines };
}

/** The lines the text of a page gives: the ends of its words across, the tops and bottoms of its lines down. */
export function textLines(words: readonly Rect[], lines: readonly Rect[]): SnapLines {
  const out = noLines();
  for (const w of words) {
    out.xs.push({ value: w[0], source: "text" }, { value: w[2], source: "text" });
  }
  for (const l of lines) {
    out.ys.push({ value: l[1], source: "text" }, { value: l[3], source: "text" });
  }
  return out;
}

export function joinLines(...all: SnapLines[]): SnapLines {
  return { xs: all.flatMap((a) => a.xs), ys: all.flatMap((a) => a.ys) };
}

/** What a snap did: the box, and the lines it caught on (to draw while dragging). */
export interface Snapped {
  rect: Rect;
  guides: { axis: "x" | "y"; value: number; source: SnapSource }[];
}

/** The nearest line to any of `values` within `limit`, as the shift that puts the value on it. */
function nearest(values: readonly number[], lines: readonly SnapLine[], limit: number): { shift: number; line: SnapLine } | null {
  let best: { shift: number; line: SnapLine } | null = null;
  for (const v of values) {
    for (const line of lines) {
      const shift = line.value - v;
      if (Math.abs(shift) <= limit && (best === null || Math.abs(shift) < Math.abs(best.shift))) best = { shift, line };
    }
  }
  return best;
}

/** A box being moved: its edges and middle catch on the lines. `limit` is in points. */
export function snapMove(r: Rect, lines: SnapLines, limit: number): Snapped {
  const guides: Snapped["guides"] = [];
  const x = nearest([r[0], (r[0] + r[2]) / 2, r[2]], lines.xs, limit);
  const y = nearest([r[1], (r[1] + r[3]) / 2, r[3]], lines.ys, limit);
  const dx = x?.shift ?? 0;
  const dy = y?.shift ?? 0;
  if (x) guides.push({ axis: "x", value: x.line.value, source: x.line.source });
  if (y) guides.push({ axis: "y", value: y.line.value, source: y.line.source });
  return { rect: [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy], guides };
}

/** A box being resized by a handle: only the edges that move catch on the lines. */
export function snapResize(r: Rect, handle: Handle, lines: SnapLines, limit: number): Snapped {
  const out: Rect = [...r];
  const guides: Snapped["guides"] = [];
  const catchEdge = (index: 0 | 1 | 2 | 3, set: SnapLine[]) => {
    const hit = nearest([out[index]], set, limit);
    if (hit) {
      out[index] = out[index] + hit.shift;
      guides.push({ axis: index % 2 === 0 ? "x" : "y", value: hit.line.value, source: hit.line.source });
    }
  };
  if (handle.includes("w")) catchEdge(0, lines.xs);
  if (handle.includes("e")) catchEdge(2, lines.xs);
  if (handle.includes("n")) catchEdge(1, lines.ys);
  if (handle.includes("s")) catchEdge(3, lines.ys);
  return { rect: out, guides };
}

/** Whether a box has the width or the height of one of the others: a hint to show. */
export function sameSizeHints(r: Rect, others: readonly Rect[]): ("width" | "height")[] {
  const near = (a: number, b: number) => Math.abs(a - b) < 0.5;
  const hints: ("width" | "height")[] = [];
  if (others.some((o) => near(width(o), width(r)))) hints.push("width");
  if (others.some((o) => near(height(o), height(r)))) hints.push("height");
  return hints;
}

// ---- the model ---------------------------------------------------------------------------------

/** The widgets of the pages the window has asked for, by page. */
export type DesignModel = Map<number, DesignWidgetInfo[]>;

export function cloneModel(model: DesignModel): DesignModel {
  return new Map([...model].map(([page, list]) => [page, list.map((w) => ({ ...w, props: { ...w.props, choices: [...w.props.choices] } }))]));
}

function withProps(base: Partial<DesignProps>): DesignProps {
  return { ...emptyProps(), ...base, choices: [...(base.choices ?? [])] };
}

/**
 * Makes a change to the model the way the engine will make it to the document, so that the window
 * shows the result at once. The engine's answer is the truth: when it refuses, or when the pages are
 * asked for again, they replace what this made.
 */
export function applyLocal(model: DesignModel, op: DesignOp): void {
  const all = () => [...model.values()].flat();
  const reindex = (name: string) => {
    const mine = all().filter((w) => w.name === name);
    mine.forEach((w, i) => {
      w.widget = i;
      w.widgets = mine.length;
    });
  };
  switch (op.op) {
    case "add": {
      const n = op.field.widgets.length;
      op.field.widgets.forEach((w, i) => {
        const list = model.get(w.page) ?? [];
        list.push({
          name: op.field.name,
          kind: op.field.kind,
          widget: i,
          widgets: n,
          rect: [...w.rect],
          export: w.export ?? "",
          editable: true,
          props: withProps(op.field.props),
        });
        model.set(w.page, list);
      });
      break;
    }
    case "remove":
      for (const [page, list] of model) model.set(page, list.filter((w) => w.name !== op.name));
      break;
    case "removeWidget": {
      for (const [page, list] of model) model.set(page, list.filter((w) => !(w.name === op.name && w.widget === op.widget)));
      reindex(op.name);
      break;
    }
    case "addWidget": {
      const template = all().find((w) => w.name === op.name);
      if (!template) break;
      const list = model.get(op.widget.page) ?? [];
      list.push({ ...template, rect: [...op.widget.rect], export: op.widget.export ?? "", props: { ...template.props } });
      model.set(op.widget.page, list);
      reindex(op.name);
      break;
    }
    case "move": {
      let found: DesignWidgetInfo | undefined;
      for (const [page, list] of model) {
        const at = list.findIndex((w) => w.name === op.name && w.widget === op.widget);
        if (at >= 0) {
          found = list[at];
          if (page !== op.page) model.set(page, list.filter((_, i) => i !== at));
          break;
        }
      }
      if (found) {
        found.rect = [...op.rect];
        const list = model.get(op.page) ?? [];
        if (!list.includes(found)) list.push(found);
        model.set(op.page, list);
      }
      break;
    }
    case "update": {
      const mine = all().filter((w) => w.name === op.name);
      mine.forEach((w, i) => {
        w.props = { ...op.props, choices: [...op.props.choices] };
        if (op.exports && op.exports.length === mine.length) w.export = op.exports[i];
        if (op.newName !== "") w.name = op.newName;
      });
      break;
    }
    case "reorder": {
      const list = model.get(op.page);
      if (!list) break;
      const wanted = op.order.map((r) => list.find((w) => w.name === r.name && w.widget === r.widget));
      if (wanted.some((w) => w === undefined)) break;
      const slots = wanted.map((w) => list.indexOf(w!)).sort((a, b) => a - b);
      const next = [...list];
      slots.forEach((slot, k) => (next[slot] = wanted[k]!));
      model.set(op.page, next);
      break;
    }
  }
}

/** Every name in use in the model (the names the engine reported at the start are added by the caller). */
export function namesIn(model: DesignModel): Set<string> {
  return new Set(all(model).map((w) => w.name));
}

function all(model: DesignModel): DesignWidgetInfo[] {
  return [...model.values()].flat();
}

/** A short line for a screen reader: what a box is and where. */
export function describeWidget(w: DesignWidgetInfo): string {
  const kind = KIND_LABELS[w.kind];
  const part = w.widgets > 1 ? `, button ${w.widget + 1} of ${w.widgets}` : "";
  return `${w.name}, ${kind.toLowerCase()}${part}${w.editable ? "" : ", cannot be changed here"}`;
}

/** Moves one entry of a list to another place (for the tab order). */
export function moveEntry<T>(list: readonly T[], from: number, to: number): T[] {
  if (from < 0 || from >= list.length || to < 0 || to >= list.length) return [...list];
  const next = [...list];
  const [item] = next.splice(from, 1);
  next.splice(to, 0, item);
  return next;
}

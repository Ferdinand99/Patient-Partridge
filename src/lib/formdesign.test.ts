import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  DEFAULT_SIZES,
  BOX_MIN,
  MIN_SIDE,
  applyLocal,
  describeBox,
  describeWidget,
  emptyProps,
  exportProblem,
  exportsProblem,
  fieldLines,
  freeName,
  joinLines,
  keepOnPage,
  moveBox,
  moveEntry,
  nameProblem,
  namesIn,
  noLines,
  pageLines,
  placeBox,
  propsProblem,
  resizeBox,
  roundBox,
  sameSizeHints,
  snapMove,
  snapResize,
  textBoxes,
  textLines,
  textProblem,
  tooltipProblem,
  type DesignModel,
  type DesignProps,
  type DesignWidgetInfo,
  type Rect,
} from "./formdesign";

const PAGE = { width: 612, height: 792 };

function widget(name: string, rect: Rect, extra: Partial<DesignWidgetInfo> = {}): DesignWidgetInfo {
  return { name, kind: "text", widget: 0, widgets: 1, rect, export: "", editable: true, props: emptyProps(), ...extra };
}

describe("names", () => {
  const taken = new Set(["a", "Name"]);

  it("accepts a name that is free, in any script", () => {
    expect(nameProblem("first_name", taken)).toBeNull();
    expect(nameProblem("Navn æøå 日本", taken)).toBeNull();
    // Case counts: two names that differ in case are two names.
    expect(nameProblem("name", taken)).toBeNull();
  });

  it("refuses an empty name, a dot, a space at the ends, a name that is taken and a control character", () => {
    expect(nameProblem("", taken)).toMatch(/Give the field a name/);
    expect(nameProblem("   ", taken)).toMatch(/Give the field a name/);
    expect(nameProblem("a.b", taken)).toMatch(/dot/);
    expect(nameProblem(" a1", taken)).toMatch(/space/);
    expect(nameProblem("a1 ", taken)).toMatch(/space/);
    expect(nameProblem("a", taken)).toMatch(/already a field called/);
    expect(nameProblem("x\u0000y", taken)).toMatch(/cannot be used/);
    expect(nameProblem("x‮y", taken)).toMatch(/cannot be used/);
    expect(nameProblem("x".repeat(256), taken)).toMatch(/too long/);
    expect(nameProblem("x".repeat(255), taken)).toBeNull();
  });

  it("lets a field keep its own name", () => {
    expect(nameProblem("a", taken, "a")).toBeNull();
    expect(nameProblem("Name", taken, "a")).toMatch(/already/);
  });

  it("suggests the first free name of the kind", () => {
    expect(freeName("text", new Set())).toBe("Text 1");
    expect(freeName("text", new Set(["Text 1", "Text 2"]))).toBe("Text 3");
    expect(freeName("radio", new Set(["Text 1"]))).toBe("Group 1");
    expect(freeName("signature", new Set())).toBe("Signature 1");
  });
});

describe("what the font can draw", () => {
  it("allows Latin-1, with the Nordic letters, and refuses the rest", () => {
    expect(textProblem("x", "Blåbærsyltetøy ÆØÅ café")).toBeNull();
    expect(textProblem("The default value", "price €5")).toMatch(/“€”/);
    expect(textProblem("x", "Привет")).toMatch(/cannot be drawn/);
    expect(textProblem("x", "a\nb")).toMatch(/cannot be drawn/);
    expect(textProblem("x", "a\nb", true)).toBeNull();
  });

  it("checks the properties of each kind", () => {
    const p = emptyProps();
    expect(propsProblem("text", { ...p, comb: true })).toMatch(/box for each character/);
    expect(propsProblem("text", { ...p, comb: true, maxLen: 8 })).toBeNull();
    expect(propsProblem("text", { ...p, comb: true, maxLen: 8, password: true })).toMatch(/cannot also be/);
    expect(propsProblem("text", { ...p, password: true, multiline: true })).toMatch(/one line/);
    expect(propsProblem("text", { ...p, maxLen: 3, defaultValue: "abcd" })).toMatch(/longer/);
    expect(propsProblem("text", { ...p, defaultValue: "€" })).toMatch(/cannot be drawn/);
    expect(propsProblem("combo", p)).toMatch(/at least one choice/);
    expect(propsProblem("list", { ...p, choices: ["a", "a"] })).toMatch(/twice/);
    expect(propsProblem("list", { ...p, choices: ["a", " "] })).toMatch(/empty/);
    expect(propsProblem("combo", { ...p, choices: ["a", "b"] })).toBeNull();
    expect(propsProblem("signature", p)).toBeNull();
  });
});

describe("boxes", () => {
  it("places the default size around a click and keeps it on the page", () => {
    const r = placeBox("text", [300, 400], [300, 400], PAGE);
    expect(r).toEqual([300 - DEFAULT_SIZES.text[0] / 2, 400 - DEFAULT_SIZES.text[1] / 2, 300 + DEFAULT_SIZES.text[0] / 2, 400 + DEFAULT_SIZES.text[1] / 2]);
    const corner = placeBox("text", [2, 2], [2, 2], PAGE);
    expect(corner[0]).toBe(0);
    expect(corner[1]).toBe(0);
    expect(corner[2] - corner[0]).toBe(DEFAULT_SIZES.text[0]);
    const far = placeBox("signature", [610, 790], [610, 790], PAGE);
    expect(far[2]).toBe(612);
    expect(far[3]).toBe(792);
  });

  it("gives a drag its own size, from any corner, and no side thinner than the least", () => {
    expect(placeBox("text", [100, 100], [300, 140], PAGE)).toEqual([100, 100, 300, 140]);
    expect(placeBox("text", [300, 140], [100, 100], PAGE)).toEqual([100, 100, 300, 140]);
    // A drag that stays thin gets a little more than the least side: a box that is exactly the least
    // can come out of the engine's arithmetic a hair under it and be refused.
    const thin = placeBox("text", [100, 100], [300, 101], PAGE);
    expect(thin[3] - thin[1]).toBeGreaterThan(MIN_SIDE);
    expect(thin[3] - thin[1]).toBeCloseTo(BOX_MIN, 5);
  });

  it("cuts a drag that goes past the edge of the page at the edge, and does not slide the box", () => {
    expect(placeBox("text", [500, 700], [700, 900], PAGE)).toEqual([500, 700, 612, 792]);
    expect(placeBox("text", [-50, -20], [100, 60], PAGE)).toEqual([0, 0, 100, 60]);
    // Next to the edge there is still room for the least side.
    const edge = placeBox("text", [611, 300], [700, 340], PAGE);
    expect(edge[2]).toBe(612);
    expect(edge[2] - edge[0]).toBeGreaterThanOrEqual(BOX_MIN);
  });

  it("gives every box numbers in hundredths, so that no side comes out a hair short", () => {
    const moved = moveBox([10.123456, 10.987654, 110.123456, 40.987654], 0.333333, 0.333333, PAGE);
    for (const v of moved) expect(Math.round(v * 100) / 100).toBe(v);
    const resized = resizeBox([100, 100, 104.1, 150], "w", 0.0000001, 0, PAGE);
    expect(resized[2] - resized[0]).toBeGreaterThanOrEqual(MIN_SIDE);
    expect(roundBox([1.234, 2.006, 3.1, 4])).toEqual([1.23, 2.01, 3.1, 4]);
  });

  it("moves a box and keeps its size at the edge of the page", () => {
    expect(moveBox([10, 10, 110, 40], 5, -20, PAGE)).toEqual([15, 0, 115, 30]);
    expect(moveBox([500, 700, 600, 780], 100, 100, PAGE)).toEqual([512, 712, 612, 792]);
    expect(keepOnPage([-5, -5, 700, 900], PAGE)).toEqual([0, 0, 612, 792]);
  });

  it("resizes by a handle, with the least side and the page as limits", () => {
    const r: Rect = [100, 100, 200, 150];
    expect(resizeBox(r, "se", 20, 10, PAGE)).toEqual([100, 100, 220, 160]);
    expect(resizeBox(r, "nw", -20, -10, PAGE)).toEqual([80, 90, 200, 150]);
    expect(resizeBox(r, "e", -500, 0, PAGE)).toEqual([100, 100, 100 + BOX_MIN, 150]);
    expect(resizeBox(r, "n", 0, 500, PAGE)).toEqual([100, 150 - BOX_MIN, 200, 150]);
    expect(resizeBox(r, "s", 0, 5000, PAGE)).toEqual([100, 100, 200, 792]);
    expect(resizeBox(r, "w", -5000, 0, PAGE)).toEqual([0, 100, 200, 150]);
  });
});

describe("snapping", () => {
  const lines = joinLines(pageLines(PAGE), fieldLines([[100, 300, 260, 322]]));

  it("pulls the edge of a box that is near a line onto it, and not one that is far", () => {
    // The left edge is 3 points from the left edge of the field above.
    const near = snapMove([103, 340, 203, 362], lines, 6);
    expect(near.rect).toEqual([100, 340, 200, 362]);
    expect(near.guides).toContainEqual({ axis: "x", value: 100, source: "field" });
    const far = snapMove([110, 340, 210, 362], lines, 6);
    expect(far.rect).toEqual([110, 340, 210, 362]);
    expect(far.guides).toEqual([]);
  });

  it("snaps to the edges and the middle of the page", () => {
    const edge = snapMove([2, 2, 102, 24], lines, 6);
    expect(edge.rect).toEqual([0, 0, 100, 22]);
    const middle = snapMove([253, 400, 353, 422], lines, 6);
    // The middle of the box (303) is 3 from the middle of the page (306).
    expect(middle.rect[0]).toBe(256);
  });

  it("snaps the sides that a handle moves and no others", () => {
    const r = snapResize([100, 100, 257, 150], "e", lines, 6);
    expect(r.rect).toEqual([100, 100, 260, 150]);
    expect(r.guides).toEqual([{ axis: "x", value: 260, source: "field" }]);
    const s = snapResize([100, 100, 200, 297], "s", lines, 6);
    expect(s.rect[3]).toBe(300);
    expect(s.rect[0]).toBe(100);
    const none = snapResize([100, 100, 200, 297], "e", lines, 6);
    expect(none.rect[3]).toBe(297);
  });

  it("does nothing with no lines", () => {
    const r: Rect = [10, 20, 110, 50];
    expect(snapMove(r, noLines(), 6).rect).toEqual(r);
    expect(snapResize(r, "se", noLines(), 6).rect).toEqual(r);
  });

  it("says when a box has the width or the height of another", () => {
    expect(sameSizeHints([0, 0, 160, 22], [[10, 10, 170, 40]])).toEqual(["width"]);
    expect(sameSizeHints([0, 0, 100, 30], [[10, 10, 170, 40]])).toEqual(["height"]);
    expect(sameSizeHints([0, 0, 100, 31], [[10, 10, 170, 40]])).toEqual([]);
  });

  it("takes the ends of the words and the tops and bottoms of the lines from the text of a page", () => {
    // "Name: x" on one line, 6 points a character from 72; "Next" on the line below.
    const text = "Name: x\nNext";
    const rects: Rect[] = [];
    [...text].forEach((ch, i) => {
      if (ch === "\n") rects.push([0, 0, 0, 0]);
      else if (i < 7) rects.push([72 + i * 6, 100, 78 + i * 6, 110]);
      else rects.push([72 + (i - 8) * 6, 120, 78 + (i - 8) * 6, 130]);
    });
    const { words, lines: rows } = textBoxes(text, rects);
    expect(words.map((w) => w.map(Math.round))).toEqual([
      [72, 100, 102, 110],
      [108, 100, 114, 110],
      [72, 120, 96, 130],
    ]);
    expect(rows.map((l) => l.map(Math.round))).toEqual([
      [72, 100, 114, 110],
      [72, 120, 96, 130],
    ]);
    const l = textLines(words, rows);
    expect(l.xs.map((x) => x.value)).toContain(102);
    expect(l.ys.map((y) => y.value)).toEqual([100, 110, 120, 130]);
    expect(l.xs.every((x) => x.source === "text")).toBe(true);
  });
});

describe("the model, changed the way the engine changes the document", () => {
  function model(): DesignModel {
    return new Map([[0, [widget("a", [10, 10, 110, 30]), widget("b", [10, 40, 110, 60])]]]);
  }

  it("adds a field and a button to a group", () => {
    const m = model();
    applyLocal(m, {
      op: "add",
      field: { name: "size", kind: "radio", props: {}, widgets: [{ page: 0, rect: [10, 70, 24, 84], export: "S" }] },
    });
    applyLocal(m, { op: "addWidget", name: "size", widget: { page: 0, rect: [40, 70, 54, 84], export: "M" } });
    const group = m.get(0)!.filter((w) => w.name === "size");
    expect(group.map((w) => [w.widget, w.widgets, w.export])).toEqual([
      [0, 2, "S"],
      [1, 2, "M"],
    ]);
    expect(namesIn(m)).toEqual(new Set(["a", "b", "size"]));
  });

  it("removes a field and one button of a group, and counts the buttons again", () => {
    const m = model();
    applyLocal(m, { op: "remove", name: "a" });
    expect(m.get(0)!.map((w) => w.name)).toEqual(["b"]);
    const g = model();
    applyLocal(g, {
      op: "add",
      field: {
        name: "size",
        kind: "radio",
        props: {},
        widgets: ["S", "M", "L"].map((e, i) => ({ page: 0, rect: [10 + 30 * i, 70, 24 + 30 * i, 84] as Rect, export: e })),
      },
    });
    applyLocal(g, { op: "removeWidget", name: "size", widget: 0 });
    const left = g.get(0)!.filter((w) => w.name === "size");
    expect(left.map((w) => [w.widget, w.widgets, w.export])).toEqual([
      [0, 2, "M"],
      [1, 2, "L"],
    ]);
  });

  it("moves a widget on its page and to another", () => {
    const m = model();
    applyLocal(m, { op: "move", name: "a", widget: 0, page: 0, rect: [20, 20, 120, 40] });
    expect(m.get(0)![0].rect).toEqual([20, 20, 120, 40]);
    applyLocal(m, { op: "move", name: "a", widget: 0, page: 1, rect: [5, 5, 50, 25] });
    expect(m.get(0)!.map((w) => w.name)).toEqual(["b"]);
    expect(m.get(1)!.map((w) => w.name)).toEqual(["a"]);
  });

  it("renames a field and changes what it is", () => {
    const m = model();
    applyLocal(m, { op: "update", name: "a", newName: "c", props: { ...emptyProps(), tooltip: "hi", required: true } });
    const c = m.get(0)![0];
    expect(c.name).toBe("c");
    expect(c.props.tooltip).toBe("hi");
    expect(c.props.required).toBe(true);
    // The same name keeps it.
    applyLocal(m, { op: "update", name: "c", newName: "", props: emptyProps() });
    expect(m.get(0)![0].name).toBe("c");
  });

  it("puts widgets in a new order", () => {
    const m = model();
    applyLocal(m, {
      op: "reorder",
      page: 0,
      order: [
        { name: "b", widget: 0 },
        { name: "a", widget: 0 },
      ],
    });
    expect(m.get(0)!.map((w) => w.name)).toEqual(["b", "a"]);
    // A name that is not there changes nothing.
    applyLocal(m, { op: "reorder", page: 0, order: [{ name: "zzz", widget: 0 }] });
    expect(m.get(0)!.map((w) => w.name)).toEqual(["b", "a"]);
  });

  it("moves an entry of a list", () => {
    expect(moveEntry(["a", "b", "c"], 0, 2)).toEqual(["b", "c", "a"]);
    expect(moveEntry(["a", "b", "c"], 2, 1)).toEqual(["a", "c", "b"]);
    expect(moveEntry(["a", "b"], 0, 5)).toEqual(["a", "b"]);
  });

  it("says what a box is, for a screen reader", () => {
    expect(describeWidget(widget("Name", [0, 0, 1, 1]))).toBe("Name, text field");
    expect(describeWidget(widget("size", [0, 0, 1, 1], { kind: "radio", widget: 1, widgets: 3 }))).toBe(
      "size, radio button, button 2 of 3",
    );
    expect(describeWidget(widget("x", [0, 0, 1, 1], { editable: false }))).toMatch(/cannot be changed here/);
  });
});

// ---- the rules are the engine's -------------------------------------------------------------------

/**
 * crates/engine/src/author/rules.rs reads the same file and holds the engine to it: a line is
 * `kind <tab> ok|no <tab> text`, with \uXXXX for a character that is not typed.
 */
function cases(): { kind: string; ok: boolean; text: string }[] {
  const file = readFileSync(new URL("./formrules.cases.txt", import.meta.url), "utf8");
  return file
    .split("\n")
    .map((raw) => raw.replace(/\r$/, ""))
    .filter((line) => line.trim() !== "" && !line.startsWith("#"))
    .map((line) => {
      const [kind, verdict, ...rest] = line.split("\t");
      const text = rest.join("\t").replace(/\\u([0-9a-fA-F]{4})/g, (_, h: string) => String.fromCharCode(parseInt(h, 16))).replace(/\\n/g, "\n");
      return { kind, ok: verdict === "ok", text };
    });
}

describe("the rules of the window and of the engine", () => {
  const props = (over: Partial<DesignProps>): DesignProps => ({ ...emptyProps(), ...over });
  const verdict = (kind: string, text: string): string | null => {
    switch (kind) {
      case "name":
        return nameProblem(text, new Set());
      case "tooltip":
        return tooltipProblem(text);
      case "export":
        return exportProblem(text);
      case "drawn":
        return textProblem("x", text);
      case "kept":
        // A word the reader left as it was.
        return exportsProblem([text], "", [text]);
      case "multiline":
        return textProblem("x", text, true);
      case "fontsize":
        return propsProblem("text", props({ fontSize: Number(text) }));
      case "maxlen":
        return propsProblem("text", props({ maxLen: Number(text) }));
      default:
        throw new Error(`a kind of case that is not known: ${kind}`);
    }
  };

  it("agree on every case of the file the engine is held to", () => {
    const all = cases();
    expect(all.length).toBeGreaterThan(40);
    for (const c of all) {
      const problem = verdict(c.kind, c.text);
      expect(problem === null, `${c.kind} ${JSON.stringify(c.text)}: ${problem ?? "accepted"}`).toBe(c.ok);
    }
  });

  it("let the joiners of Persian through and refuse the mark that reverses Arabic", () => {
    expect(nameProblem("a\u200cb", new Set())).toBeNull();
    expect(nameProblem("a\u200db", new Set())).toBeNull();
    expect(nameProblem("a\u061cb", new Set())).not.toBeNull();
  });

  it("check the tooltip, the words of the buttons, the limits and the default of a list", () => {
    expect(propsProblem("text", props({ tooltip: "x".repeat(513) }))).toMatch(/tooltip is too long/);
    expect(propsProblem("text", props({ maxLen: 32768 }))).toMatch(/at most 32767/);
    expect(propsProblem("text", props({ fontSize: 97 }))).toMatch(/between 1 and 96/);
    expect(propsProblem("text", props({ fontSize: 0.5 }))).toMatch(/between 1 and 96/);
    expect(propsProblem("checkbox", props({ exportValue: "Off" }))).toMatch(/Off/);
    expect(propsProblem("checkbox", props({ exportValue: "J\u00e5" }))).toMatch(/cannot be used/);
    expect(propsProblem("checkbox", props({ defaultValue: "maybe" }))).toMatch(/on or off/);
    const list = props({ choices: ["a", "b"], defaultValue: "z" });
    expect(propsProblem("list", list)).toMatch(/not one of the choices/);
    expect(propsProblem("combo", { ...list, editable: true })).toBeNull();
    expect(propsProblem("list", { ...list, defaultValue: "a\nb" })).toMatch(/several/);
    expect(propsProblem("list", { ...list, defaultValue: "a\nb", multiselect: true })).toBeNull();
    expect(propsProblem("list", props({ choices: Array.from({ length: 501 }, (_, i) => String(i)) }))).toMatch(/at most 500/);
    expect(propsProblem("list", props({ choices: ["x".repeat(257)] }))).toMatch(/too long/);
    expect(propsProblem("text", props({ defaultValue: "x".repeat(8193) }))).toMatch(/too long/);
    expect(exportsProblem(["S", "M"], "M")).toBeNull();
    expect(exportsProblem(["S", "M"], "XL")).toMatch(/not one of the buttons/);
    expect(exportsProblem(["S", "S"], "")).toMatch(/twice|its own/);
    expect(exportsProblem(["S", ""], "")).toMatch(/Give each/);
    // A word that was left as it was is not asked about again, a new one is, and the words still differ.
    expect(exportsProblem(["S\u00ed", "Andre"], "", ["S\u00ed", "Andre"])).toBeNull();
    expect(exportsProblem(["S\u00ed", "\u00c1ndre"], "", ["S\u00ed", "Andre"])).toMatch(/cannot be used/);
    expect(exportsProblem(["S\u00ed", "S\u00ed"], "", ["S\u00ed", "S\u00ed"])).toMatch(/its own/);
  });
});

describe("a radio group when only some of its pages are known", () => {
  const w = (name: string, widget: number, widgets: number, rect: Rect): DesignWidgetInfo => ({
    name,
    kind: "radio",
    widget,
    widgets,
    rect,
    export: `C${widget + 1}`,
    editable: true,
    props: emptyProps(),
  });

  it("keeps the engine's count and order when a button is added or taken away", () => {
    // A group of five buttons: the buttons 0 and 4 are on page 0, the rest on pages that were not read.
    const model: DesignModel = new Map([[0, [w("g", 0, 5, [10, 10, 24, 24]), w("g", 4, 5, [40, 10, 54, 24])]]]);
    applyLocal(model, { op: "removeWidget", name: "g", widget: 0 });
    let list = model.get(0) as DesignWidgetInfo[];
    // The button that was the fifth is the fourth now, and the group has four.
    expect(list.map((x) => [x.widget, x.widgets])).toEqual([[3, 4]]);
    applyLocal(model, { op: "removeWidget", name: "g", widget: 3 });
    list = model.get(0) as DesignWidgetInfo[];
    expect(list).toHaveLength(0);
    // A button added to a group that is only partly known goes after all of them.
    const known: DesignModel = new Map([[0, [w("g", 1, 5, [10, 10, 24, 24])]]]);
    applyLocal(known, { op: "addWidget", name: "g", widget: { page: 0, rect: [60, 10, 74, 24], export: "C6" } });
    const after = known.get(0) as DesignWidgetInfo[];
    expect(after.map((x) => [x.widget, x.widgets])).toEqual([
      [1, 6],
      [5, 6],
    ]);
  });
});

describe("what a screen reader is told", () => {
  it("says where a box is and how big", () => {
    expect(describeBox("Text 1", [170.4, 439.2, 330.4, 461.2], 1)).toBe(
      "Text 1 is on page 2, 170 points from the left and 439 from the top, 160 points wide and 22 tall.",
    );
  });
});

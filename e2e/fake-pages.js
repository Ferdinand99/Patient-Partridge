// The page tools for the browser tests, on top of the stand-in for the Rust side (fake-tauri.js,
// which must be loaded first). It adds documents of its own and answers the commands of the page
// tools the way the real app process does, as far as the window can tell: the same arguments, the
// same answers and the same error codes. The documents are made up:
//
//   - /docs/ten.pdf: ten Letter pages
//   - /docs/mixed.pdf: five pages of different sizes
//   - /docs/many.pdf: 3000 Letter pages
//   - /docs/locked-doc.pdf: four pages, protected by a password (it is open already)
//   - /docs/signed-doc.pdf: three pages, signed
//   - /docs/form-ten.pdf: four Letter pages, a form with one text field ("Full name") on every
//   - /docs/design.pdf: two Letter pages and no form, for the form designer: the commands
//     author_check, author_list and author_apply keep a list of widgets and follow the rules of the
//     engine (a name is free, has no dot and is not empty; a batch is all or nothing). Page 1 has the
//     word "Name:" at 72, 100, so the lines of the text can be snapped to.
//
// and the files that pages can be inserted from (the file dialog answers with the paths in
// `__E2E__.dialog`):
//
//   - /docs/second.pdf: 2 pages, /docs/third.pdf: 3 pages
//   - /docs/locked.pdf: 2 pages, password "secret"; its pages join without the protection only when
//     the window says that is all right
//   - /docs/signed.pdf: 1 page, signed; the same goes for the signature
//   - /docs/bad.pdf: not usable
//   - /docs/slow.pdf: 4 pages that take two seconds to hand over, with progress; it can be stopped
//
// Every page has a number from when the document was opened, and the picture of a page is drawn with
// a shade that depends on that number, so a test can see in a thumbnail or on the page which page it
// is. `__E2E__.pages.labels(doc)` gives the numbers in the order of the pages now.
(() => {
  const base = window.__TAURI_INTERNALS__;
  const baseInvoke = base.invoke;
  const stand = window.__E2E__;

  // The channels the window makes for progress: the stand-in below sends to them.
  const channels = new Map();
  const transform = base.transformCallback;
  base.transformCallback = (callback, once) => {
    const id = transform(callback, once);
    channels.set(id, callback);
    return id;
  };

  const pg = {
    calls: [],
    saves: [],
    extracts: [],
    dirty: [],
    inserts: [],
    cancels: 0,
    // The handle of the document opened last.
    lastDoc: 0,
    // An error for the next call of a command: { cmd, error }.
    failNext: null,
    // How long the commands take, in milliseconds.
    delay: 0,
    // When set, only the commands of this document take that long.
    delayOnly: null,
    // True while a command of the page tools is in the middle of its work (a stop is too late).
    working: false,
    labels: (doc) => models.get(doc)?.pages.map((p) => p.id) ?? [],
    sizes: (doc) => models.get(doc)?.pages.map((p) => [p.w, p.h]) ?? [],
    snapshots: (doc) => models.get(doc)?.snaps.size ?? 0,
  };
  stand.pages = pg;

  const LETTER = [612, 792];
  const SPECS = {
    "/docs/ten.pdf": { sizes: Array.from({ length: 10 }, () => LETTER) },
    "/docs/mixed.pdf": { sizes: [LETTER, [842, 595], [300, 300], [200, 1000], LETTER] },
    "/docs/many.pdf": { sizes: Array.from({ length: 3000 }, () => LETTER) },
    "/docs/locked-doc.pdf": { sizes: Array.from({ length: 4 }, () => LETTER), protected: true },
    "/docs/signed-doc.pdf": { sizes: Array.from({ length: 3 }, () => LETTER), signed: true },
    "/docs/form-ten.pdf": { sizes: Array.from({ length: 4 }, () => LETTER), form: true },
    "/docs/design.pdf": { sizes: [LETTER, LETTER], design: true },
  };
  const SOURCES = {
    "/docs/second.pdf": { pages: 2 },
    "/docs/third.pdf": { pages: 3 },
    "/docs/locked.pdf": { pages: 2, password: "secret", protected: true },
    "/docs/signed.pdf": { pages: 1, signed: true },
    "/docs/bad.pdf": { broken: true },
    "/docs/slow.pdf": { pages: 4, slow: true },
  };

  const models = new Map();
  let nextDoc = 5000;

  const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const fresh = (id, [w, h]) => ({ id, w, h, turns: 0 });
  const shade = (id) => 40 + ((id * 37) % 190);

  function open(path) {
    const spec = SPECS[path];
    const doc = nextDoc++;
    pg.lastDoc = doc;
    models.set(doc, {
      path,
      pages: spec.sizes.map((s, i) => fresh(i, s)),
      next: spec.sizes.length,
      snaps: new Map(),
      nextSnap: 1,
      protected: Boolean(spec.protected),
      signed: Boolean(spec.signed),
      // The made-up form: one text field on every page.
      fields: spec.form ? [textField()] : null,
      // The widgets the form designer made, in the order of the annotations: { page, name, kind, rect, export, props }.
      design: spec.design ? [] : null,
    });
    return {
      doc,
      name: path.split("/").pop(),
      pages: spec.sizes.map((s) => [s[0], s[1]]),
      position: null,
    };
  }

  function textField() {
    return {
      index: 0,
      name: "fullname",
      kind: "text",
      readOnly: false,
      required: false,
      multiline: false,
      password: false,
      editable: false,
      multiselect: false,
      checked: false,
      valueCut: false,
      noToggleOff: false,
      comb: false,
      action: false,
      actionBlocked: false,
      formBlocked: false,
      turns: 0,
      fontSize: 12,
      tooltip: "Full name",
      value: "",
      maxLen: 0,
      options: [],
      rect: [150, 68, 400, 92],
    };
  }

  function model(doc) {
    const m = models.get(doc);
    if (!m) throw { code: "unknown_document", message: "gone" };
    return m;
  }

  const sizesOf = (m) => m.pages.map((p) => [p.w, p.h]);
  const copyPages = (m) => m.pages.map((p) => ({ ...p }));
  const changed = (m, snapshot = 0, count = 1, formFields = 0) => ({
    pages: sizesOf(m),
    snapshot,
    count,
    formFields,
  });

  function keep(m) {
    const token = m.nextSnap++;
    m.snaps.set(token, { pages: copyPages(m), design: structuredClone(m.design) });
    return token;
  }

  // ---- the form designer -----------------------------------------------------

  const refuse = (message) => {
    throw { code: "author_refused", message };
  };
  const designError = (m) => {
    if (!m.design) throw { code: "author_unsupported", message: "this document is encrypted, so fields cannot be added to it" };
  };
  const emptyProps = () => ({
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
  });
  const widgetsOf = (list, name) => list.filter((w) => w.name === name);

  function listPage(m, page) {
    designError(m);
    return m.design
      .filter((w) => w.page === page)
      .map((w) => {
        const mine = widgetsOf(m.design, w.name);
        return {
          name: w.name,
          kind: w.kind,
          widget: mine.indexOf(w),
          widgets: mine.length,
          rect: w.rect,
          export: w.export,
          editable: true,
          props: structuredClone(w.props),
        };
      });
  }

  function applyDesign(m, ops) {
    designError(m);
    const list = structuredClone(m.design);
    const taken = (name) => list.some((w) => w.name === name);
    const need = (name, widget = 0) => {
      const w = widgetsOf(list, name)[widget];
      if (!w) refuse("There is no field called \u201c" + name + "\u201d that can be changed here.");
      return w;
    };
    for (const op of ops) {
      switch (op.op) {
        case "add": {
          const f = op.field;
          if (f.name.trim() === "") refuse("Give the field a name.");
          if (f.name.includes(".")) refuse("A field name cannot contain a dot, because a dot makes the field a part of another field. Use a dash or an underscore instead.");
          if (taken(f.name)) refuse("There is already a field called \u201c" + f.name + "\u201d. Every field needs a name of its own.");
          for (const w of f.widgets) {
            list.push({ page: w.page, name: f.name, kind: f.kind, rect: w.rect, export: w.export ?? "", props: { ...emptyProps(), ...f.props } });
          }
          break;
        }
        case "remove":
          need(op.name);
          for (let i = list.length - 1; i >= 0; i--) if (list[i].name === op.name) list.splice(i, 1);
          break;
        case "removeWidget": {
          const w = need(op.name, op.widget);
          list.splice(list.indexOf(w), 1);
          break;
        }
        case "addWidget": {
          const first = need(op.name);
          list.push({ ...structuredClone(first), page: op.widget.page, rect: op.widget.rect, export: op.widget.export ?? "" });
          break;
        }
        case "move": {
          const w = need(op.name, op.widget);
          w.page = op.page;
          w.rect = op.rect;
          break;
        }
        case "update": {
          const mine = widgetsOf(list, op.name);
          need(op.name);
          if (op.newName !== "" && op.newName !== op.name) {
            if (op.newName.includes(".")) refuse("A field name cannot contain a dot, because a dot makes the field a part of another field. Use a dash or an underscore instead.");
            if (taken(op.newName)) refuse("There is already a field called \u201c" + op.newName + "\u201d. Every field needs a name of its own.");
          }
          mine.forEach((w, i) => {
            w.props = { ...op.props };
            if (op.exports && op.exports.length === mine.length) w.export = op.exports[i];
            if (op.newName !== "") w.name = op.newName;
          });
          break;
        }
        case "reorder": {
          const onPage = list.filter((w) => w.page === op.page);
          const wanted = op.order.map((r) => widgetsOf(list, r.name)[r.widget]);
          if (wanted.some((w) => !w || w.page !== op.page)) refuse("A field in the order is not on that page.");
          const slots = wanted.map((w) => list.indexOf(w)).sort((a, b) => a - b);
          const copy = [...list];
          slots.forEach((slot, k) => (copy[slot] = wanted[k]));
          list.splice(0, list.length, ...copy);
          void onPage;
          break;
        }
        default:
          refuse("That change is not known.");
      }
    }
    return list;
  }

  function uniqueSorted(list, limit) {
    const sorted = [...new Set(list)].sort((a, b) => a - b);
    if (sorted.length === 0) throw { code: "invalid_request", message: "no pages were named" };
    if (sorted[sorted.length - 1] >= limit) throw { code: "page_out_of_range", message: "a page that was named does not exist" };
    return sorted;
  }

  function send(channel, index, message) {
    channels.get(channel.id)?.({ index, message });
  }

  async function insertFrom(m, args) {
    const source = SOURCES[args.path];
    // What the engine answers for a file it cannot use: it is the other file that has the fault.
    if (!source || source.broken) throw { code: "insert_source_failed", message: "the file could not be used" };
    if (source.password) {
      if (!args.password) throw { code: "password_required", message: "locked" };
      if (args.password !== source.password) throw { code: "password_incorrect", message: "wrong" };
    }
    if (source.protected && !args.allowProtected) throw { code: "insert_protected", message: "protected" };
    if (source.signed && !args.allowSigned) throw { code: "insert_signed", message: "signed" };
    if (args.at > m.pages.length) throw { code: "page_out_of_range", message: "no such place" };
    const total = 100;
    let index = 0;
    if (source.slow) {
      pg.cancelled = false;
      for (let step = 1; step <= 10; step++) {
        await delay(200);
        if (pg.cancelled) throw { code: "cancelled", message: "it was stopped" };
        send(args.progress, index++, [step * 10, total]);
      }
    } else {
      send(args.progress, index, [total, total]);
      if (pg.delay) await delay(pg.delay);
    }
    const snapshot = keep(m);
    const added = Array.from({ length: source.pages }, () => fresh(m.next++, LETTER));
    m.pages.splice(args.at, 0, ...added);
    pg.inserts.push({ path: args.path, at: args.at, password: args.password ?? null });
    return { changed: changed(m, snapshot, source.pages), inserted: source.pages };
  }

  pg.designWidgets = (doc) => structuredClone(models.get(doc)?.design ?? []);

  async function invoke(cmd, args = {}) {
    if (cmd === "open_pdf" && SPECS[args.path]) return open(args.path);
    const isMine = cmd.startsWith("pages_") || (typeof args.doc === "number" && models.has(args.doc));
    // The form designer works on the documents of this file only.
    if (cmd.startsWith("author_") && !models.has(args.doc)) return baseInvoke(cmd, args);
    if (!isMine) return baseInvoke(cmd, args);
    pg.calls.push({ cmd, args });
    if (pg.failNext && pg.failNext.cmd === cmd) {
      const { error } = pg.failNext;
      pg.failNext = null;
      throw error;
    }
    if (pg.delay && (pg.delayOnly === null || args.doc === pg.delayOnly) && cmd !== "pages_insert_pdf") {
      pg.working = cmd.startsWith("pages_");
      try {
        await delay(pg.delay);
      } finally {
        pg.working = false;
      }
    }
    switch (cmd) {
      case "render_page": {
        const m = model(args.doc);
        const page = m.pages[args.page];
        if (!page) throw { code: "page_out_of_range", message: "no such page" };
        const { width: w, height: h } = args;
        const rgba = new Uint8ClampedArray(w * h * 4).fill(255);
        const band = Math.floor(w * 0.45);
        const gray = shade(page.id);
        for (let y = 0; y < h; y++) {
          for (let x = 0; x < band; x++) {
            const o = (y * w + x) * 4;
            rgba[o] = rgba[o + 1] = rgba[o + 2] = gray;
          }
        }
        return rgba.buffer;
      }
      case "close_pdf":
        models.delete(args.doc);
        return null;
      case "form_info":
        if (!model(args.doc).fields) return baseInvoke(cmd, args);
        return { kind: "acro", actions: false };
      case "get_form_fields": {
        const { fields } = model(args.doc);
        if (!fields) return baseInvoke(cmd, args);
        return structuredClone(fields);
      }
      case "set_form_field": {
        const { fields } = model(args.doc);
        if (!fields) return baseInvoke(cmd, args);
        const f = fields.find((x) => x.index === args.index && x.name === args.name);
        if (!f) throw { code: "form_field_refused", message: "not the field" };
        if (args.value.kind === "text") f.value = args.value.text;
        return structuredClone(fields);
      }
      case "get_outline":
      case "get_page_links":
      case "search_page":
        return [];
      case "get_page_text": {
        const m = model(args.doc);
        if (m.design && args.page === 0) {
          // "Name:" at 72, 100: six points a letter, ten points tall.
          return { text: "Name:", rects: Array.from({ length: 5 }, (_, i) => [72 + i * 6, 100, 78 + i * 6, 110]).flat() };
        }
        return { text: "", rects: [] };
      }
      case "author_check": {
        const m = model(args.doc);
        designError(m);
        return [...new Set(m.design.map((w) => w.name))].sort();
      }
      case "author_list": {
        const m = model(args.doc);
        return listPage(m, args.page);
      }
      case "author_apply": {
        const m = model(args.doc);
        const next = applyDesign(m, args.ops);
        const snapshot = keep(m);
        m.design = next;
        return changed(m, snapshot, args.ops.length);
      }
      case "missed_fonts":
        return [];
      case "set_dirty":
        pg.dirty.push([args.doc, args.dirty]);
        return null;
      case "save_pdf": {
        const m = model(args.doc);
        if (m.signed && !args.allowSigned) throw { code: "save_signed", message: "signed" };
        pg.saves.push({ doc: args.doc, labels: m.pages.map((p) => p.id), sizes: sizesOf(m) });
        return { path: m.path, name: m.path.split("/").pop(), touched: false };
      }
      case "save_pdf_as": {
        const m = model(args.doc);
        pg.saves.push({ doc: args.doc, labels: m.pages.map((p) => p.id), sizes: sizesOf(m), as: true });
        return { path: "/docs/copy.pdf", name: "copy.pdf", touched: false };
      }
      case "pages_delete": {
        const m = model(args.doc);
        const pages = uniqueSorted(args.pages, m.pages.length);
        if (pages.length >= m.pages.length) throw { code: "invalid_request", message: "a document needs at least one page" };
        const snapshot = keep(m);
        for (const p of [...pages].reverse()) m.pages.splice(p, 1);
        return changed(m, snapshot, pages.length, args.formFields ?? 0);
      }
      case "pages_reorder": {
        const m = model(args.doc);
        const order = args.order;
        const ok =
          order.length === m.pages.length &&
          new Set(order).size === order.length &&
          order.every((p) => Number.isInteger(p) && p >= 0 && p < m.pages.length);
        if (!ok) throw { code: "invalid_request", message: "the new order must name every page once" };
        const snapshot = keep(m);
        const old = m.pages;
        m.pages = order.map((p) => old[p]);
        return changed(m, snapshot, order.filter((p, i) => p !== i).length);
      }
      case "pages_rotate": {
        const m = model(args.doc);
        const pages = uniqueSorted(args.pages, m.pages.length);
        if (![1, 2, 3].includes(args.turns)) throw { code: "invalid_request", message: "a page turns by 1 to 3 quarter turns" };
        for (const i of pages) {
          const p = m.pages[i];
          p.turns = (p.turns + args.turns) % 4;
          if (args.turns % 2 === 1) [p.w, p.h] = [p.h, p.w];
        }
        return changed(m, 0, pages.length);
      }
      case "pages_crop": {
        const m = model(args.doc);
        const pages = uniqueSorted(args.pages, m.pages.length);
        const [left, top, right, bottom] = args.margins;
        const before = [];
        const after = [];
        for (const i of pages) {
          const p = m.pages[i];
          if (p.w - left - right < 8 || p.h - top - bottom < 8) {
            throw { code: "invalid_request", message: "the crop would leave too little of a page" };
          }
          before.push([0, 0, p.w, p.h]);
          after.push([left, bottom, p.w - right, p.h - top]);
        }
        pages.forEach((i, k) => {
          const p = m.pages[i];
          p.w = after[k][2] - after[k][0];
          p.h = after[k][3] - after[k][1];
        });
        return { pages: sizesOf(m), before, after };
      }
      case "pages_set_boxes": {
        const m = model(args.doc);
        args.pages.forEach((i, k) => {
          const [l, b, r, t] = args.boxes[k];
          m.pages[i].w = r - l;
          m.pages[i].h = t - b;
        });
        return changed(m, 0, args.pages.length);
      }
      case "pages_insert_blank": {
        const m = model(args.doc);
        if (args.at > m.pages.length) throw { code: "page_out_of_range", message: "no such place" };
        const snapshot = keep(m);
        m.pages.splice(args.at, 0, fresh(m.next++, [args.width, args.height]));
        return changed(m, snapshot, 1);
      }
      case "pages_restore": {
        const m = model(args.doc);
        const wanted = m.snaps.get(args.snapshot);
        if (!wanted) throw { code: "snapshot_gone", message: "the copy is gone" };
        m.snaps.delete(args.snapshot);
        const leaving = keep(m);
        m.pages = wanted.pages;
        m.design = wanted.design;
        return changed(m, leaving, 0);
      }
      case "pages_insert_pdf": {
        const m = model(args.doc);
        return insertFrom(m, args);
      }
      case "pages_extract": {
        const m = model(args.doc);
        const pages = uniqueSorted(args.pages, m.pages.length);
        if (m.signed && !args.allowSigned) throw { code: "save_signed", message: "signed" };
        if (m.protected && !args.allowUnprotected) throw { code: "save_unprotected", message: "protected" };
        send(args.progress, 0, [100, 100]);
        pg.extracts.push({ doc: args.doc, labels: pages.map((i) => m.pages[i].id) });
        return { path: "/docs/part.pdf", name: "part.pdf", touched: false };
      }
      case "pages_cancel":
        pg.cancels += 1;
        // A change that is under way is not stopped; the transfer of a file is.
        if (pg.working) return "working";
        pg.cancelled = true;
        return "handover";
      default:
        return baseInvoke(cmd, args);
    }
  }

  base.invoke = invoke;
})();

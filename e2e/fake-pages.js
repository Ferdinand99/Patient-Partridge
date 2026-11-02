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
    });
    return {
      doc,
      name: path.split("/").pop(),
      pages: spec.sizes.map((s) => [s[0], s[1]]),
      position: null,
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
    m.snaps.set(token, copyPages(m));
    return token;
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
    if (!source || source.broken) throw { code: "import_failed", message: "the file could not be used" };
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
      send(args.progress, index++, [total, total]);
      if (pg.delay) await delay(pg.delay);
    }
    const snapshot = keep(m);
    const added = Array.from({ length: source.pages }, () => fresh(m.next++, LETTER));
    m.pages.splice(args.at, 0, ...added);
    pg.inserts.push({ path: args.path, at: args.at, password: args.password ?? null });
    return { changed: changed(m, snapshot, source.pages), inserted: source.pages };
  }

  async function invoke(cmd, args = {}) {
    if (cmd === "open_pdf" && SPECS[args.path]) return open(args.path);
    const isMine = cmd.startsWith("pages_") || (typeof args.doc === "number" && models.has(args.doc));
    if (!isMine) return baseInvoke(cmd, args);
    pg.calls.push({ cmd, args });
    if (pg.failNext && pg.failNext.cmd === cmd) {
      const { error } = pg.failNext;
      pg.failNext = null;
      throw error;
    }
    if (pg.delay && cmd !== "pages_insert_pdf") await delay(pg.delay);
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
      case "get_outline":
      case "get_page_links":
      case "search_page":
        return [];
      case "get_page_text":
        return { text: "", rects: [] };
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
        m.pages = wanted;
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
        pg.cancelled = true;
        return "handover";
      default:
        return baseInvoke(cmd, args);
    }
  }

  base.invoke = invoke;
})();

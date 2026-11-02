// A stand-in for the Rust side of the app, for the browser tests. It is loaded into the page before
// the app starts and answers the same commands, with a small made-up document:
//
//   - /docs/book.pdf: 3 Letter pages with text, links and a table of contents
//   - /docs/second.pdf: 2 pages
//   - /docs/rtl.pdf: 1 page with two lines of Hebrew, laid out the way the real engine reports them
//     for rtl_pdf() in crates/testkit: letters in reading order, the first one at the right end
//   - /docs/turned.pdf: 1 landscape page (792 x 612 points) with two lines of text, the shape of a
//     page that is turned a quarter turn
//   - /docs/annotated.pdf: 1 page with annotations that "another program" made: a red highlight with
//     a comment, a note whose text looks like markup, a drawing and a text box
//   - /docs/slivers.pdf: 5000 pages one point high, each with the most text a page can have
//     (100,000 characters). It stands for a small file made to fill the memory of the window.
//   - /docs/crafted.pdf: 5000 pages one point high, each with 10,000 characters that need a box for
//     every two of them, and 500 links. It stands for a small file made to fill the window with
//     elements.
//
// Nothing here is part of the app. The app code is the real code, built by Vite and served with the
// same content security policy as the real window.
(() => {
  const PAGE = { w: 612, h: 792 };
  const state = {
    calls: [],
    dialog: ["/docs/book.pdf"],
    printed: 0,
    violations: [],
    title: "",
    // Files that are gone (the stand-in answers "io" for them), and one that needs a password.
    missingFiles: ["/docs/gone.pdf", "/docs/gone2.pdf"],
    protectedFiles: ["/docs/locked.pdf"],
    // The scripts the stand-in engine had no font for when it was asked (empty: it had them all).
    fonts: [],
    // The unsaved changes the window reported, by document, and the saves and quits it asked for.
    dirty: {},
    saves: [],
    quits: 0,
    stays: 0,
    // The scripts of the stand-in's annotations: what the next call of a kind should do instead.
    annotFailure: null,
    // The update side. `found` is what the next check returns (null: nothing new).
    update: {
      settings: { autoUpdate: true, method: "automatic", currentVersion: "0.1.0" },
      found: null,
      checkError: null,
      installError: null,
      downloadError: null,
      restored: [],
      installDelay: 0,
      restarted: 0,
      releasesPageOpened: 0,
    },
  };
  window.__E2E__ = state;

  document.addEventListener("securitypolicyviolation", (e) => {
    state.violations.push(`${e.violatedDirective} ${e.blockedURI}`);
  });
  window.print = () => {
    state.printed += 1;
  };

  // ---- the made-up documents -------------------------------------------------
  const LINES = {
    "/docs/book.pdf": [
      ["Hello Partridge", "A quick brown fox jumps over the lazy dog.", "Another needle in a haystack."],
      ["Second page has the second needle", "and more text here"],
      ["Third page", "The end of the book"],
    ],
    "/docs/second.pdf": [["Other document"], ["Its second page"]],
    "/docs/no-ending": [["A file whose name does not end in .pdf"]],
    "/docs/rtl.pdf": [[]],
    "/docs/turned.pdf": [["Turned page text goes here", "and a second line of it"]],
    "/docs/annotated.pdf": [["Hello Partridge", "A quick brown fox jumps over the lazy dog."]],
    "/docs/slivers.pdf": Array.from({ length: 5000 }, () => []),
    "/docs/crafted.pdf": Array.from({ length: 5000 }, () => []),
  };

  // The text of a sliver page: 100,000 tiny boxes. Every call gives new arrays, as a message from
  // the Rust side would, so what the window keeps is really kept.
  const SLIVER_TEXT = "x".repeat(100000);
  const SLIVER_TEMPLATE = Array.from({ length: 100000 }, () => [0, 0.2, 0.5, 0.8]).flat();

  // The text of a crafted page: characters that alternate between the right and the left of the page,
  // so that the selection needs a box for every two of them.
  const CRAFTED_TEXT = "x".repeat(10000);
  const CRAFTED_TEMPLATE = Array.from({ length: 10000 }, (_, i) =>
    i % 2 === 0 ? [300, 0.1, 300.5, 0.9] : [10, 0.1, 10.5, 0.9],
  ).flat();
  // 500 links to pages, the most the engine reports for one page.
  const CRAFTED_LINKS = Array.from({ length: 500 }, (_, i) => ({ rect: [0, 0, 1, 1], uri: null, page: i % 5000 }));

  // Text copied from the output of the real engine for the page of rtl_pdf() (see reader.rs).
  const RTL_TEXT = "\u05D0\u05D1\u05D2\u05D3\u05D4\u05D5\u05D6\u05D7\u05D8\u05D9\n\u05D5\u05D6\u05D7\u05D8\u05D9";
  const RTL_RECTS = [
    [405, 75, 414, 96.0625],
    [400, 75, 405, 96.0625],
    [387, 75, 400, 96.0625],
    [373, 75, 387, 96.0625],
    [362, 75, 373, 96.0625],
    [350, 75, 362, 96.0625],
    [337, 75, 350, 96.0625],
    [324, 75, 337, 96.0625],
    [312, 75, 324, 96.0625],
    [300, 75, 312, 96.0625],
    [0, 0, 0, 0],
    [350, 115, 362, 136.0625],
    [337, 115, 350, 136.0625],
    [324, 115, 337, 136.0625],
    [312, 115, 324, 136.0625],
    [300, 115, 312, 136.0625],
  ].flat();

  /** The text of a page of a document, with a box for every character. */
  function pageData(path, page) {
    if (path === "/docs/rtl.pdf") return { text: RTL_TEXT, rects: RTL_RECTS };
    if (path === "/docs/slivers.pdf") return { text: SLIVER_TEXT, rects: SLIVER_TEMPLATE.slice() };
    if (path === "/docs/crafted.pdf") return { text: CRAFTED_TEXT, rects: CRAFTED_TEMPLATE.slice() };
    return boxesFor(LINES[path]?.[page] ?? []);
  }

  function boxesFor(lines) {
    // One line of text per entry, 6 x 10 points per character, 18 points between lines.
    const chars = [];
    const boxes = [];
    lines.forEach((line, row) => {
      const top = 100 + row * 18;
      Array.from(line).forEach((ch, i) => {
        chars.push(ch);
        boxes.push(72 + i * 6, top, 78 + i * 6, top + 10);
      });
      if (row < lines.length - 1) {
        chars.push("\n");
        boxes.push(0, 0, 0, 0);
      }
    });
    return { text: chars.join(""), rects: boxes };
  }

  const docs = new Map();
  let nextDoc = 1;

  function openDoc(path) {
    if (state.missingFiles.includes(path)) throw { code: "io", message: "no such file" };
    if (state.protectedFiles.includes(path)) throw { code: "password_required", message: "locked" };
    const pages = LINES[path];
    if (!pages) throw { code: "corrupt", message: "not a pdf" };
    const doc = nextDoc++;
    docs.set(doc, { path, pages });
    return {
      doc,
      name: path.split("/").pop(),
      pages: pages.map(() => {
        const d = dims(path);
        return [d.w, d.h];
      }),
      position: null,
    };
  }

  /** The size of the pages of a document, in points. */
  function dims(path) {
    if (path === "/docs/slivers.pdf" || path === "/docs/crafted.pdf") return { w: PAGE.w, h: 1 };
    if (path === "/docs/turned.pdf") return { w: 792, h: 612 };
    return { w: PAGE.w, h: PAGE.h };
  }

  // A page drawn as white paper with a grey bar for every character, so selections and matches have
  // something to sit on, and with a blue box where the first link is.
  function renderPage(doc, page, w, h) {
    const rgba = new Uint8ClampedArray(w * h * 4).fill(255);
    const d = docs.get(doc);
    if (!d) throw { code: "unknown_document", message: "gone" };
    if (d.path === "/docs/slivers.pdf" || d.path === "/docs/crafted.pdf") return rgba.buffer;
    const { rects } = pageData(d.path, page);
    const size = dims(d.path);
    const sx = w / size.w;
    const sy = h / size.h;
    for (let i = 0; i < rects.length; i += 4) {
      if (rects[i + 2] === 0) continue;
      const x0 = Math.floor(rects[i] * sx);
      const y0 = Math.floor((rects[i + 1] + 2) * sy);
      const x1 = Math.ceil((rects[i + 2] - 1) * sx);
      const y1 = Math.ceil((rects[i + 3] - 2) * sy);
      for (let y = y0; y < y1 && y < h; y++) {
        for (let x = x0; x < x1 && x < w; x++) {
          const o = (y * w + x) * 4;
          rgba[o] = rgba[o + 1] = rgba[o + 2] = 70;
        }
      }
    }
    paintAnnotations(doc, page, rgba, w, h, size);
    return rgba.buffer;
  }

  // ---- annotations: a small copy of what the engine does ---------------------------------------
  // The boxes of markup are found from the boxes of the characters the way the engine finds them
  // (crates/engine/src/annots.rs, line_boxes), so that a selection of right-to-left text, say, ends
  // up as one box over exactly the letters.
  const annots = new Map();
  const gone = new Map();
  let nextAnnot = 1;

  const ABILITIES = {
    highlight: { editText: true, recolour: true, move: false, remove: true, restore: true, locked: false },
    underline: { editText: true, recolour: true, move: false, remove: true, restore: true, locked: false },
    strikeout: { editText: true, recolour: true, move: false, remove: true, restore: true, locked: false },
    note: { editText: true, recolour: false, move: true, remove: true, restore: true, locked: false },
    ink: { editText: true, recolour: false, move: false, remove: true, restore: false, locked: false },
    stamp: { editText: false, recolour: false, move: false, remove: true, restore: false, locked: false },
    other: { editText: false, recolour: false, move: false, remove: true, restore: false, locked: false },
  };
  const YELLOW = [255, 235, 59, 255];

  function made(kind, fields) {
    const can = { ...ABILITIES[kind] };
    // A drawing and a stamp that this session made can be put back; those of a file cannot.
    if (kind === "ink" || kind === "stamp") can.restore = true;
    return { id: nextAnnot++, kind, can, color: YELLOW, text: "", author: "", ...fields };
  }

  function listFor(doc, page) {
    const key = `${doc}:${page}`;
    if (!annots.has(key)) {
      const path = docs.get(doc)?.path;
      const list = [];
      if (path === "/docs/annotated.pdf" && page === 0) {
        list.push(
          { id: nextAnnot++, kind: "highlight", can: { ...ABILITIES.highlight }, rect: [72, 100, 200, 110], color: [230, 40, 40, 255], text: "Check this title", author: "Alice", boxes: [[72, 100, 200, 110]] },
          { id: nextAnnot++, kind: "note", can: { ...ABILITIES.note }, rect: [300, 50, 320, 70], color: YELLOW, text: "<img src=x onerror=alert(1)> a note from the file", author: "<b>Bob</b>" },
          { id: nextAnnot++, kind: "ink", can: { ...ABILITIES.ink }, rect: [100, 300, 200, 350], color: [0, 0, 255, 255], text: "", author: "", strokes: [[[100, 300], [150, 350], [200, 300]]], width: 3 },
          { id: nextAnnot++, kind: "other", can: { ...ABILITIES.other }, rect: [72, 500, 300, 540], color: YELLOW, text: "A text box", author: "" },
        );
      }
      annots.set(key, list);
    }
    return annots.get(key);
  }

  const overlap = (a0, a1, b0, b1) => Math.min(a1, b1) - Math.max(a0, b0);

  function joins(u, r) {
    const uw = u[2] - u[0];
    const uh = u[3] - u[1];
    const rw = r[2] - r[0];
    const rh = r[3] - r[1];
    const alongLine = overlap(u[1], u[3], r[1], r[3]) > 0.5 * Math.min(uh, rh) && overlap(u[0], u[2], r[0], r[2]) > -0.6 * Math.max(rh, uh);
    const downColumn = overlap(u[0], u[2], r[0], r[2]) > 0.5 * Math.min(uw, rw) && overlap(u[1], u[3], r[1], r[3]) > -0.6 * Math.max(rw, uw);
    return alongLine || downColumn;
  }

  function lineBoxes(chars, rects) {
    const out = [];
    let cur = null;
    chars.forEach((ch, i) => {
      const r = rects.slice(i * 4, i * 4 + 4);
      if (ch === "\n") {
        if (cur) out.push(cur);
        cur = null;
        return;
      }
      if (!(r[2] - r[0] > 0 && r[3] - r[1] > 0)) return;
      if (!cur) cur = r;
      else if (joins(cur, r)) cur = [Math.min(cur[0], r[0]), Math.min(cur[1], r[1]), Math.max(cur[2], r[2]), Math.max(cur[3], r[3])];
      else {
        out.push(cur);
        cur = r;
      }
    });
    if (cur) out.push(cur);
    return out;
  }

  function unionOf(boxes) {
    return [Math.min(...boxes.map((b) => b[0])), Math.min(...boxes.map((b) => b[1])), Math.max(...boxes.map((b) => b[2])), Math.max(...boxes.map((b) => b[3]))];
  }

  const STAMP_LABELS = { approved: "APPROVED", rejected: "REJECTED", draft: "DRAFT", final: "FINAL", confidential: "CONFIDENTIAL", reviewed: "REVIEWED" };
  const stampSize = (name) => [STAMP_LABELS[name].length * 26 * 0.72 + 18, 44];

  function failIfScripted(cmd) {
    const f = state.annotFailure;
    if (f && f.cmd === cmd) {
      if (f.once) state.annotFailure = null;
      throw f.error;
    }
  }

  function findAnnot(doc, page, id) {
    const list = listFor(doc, page);
    const at = list.findIndex((a) => a.id === id);
    if (at < 0) throw { code: "no_such_annotation", message: "gone" };
    return { list, at, item: list[at] };
  }

  const view = (a) => JSON.parse(JSON.stringify({ id: a.id, kind: a.kind, can: a.can, rect: a.rect, color: a.color, text: a.text, author: a.author }));

  function paintAnnotations(doc, page, rgba, w, h, size) {
    const sx = w / size.w;
    const sy = h / size.h;
    const put = (x, y, c, multiply) => {
      const X = Math.round(x * sx);
      const Y = Math.round(y * sy);
      if (X < 0 || Y < 0 || X >= w || Y >= h) return;
      const o = (Y * w + X) * 4;
      for (let k = 0; k < 3; k++) rgba[o + k] = multiply ? Math.round((rgba[o + k] * c[k]) / 255) : c[k];
    };
    const fill = (b, c, multiply) => {
      for (let y = Math.floor(b[1] * sy); y < Math.ceil(b[3] * sy); y++) for (let x = Math.floor(b[0] * sx); x < Math.ceil(b[2] * sx); x++) put(x / sx, y / sy, c, multiply);
    };
    for (const a of listFor(doc, page)) {
      if (a.kind === "highlight") for (const b of a.boxes ?? [a.rect]) fill(b, a.color, true);
      else if (a.kind === "underline") for (const b of a.boxes ?? [a.rect]) fill([b[0], b[3] - 1.5, b[2], b[3]], a.color, false);
      else if (a.kind === "strikeout") for (const b of a.boxes ?? [a.rect]) fill([b[0], (b[1] + b[3]) / 2 - 0.75, b[2], (b[1] + b[3]) / 2 + 0.75], a.color, false);
      else if (a.kind === "note") fill(a.rect, [255, 235, 59], false);
      else if (a.kind === "stamp") fill(a.rect, a.color, false);
      else if (a.kind === "ink") {
        const r = Math.max(1, (a.width ?? 3) / 2);
        for (const stroke of a.strokes) {
          for (let i = 0; i < stroke.length; i++) {
            const [x0, y0] = stroke[i];
            const [x1, y1] = stroke[Math.min(i + 1, stroke.length - 1)];
            const steps = Math.max(1, Math.ceil(Math.hypot(x1 - x0, y1 - y0) * 2));
            for (let s = 0; s <= steps; s++) {
              const x = x0 + ((x1 - x0) * s) / steps;
              const y = y0 + ((y1 - y0) * s) / steps;
              fill([x - r, y - r, x + r, y + r], a.color, false);
            }
          }
        }
      }
    }
  }

  function annotate(cmd, args) {
    switch (cmd) {
      case "list_annotations":
        return listFor(args.doc, args.page).map(view);
      case "add_markup": {
        failIfScripted(cmd);
        const d = docs.get(args.doc);
        const { text, rects } = pageData(d.path, args.page);
        const chars = Array.from(text);
        const start = args.start;
        const end = Math.min(args.end, chars.length);
        if (start >= end) throw { code: "invalid_request", message: "no text" };
        const boxes = lineBoxes(chars.slice(start, end), rects.slice(start * 4, end * 4));
        if (boxes.length === 0) throw { code: "annotation_refused", message: "no place" };
        const item = made(args.kind, { color: args.color, rect: unionOf(boxes), boxes });
        listFor(args.doc, args.page).push(item);
        return view(item);
      }
      case "add_note": {
        failIfScripted(cmd);
        const x = Math.min(Math.max(args.x, 0), dims(docs.get(args.doc).path).w - 20);
        const y = Math.min(Math.max(args.y, 0), dims(docs.get(args.doc).path).h - 20);
        const item = made("note", { rect: [x, y, x + 20, y + 20], text: args.text, color: YELLOW });
        listFor(args.doc, args.page).push(item);
        return view(item);
      }
      case "add_ink": {
        failIfScripted(cmd);
        const pts = args.strokes.flat();
        const pad = args.width / 2 + 1;
        const item = made("ink", {
          color: args.color,
          width: args.width,
          strokes: args.strokes,
          rect: [Math.min(...pts.map((p) => p[0])) - pad, Math.min(...pts.map((p) => p[1])) - pad, Math.max(...pts.map((p) => p[0])) + pad, Math.max(...pts.map((p) => p[1])) + pad],
        });
        listFor(args.doc, args.page).push(item);
        return view(item);
      }
      case "add_stamp": {
        failIfScripted(cmd);
        const [sw, sh] = stampSize(args.stamp);
        const item = made("stamp", { rect: [args.x, args.y, args.x + sw, args.y + sh], text: STAMP_LABELS[args.stamp], color: [26, 127, 55, 255] });
        listFor(args.doc, args.page).push(item);
        return view(item);
      }
      case "update_annotation": {
        failIfScripted(cmd);
        const { item } = findAnnot(args.doc, args.page, args.id);
        if ((args.text !== null && !item.can.editText) || (args.color !== null && !item.can.recolour) || ((args.dx || args.dy) && !item.can.move)) {
          throw { code: "annotation_refused", message: "not allowed" };
        }
        if (args.text !== null) item.text = args.text;
        if (args.color !== null) item.color = args.color;
        if (args.dx || args.dy) item.rect = [item.rect[0] + args.dx, item.rect[1] + args.dy, item.rect[2] + args.dx, item.rect[3] + args.dy];
        return view(item);
      }
      case "remove_annotation": {
        failIfScripted(cmd);
        const { list, at, item } = findAnnot(args.doc, args.page, args.id);
        list.splice(at, 1);
        gone.set(`${args.doc}:${args.page}:${args.id}`, item);
        return null;
      }
      case "restore_annotation": {
        failIfScripted(cmd);
        const key = `${args.doc}:${args.page}:${args.id}`;
        const item = gone.get(key);
        if (!item || !item.can.restore) throw { code: "annotation_refused", message: "cannot be put back" };
        gone.delete(key);
        listFor(args.doc, args.page).push(item);
        return view(item);
      }
      default:
        return undefined;
    }
  }

  const LINKS = {
    "/docs/book.pdf": {
      0: [
        { rect: [72, 40, 222, 60], uri: "https://example.com/hello?from=pdf", page: null },
        { rect: [72, 64, 222, 84], uri: "file:///etc/passwd", page: null },
        { rect: [300, 40, 400, 60], uri: null, page: 2 },
      ],
    },
  };

  const OUTLINE = {
    "/docs/book.pdf": [
      { depth: 0, page: 0, title: "Chapter 1" },
      { depth: 1, page: 1, title: "Section 1.1" },
      { depth: 0, page: null, title: "On the web" },
      { depth: 0, page: 2, title: "Chapter 3" },
    ],
  };

  function search(doc, page, query) {
    const d = docs.get(doc);
    if (!d) throw { code: "unknown_document", message: "gone" };
    const { text } = pageData(d.path, page);
    const hay = Array.from(text).map((c) => (c === "\n" ? " " : c.toLowerCase()));
    const needle = Array.from(query.toLowerCase());
    const found = [];
    for (let i = 0; i + needle.length <= hay.length; i++) {
      if (needle.every((c, k) => hay[i + k] === c)) {
        found.push([i, needle.length]);
        i += needle.length - 1;
      }
    }
    return found;
  }

  // ---- the commands ----------------------------------------------------------
  let callbackId = 1;
  const callbacks = new Map();
  const handlers = new Map();
  // Sends the window an event, as the Rust side does.
  state.emit = (name, payload) => {
    for (const { id, handler } of handlers.get(name) ?? []) callbacks.get(handler)?.({ event: name, id, payload });
  };

  async function invoke(cmd, args = {}) {
    state.calls.push({ cmd, args });
    switch (cmd) {
      case "plugin:event|listen": {
        // The handlers of events are kept, so that a test can send the window an event.
        const id = callbackId++;
        const list = handlers.get(args.event) ?? [];
        list.push({ id, handler: args.handler });
        handlers.set(args.event, list);
        return id;
      }
      case "list_annotations":
      case "add_markup":
      case "add_note":
      case "add_ink":
      case "add_stamp":
      case "update_annotation":
      case "remove_annotation":
      case "restore_annotation":
        return annotate(cmd, args);
      case "set_dirty":
        state.dirty[args.doc] = args.dirty;
        return null;
      case "save_pdf": {
        state.saves.push({ doc: args.doc, overwriteChanged: args.overwriteChanged, allowSigned: args.allowSigned });
        const path = docs.get(args.doc)?.path ?? "/docs/book.pdf";
        return { path, name: path.split("/").pop() };
      }
      case "quit_app":
        state.quits += 1;
        return null;
      case "stay_open":
        state.stays += 1;
        return null;
      case "plugin:event|unlisten":
        return null;
      case "plugin:window|set_title":
        state.title = args.value ?? "";
        return null;
      case "plugin:dialog|open":
        return state.dialog.length > 1 ? state.dialog : (state.dialog[0] ?? null);
      case "take_pending_paths":
        return [];
      case "measure_mode":
        return false;
      case "open_pdf":
        return openDoc(args.path);
      case "render_page": {
        const bytes = renderPage(args.doc, args.page, args.width, args.height);
        return bytes;
      }
      case "close_pdf":
        docs.delete(args.doc);
        return null;
      case "get_outline":
        return OUTLINE[docs.get(args.doc)?.path] ?? [];
      case "get_page_text":
        return pageData(docs.get(args.doc)?.path, args.page);
      case "missed_fonts":
        return state.fonts.slice();
      case "get_page_links":
        if (docs.get(args.doc)?.path === "/docs/crafted.pdf") return CRAFTED_LINKS.slice();
        return LINKS[docs.get(args.doc)?.path]?.[args.page] ?? [];
      case "search_page":
        return search(args.doc, args.page, args.query);
      case "open_link":
        return null;
      case "get_update_settings":
        return { ...state.update.settings };
      case "set_auto_update":
        state.update.settings.autoUpdate = args.enabled;
        return null;
      case "check_update":
        if (state.update.checkError) throw state.update.checkError;
        return state.update.found;
      case "download_update": {
        // Sends the same messages the Rust side does, through the channel the window made.
        const send = (index, message) => callbacks.get(args.onEvent.id)({ index, message });
        send(0, { event: "progress", chunk: 524288, total: 2097152 });
        await new Promise((r) => setTimeout(r, 50));
        send(1, { event: "progress", chunk: 1572864, total: 2097152 });
        if (state.update.downloadError) throw state.update.downloadError;
        return null;
      }
      case "install_update":
        await new Promise((r) => setTimeout(r, state.update.installDelay));
        if (state.update.installError) throw state.update.installError;
        return null;
      case "take_restored_paths": {
        const list = state.update.restored;
        state.update.restored = [];
        return list;
      }
      case "restart_app":
        state.update.restarted += 1;
        return null;
      case "open_releases_page":
        state.update.releasesPageOpened += 1;
        return null;
      // The list of tabs and its setting live in the session storage of the page, so that they
      // outlive a reload the way the files of the real app outlive a restart. "e2e-reopen" is
      // "off" when the setting is off.
      case "get_reopen_tabs":
        return sessionStorage.getItem("e2e-reopen") !== "off";
      case "set_reopen_tabs":
        sessionStorage.setItem("e2e-reopen", args.enabled ? "on" : "off");
        if (!args.enabled) sessionStorage.removeItem("e2e-session");
        return null;
      case "save_session":
        if (sessionStorage.getItem("e2e-reopen") !== "off") {
          sessionStorage.setItem("e2e-session", JSON.stringify({ paths: args.paths, active: args.active, failed: args.failed }));
        }
        return null;
      case "take_session": {
        const stored = sessionStorage.getItem("e2e-session");
        return sessionStorage.getItem("e2e-reopen") === "off" || !stored
          ? { paths: [], active: null }
          : JSON.parse(stored);
      }
      case "set_position":
      case "clear_positions":
      case "frontend_error":
      case "print_webview":
        return null;
      default:
        throw { code: "internal", message: `the test stand-in does not know ${cmd}` };
    }
  }

  window.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback(callback) {
      const id = callbackId++;
      callbacks.set(id, callback);
      return id;
    },
    unregisterCallback(id) {
      callbacks.delete(id);
    },
    convertFileSrc: (path) => path,
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { label: "main", windowLabel: "main" },
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener() {},
  };
})();

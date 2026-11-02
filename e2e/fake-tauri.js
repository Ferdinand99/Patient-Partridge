// A stand-in for the Rust side of the app, for the browser tests. It is loaded into the page before
// the app starts and answers the same commands, with a small made-up document:
//
//   - /docs/book.pdf: 3 Letter pages with text, links and a table of contents
//   - /docs/second.pdf: 2 pages
//   - /docs/rtl.pdf: 1 page with two lines of Hebrew, laid out the way the real engine reports them
//     for rtl_pdf() in crates/testkit: letters in reading order, the first one at the right end
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
      pages: pages.map(() => (path === "/docs/slivers.pdf" || path === "/docs/crafted.pdf" ? [PAGE.w, 1] : [PAGE.w, PAGE.h])),
      position: null,
    };
  }

  // A page drawn as white paper with a grey bar for every character, so selections and matches have
  // something to sit on, and with a blue box where the first link is.
  function renderPage(doc, page, w, h) {
    const rgba = new Uint8ClampedArray(w * h * 4).fill(255);
    const d = docs.get(doc);
    if (!d) throw { code: "unknown_document", message: "gone" };
    if (d.path === "/docs/slivers.pdf" || d.path === "/docs/crafted.pdf") return rgba.buffer;
    const { rects } = pageData(d.path, page);
    const sx = w / PAGE.w;
    const sy = h / PAGE.h;
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
    return rgba.buffer;
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

  async function invoke(cmd, args = {}) {
    state.calls.push({ cmd, args });
    switch (cmd) {
      case "plugin:event|listen":
        return callbackId++;
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

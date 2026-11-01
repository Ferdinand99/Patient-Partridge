// A stand-in for the Rust side of the app, for the browser tests. It is loaded into the page before
// the app starts and answers the same commands, with a small made-up document:
//
//   - /docs/book.pdf: 3 Letter pages with text, links and a table of contents
//   - /docs/second.pdf: 2 pages
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
    missingFiles: [],
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
  };

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
    const pages = LINES[path];
    if (!pages) throw { code: "corrupt", message: "not a pdf" };
    const doc = nextDoc++;
    docs.set(doc, { path, pages });
    return {
      doc,
      name: path.split("/").pop(),
      pages: pages.map(() => [PAGE.w, PAGE.h]),
      position: null,
    };
  }

  // A page drawn as white paper with a grey bar for every character, so selections and matches have
  // something to sit on, and with a blue box where the first link is.
  function renderPage(doc, page, w, h) {
    const rgba = new Uint8ClampedArray(w * h * 4).fill(255);
    const d = docs.get(doc);
    if (!d) throw { code: "unknown_document", message: "gone" };
    const { rects } = boxesFor(d.pages[page] ?? []);
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
    const { text } = boxesFor(d.pages[page] ?? []);
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
        return boxesFor(docs.get(args.doc)?.pages[args.page] ?? []);
      case "get_page_links":
        return LINKS[docs.get(args.doc)?.path]?.[args.page] ?? [];
      case "search_page":
        return search(args.doc, args.page, args.query);
      case "open_link":
        return null;
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

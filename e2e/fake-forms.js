// Forms for the browser tests, on top of the stand-in for the Rust side (fake-tauri.js, which must be
// loaded first). It adds a few made-up documents with forms and answers the commands of the form
// side of the app: the fields of a page, and changing one field.
//
//   - /docs/form.pdf: one Letter page with every kind of field (the same fields as `forms_pdf` in
//     crates/testkit, in the same places)
//   - /docs/rotated.pdf: the same page turned a quarter turn
//   - /docs/xfa.pdf: a form that only XFA can show
//   - /docs/scripts.pdf: a form with scripts and actions
//   - /docs/hostile.pdf: a form whose names, tooltips and values are made of markup
//
// The pictures are drawn from the state of the fields, so a test can see in the canvas that the
// page was drawn again after a change: a field with a value is a dark bar, a check box that is on is
// a dark square. Changes follow the rules of the real engine as far as the tests need: a radio
// button cannot be turned off, and a drop-down list cannot be cleared.
(() => {
  const base = window.__TAURI_INTERNALS__;
  const baseInvoke = base.invoke;
  const stand = window.__E2E__;
  const forms = { calls: [], saves: 0, quits: 0, failNext: null, delay: 0, emit: null };
  stand.forms = forms;

  const PAGE = { w: 612, h: 792 };
  const opt = (label, selected = false) => ({ label, selected });
  const field = (over) => ({
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
    turns: 0,
    fontSize: 12,
    tooltip: "",
    value: "",
    maxLen: 0,
    options: [],
    ...over,
  });

  function standardFields(turns = 0) {
    const turn = (rect) => {
      // A page turned a quarter turn clockwise draws the point (x, y) at (y, x) of the unturned page
      // seen from the top left; the made-up fields of this page are given as drawn.
      if (turns !== 1) return rect;
      const [l, t, r, b] = rect;
      return [t, l, b, r];
    };
    const f = (over) => field({ ...over, turns, rect: turn(over.rect) });
    return [
      f({ index: 0, name: "fullname", tooltip: "Full name", required: true, rect: [150, 68, 400, 92] }),
      f({ index: 1, name: "notes", multiline: true, fontSize: 10, rect: [150, 108, 450, 192] }),
      f({ index: 2, name: "locked", readOnly: true, value: "Fixed", rect: [150, 208, 400, 232] }),
      f({ index: 3, name: "agree", kind: "checkbox", tooltip: "I agree to the terms", rect: [150, 248, 166, 264] }),
      f({ index: 4, name: "size", kind: "radio", noToggleOff: true, tooltip: "Size", rect: [150, 288, 166, 304] }),
      f({ index: 5, name: "size", kind: "radio", noToggleOff: true, tooltip: "Size", rect: [190, 288, 206, 304] }),
      f({ index: 6, name: "size", kind: "radio", noToggleOff: true, tooltip: "Size", rect: [230, 288, 246, 304] }),
      f({ index: 7, name: "country", kind: "combo", tooltip: "Country", rect: [150, 330, 350, 352], options: [opt("Norway"), opt("Sweden"), opt("Denmark")] }),
      f({ index: 8, name: "city", kind: "combo", editable: true, tooltip: "City", rect: [150, 370, 350, 392], options: [opt("Oslo"), opt("Bergen")] }),
      f({ index: 9, name: "colours", kind: "list", tooltip: "Colours", fontSize: 11, rect: [150, 400, 300, 464], options: [opt("Red"), opt("Green"), opt("Blue"), opt("Yellow")] }),
      f({ index: 10, name: "toppings", kind: "list", multiselect: true, tooltip: "Toppings", fontSize: 11, rect: [150, 480, 300, 544], options: [opt("Ham"), opt("Cheese"), opt("Olives"), opt("Onions")] }),
    ];
  }

  const HOSTILE = '<img src=x onerror="window.pwned=1"><script>window.pwned=2</script>';

  const SPECS = {
    "/docs/form.pdf": { info: { kind: "acro", actions: false }, fields: () => standardFields() },
    "/docs/rotated.pdf": { info: { kind: "acro", actions: false }, fields: () => standardFields(1) },
    "/docs/xfa.pdf": { info: { kind: "xfa", actions: true }, fields: () => [] },
    "/docs/scripts.pdf": {
      info: { kind: "acro", actions: true },
      fields: () => standardFields().map((f) => (f.index === 0 ? { ...f, action: true } : f)),
    },
    "/docs/hostile.pdf": {
      info: { kind: "acro", actions: false },
      fields: () => [
        field({ index: 0, name: HOSTILE, tooltip: HOSTILE, value: HOSTILE, rect: [100, 100, 400, 124] }),
        field({ index: 1, name: "c", kind: "combo", rect: [100, 150, 400, 172], options: [opt(HOSTILE, true)] }),
      ],
    },
  };

  const docs = new Map(); // doc number -> { path, fields }
  let nextDoc = 500;

  function openForm(path) {
    const spec = SPECS[path];
    const doc = nextDoc++;
    docs.set(doc, { path, spec, fields: spec.fields() });
    return { doc, name: path.split("/").pop(), pages: [[PAGE.w, PAGE.h]], position: null };
  }

  /** A page as white paper with a dark bar for every field that has a value. */
  function draw(doc, w, h) {
    const rgba = new Uint8ClampedArray(w * h * 4).fill(255);
    const fill = (rect, shade) => {
      const [l, t, r, b] = rect;
      const x0 = Math.floor((l / PAGE.w) * w);
      const x1 = Math.ceil((r / PAGE.w) * w);
      const y0 = Math.floor((t / PAGE.h) * h);
      const y1 = Math.ceil((b / PAGE.h) * h);
      for (let y = y0; y < y1 && y < h; y++) {
        for (let x = x0; x < x1 && x < w; x++) {
          const o = (y * w + x) * 4;
          rgba[o] = rgba[o + 1] = rgba[o + 2] = shade;
        }
      }
    };
    for (const f of docs.get(doc).fields) {
      const on =
        f.kind === "checkbox" || f.kind === "radio"
          ? f.checked
          : f.kind === "text" || (f.kind === "combo" && f.editable)
            ? f.value !== ""
            : f.options.some((o) => o.selected);
      if (on) fill(f.rect, 40);
    }
    return rgba.buffer;
  }

  function change(doc, index, name, value) {
    const entry = docs.get(doc);
    const f = entry.fields.find((x) => x.index === index);
    const refuse = (message) => {
      throw { code: "form_field_refused", message };
    };
    if (!f || f.name !== name) refuse("not the field");
    if (f.readOnly || f.valueCut) refuse("read only");
    if (value.kind === "text") {
      if (f.kind === "combo" && value.text === "" && f.options.length > 0 && f.value !== "") refuse("cannot empty");
      f.value = value.text;
      if (f.kind === "combo") f.options.forEach((o) => (o.selected = o.label === value.text));
    } else if (value.kind === "checked") {
      if (f.kind === "radio") {
        if (!value.on) refuse("cannot turn off");
        entry.fields.forEach((x) => {
          if (x.kind === "radio" && x.name === name) x.checked = false;
        });
      }
      f.checked = value.on;
    } else {
      if (f.kind === "combo" && value.picked.length === 0 && f.options.some((o) => o.selected)) refuse("cannot clear");
      f.options.forEach((o, i) => (o.selected = value.picked.includes(i)));
    }
    return structuredClone(entry.fields);
  }

  // The callbacks of the page, so that a test can send an event the app process would send.
  const callbacks = new Map();
  const listeners = [];
  const baseTransform = base.transformCallback;
  base.transformCallback = (callback, once) => {
    const id = baseTransform.call(base, callback, once);
    callbacks.set(id, callback);
    return id;
  };
  forms.emit = (event, payload = null) => {
    for (const l of listeners) if (l.event === event) callbacks.get(l.handler)?.({ event, id: 1, payload });
  };

  base.invoke = async (cmd, args = {}) => {
    if (cmd === "plugin:event|listen") listeners.push({ event: args.event, handler: args.handler });
    if (cmd === "quit_app") {
      forms.quits += 1;
      return null;
    }
    if (cmd === "stay_open") return null;
    if (cmd === "open_pdf" && SPECS[args.path]) return openForm(args.path);
    const mine = docs.has(args.doc);
    if (mine) {
      switch (cmd) {
        case "render_page":
          return draw(args.doc, args.width, args.height);
        case "get_page_text":
          return { text: "", rects: [] };
        case "get_page_links":
        case "get_outline":
        case "missed_fonts":
          return [];
        case "close_pdf":
          docs.delete(args.doc);
          return null;
        case "set_dirty":
          forms.calls.push({ cmd, args });
          return null;
        case "save_pdf":
          forms.saves += 1;
          forms.calls.push({ cmd, args });
          return { path: docs.get(args.doc).path, name: docs.get(args.doc).path.split("/").pop() };
        case "save_pdf_as":
          forms.calls.push({ cmd, args });
          return { path: "/docs/copy.pdf", name: "copy.pdf" };
        case "form_info":
          return structuredClone(docs.get(args.doc).spec.info);
        case "get_form_fields":
          return args.page === 0 ? structuredClone(docs.get(args.doc).fields) : [];
        case "set_form_field": {
          forms.calls.push({ cmd, args });
          if (forms.delay) await new Promise((r) => setTimeout(r, forms.delay));
          if (forms.failNext) {
            const error = forms.failNext;
            forms.failNext = null;
            throw error;
          }
          return change(args.doc, args.index, args.name, args.value);
        }
        default:
      }
    }
    return baseInvoke(cmd, args);
  };
})();

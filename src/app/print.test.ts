// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  MAX_PRINT_PAGES,
  PRINT_DPI,
  PrintJob,
  parsePageRange,
  printBitmapSize,
  showPrintDialog,
  showPrintProgress,
  type PrintDeps,
} from "./print";

const letter = { width: 612, height: 792 };
const wait = (ms = 0) => new Promise((r) => setTimeout(r, ms));

describe("parsePageRange", () => {
  const ok = (text: string, count = 20) => {
    const r = parsePageRange(text, count);
    if ("error" in r) throw new Error(r.error);
    return r.pages;
  };
  const bad = (text: string, count = 20) => {
    const r = parsePageRange(text, count);
    expect("error" in r).toBe(true);
    return "error" in r ? r.error : "";
  };

  it("reads pages and ranges, 1-based, into indexes from 0", () => {
    expect(ok("1")).toEqual([0]);
    expect(ok("1-3")).toEqual([0, 1, 2]);
    expect(ok("1-3, 7, 9-11")).toEqual([0, 1, 2, 6, 8, 9, 10]);
    expect(ok("  4 ; 2  ,3")).toEqual([1, 2, 3]);
  });

  it("takes an open end as the first or last page", () => {
    expect(ok("18-")).toEqual([17, 18, 19]);
    expect(ok("-3")).toEqual([0, 1, 2]);
    expect(bad("-")).toContain("not a page");
  });

  it("lists each page once, in order", () => {
    expect(ok("3, 1-4, 2")).toEqual([0, 1, 2, 3]);
  });

  it("explains what is wrong", () => {
    expect(bad("")).toContain("Type the pages");
    expect(bad("abc")).toContain("not a page");
    expect(bad("1-2-3")).toContain("not a page");
    expect(bad("0")).toContain("from 1");
    expect(bad("21")).toContain("20 pages");
    expect(bad("5-30")).toContain("20 pages");
    expect(bad("5-2")).toContain("backwards");
    expect(bad("2", 1)).toContain("1 page.");
    expect(bad("1.5")).toContain("not a page");
    expect(bad("1e3")).toContain("not a page");
  });

  it("limits the size of a job", () => {
    expect(ok(`1-${MAX_PRINT_PAGES}`, 5000)).toHaveLength(MAX_PRINT_PAGES);
    expect(bad(`1-${MAX_PRINT_PAGES + 1}`, 5000)).toContain(String(MAX_PRINT_PAGES));
  });
});

describe("printBitmapSize", () => {
  it("draws a Letter page at the print resolution", () => {
    expect(printBitmapSize(letter)).toEqual({
      width: Math.round((612 / 72) * PRINT_DPI),
      height: Math.round((792 / 72) * PRINT_DPI),
    });
  });

  it("keeps a huge page within the limits and keeps its shape", () => {
    const huge = printBitmapSize({ width: 14400, height: 14400 });
    expect(huge.width * huge.height).toBeLessThanOrEqual(20_000_000);
    expect(Math.max(huge.width, huge.height)).toBeLessThanOrEqual(6000);
    const poster = printBitmapSize({ width: 200, height: 14400 });
    expect(poster.height).toBeLessThanOrEqual(6000);
    expect(poster.height / poster.width).toBeCloseTo(72, -1);
    expect(printBitmapSize({ width: 0, height: 0 })).toEqual({ width: 1, height: 1 });
  });
});

describe("PrintJob", () => {
  let root: HTMLElement;
  let urls: string[];

  beforeEach(() => {
    document.body.innerHTML = '<div id="print-root"></div>';
    root = document.getElementById("print-root") as HTMLElement;
    urls = [];
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    document.body.replaceChildren();
  });

  function make(overrides: { render?: PrintDeps["render"]; print?: PrintDeps["print"] } = {}) {
    let n = 0;
    const deps = {
      render: vi.fn(
        overrides.render ??
          ((_d: number, _p: number, w: number, h: number) => Promise.resolve(new ArrayBuffer(w * h * 4))),
      ),
      encode: vi.fn((_rgba: ArrayBuffer, _w: number, _h: number) => Promise.resolve(new Blob(["x"]))),
      createUrl: vi.fn((_blob: Blob) => {
        const url = `blob:test/${n++}`;
        urls.push(url);
        return url;
      }),
      revokeUrl: vi.fn((_url: string) => undefined),
      print: vi.fn(overrides.print ?? (() => Promise.resolve())),
    };
    return { deps, job: new PrintJob(root, deps) };
  }

  const pages = [letter, letter, { width: 595, height: 842 }];

  it("prepares the pages in order, shows them as pictures and opens the print dialog", async () => {
    const { deps, job } = make();
    const progress = vi.fn();
    const outcome = await job.run(9, pages, [0, 2], progress);
    expect(outcome).toBe("printing");
    expect(deps.render.mock.calls.map((c) => [c[0], c[1]])).toEqual([[9, 0], [9, 2]]);
    // The second page is A4, so it is drawn at its own size.
    expect(deps.render.mock.calls[1].slice(2)).toEqual([
      Math.round((595 / 72) * PRINT_DPI),
      Math.round((842 / 72) * PRINT_DPI),
    ]);
    const images = [...root.querySelectorAll("img")];
    expect(images.map((i) => i.getAttribute("src"))).toEqual(urls);
    expect(images.map((i) => i.alt)).toEqual(["Page 1", "Page 3"]);
    expect(images.every((i) => i.className === "print-page")).toBe(true);
    expect(progress.mock.calls.map((c) => c[0].done)).toEqual([0, 1, 2]);
    expect(deps.print).toHaveBeenCalledTimes(1);
  });

  it("keeps the pictures while printing and drops them when printing is over", async () => {
    const { deps, job } = make();
    await job.run(1, pages, [0, 1], vi.fn());
    expect(root.querySelectorAll("img")).toHaveLength(2);
    expect(deps.revokeUrl).not.toHaveBeenCalled();
    window.dispatchEvent(new Event("afterprint"));
    expect(root.children.length).toBe(0);
    expect(deps.revokeUrl.mock.calls.map((c) => c[0])).toEqual(urls);
  });

  it("starts clean: pictures of an earlier job do not stay", async () => {
    const { deps, job } = make();
    await job.run(1, pages, [0], vi.fn());
    await job.run(1, pages, [1, 2], vi.fn());
    expect(root.querySelectorAll("img")).toHaveLength(2);
    expect(deps.revokeUrl).toHaveBeenCalledTimes(1);
  });

  it("stops when cancelled, and nothing is printed or kept", async () => {
    let jobRef: PrintJob | null = null;
    const { deps, job } = make({
      render: vi.fn(async (_d, page, w, h) => {
        if (page === 1) jobRef?.cancel();
        return new ArrayBuffer(w * h * 4);
      }),
    });
    jobRef = job;
    const outcome = await job.run(1, pages, [0, 1, 2], vi.fn());
    expect(outcome).toBe("cancelled");
    expect(deps.print).not.toHaveBeenCalled();
    expect(root.children.length).toBe(0);
    expect(deps.revokeUrl).toHaveBeenCalledTimes(urls.length);
    // And the job can run again afterwards.
    expect(await job.run(1, pages, [0], vi.fn())).toBe("printing");
  });

  it("fails cleanly when a page cannot be drawn, and prints nothing", async () => {
    const { deps, job } = make({
      render: vi.fn(async (_d, page, w, h) => {
        if (page === 1) throw { code: "engine_stopped", message: "it exited" };
        return new ArrayBuffer(w * h * 4);
      }),
    });
    await expect(job.run(1, pages, [0, 1], vi.fn())).rejects.toMatchObject({ code: "engine_stopped" });
    expect(deps.print).not.toHaveBeenCalled();
    expect(root.children.length).toBe(0);
    expect(deps.revokeUrl).toHaveBeenCalledTimes(1);
  });

  it("refuses a bitmap of the wrong size", async () => {
    const { deps, job } = make({ render: vi.fn(() => Promise.resolve(new ArrayBuffer(10))) });
    await expect(job.run(1, pages, [0], vi.fn())).rejects.toThrow("wrong size");
    expect(deps.print).not.toHaveBeenCalled();
  });

  it("cleans up when the print dialog cannot be opened", async () => {
    const { job } = make({ print: vi.fn(() => Promise.reject(new Error("no printer"))) });
    await expect(job.run(1, pages, [0], vi.fn())).rejects.toThrow("no printer");
    expect(root.children.length).toBe(0);
  });
});

describe("the print dialog", () => {
  let layer: HTMLElement;
  const click = (el: Element | null) => (el as HTMLElement).click();

  beforeEach(() => {
    document.body.innerHTML = '<div id="layer" hidden></div>';
    layer = document.getElementById("layer") as HTMLElement;
  });

  afterEach(() => document.body.replaceChildren());

  const open = () => showPrintDialog(layer, { name: "book.pdf", pageCount: 12, currentPage: 4 });
  const submit = () => layer.querySelector("form")?.dispatchEvent(new Event("submit", { cancelable: true }));
  const radio = (value: string) => layer.querySelector<HTMLInputElement>(`input[value="${value}"]`) as HTMLInputElement;

  it("prints all pages by default", async () => {
    const answer = open();
    expect(layer.hidden).toBe(false);
    expect(layer.textContent).toContain("book.pdf has 12 pages");
    submit();
    expect(await answer).toEqual({ pages: Array.from({ length: 12 }, (_, i) => i) });
    expect(layer.hidden).toBe(true);
  });

  it("prints the page in view", async () => {
    const answer = open();
    radio("current").checked = true;
    radio("current").dispatchEvent(new Event("change"));
    submit();
    expect(await answer).toEqual({ pages: [4] });
  });

  it("prints a range, and says what is wrong without closing", async () => {
    const answer = open();
    radio("range").checked = true;
    radio("range").dispatchEvent(new Event("change"));
    const input = layer.querySelector<HTMLInputElement>('input[type="text"]') as HTMLInputElement;
    expect(input.disabled).toBe(false);
    input.value = "3-99";
    submit();
    expect(layer.textContent).toContain("12 pages");
    expect(layer.querySelector("form")).not.toBeNull();
    input.value = "2-3, 12";
    submit();
    expect(await answer).toEqual({ pages: [1, 2, 11] });
  });

  it("keeps the range box off unless Pages is chosen", () => {
    void open();
    const input = layer.querySelector<HTMLInputElement>('input[type="text"]') as HTMLInputElement;
    expect(input.disabled).toBe(true);
  });

  it("closes with Cancel and with Escape and prints nothing", async () => {
    const first = open();
    click(layer.querySelector(".btn-secondary"));
    expect(await first).toBeNull();
    const second = open();
    layer.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(await second).toBeNull();
    expect(layer.hidden).toBe(true);
  });
});

describe("the progress box", () => {
  it("shows how far the preparing is, and cancels", async () => {
    document.body.innerHTML = '<div id="layer" hidden></div>';
    const layer = document.getElementById("layer") as HTMLElement;
    const onCancel = vi.fn();
    const box = showPrintProgress(layer, onCancel);
    expect(layer.hidden).toBe(false);
    box.update({ done: 2, total: 10 });
    expect(layer.textContent).toContain("Page 3 of 10");
    expect(layer.querySelector("progress")?.value).toBe(2);
    click(layer.querySelector("button"));
    expect(onCancel).toHaveBeenCalledTimes(1);
    layer.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(onCancel).toHaveBeenCalledTimes(2);
    box.close();
    expect(layer.hidden).toBe(true);
    await wait();

    function click(el: Element | null) {
      (el as HTMLElement).click();
    }
  });
});

describe("the print dialog for a very long document", () => {
  let layer: HTMLElement;
  beforeEach(() => {
    document.body.innerHTML = '<div id="layer" hidden></div>';
    layer = document.getElementById("layer") as HTMLElement;
  });
  afterEach(() => document.body.replaceChildren());
  const submit = () => layer.querySelector("form")?.dispatchEvent(new Event("submit", { cancelable: true }));

  it("does not offer all pages when there are more than a job can hold, and says what will be printed", async () => {
    const answer = showPrintDialog(layer, { name: "big.pdf", pageCount: 5000, currentPage: 0 });
    expect(layer.textContent).not.toContain("All 5000 pages");
    expect(layer.textContent).toContain("The first 1000 of 5000 pages");
    expect(layer.textContent).toContain("at most 1000 pages");
    submit();
    const choice = await answer;
    expect(choice?.pages).toHaveLength(MAX_PRINT_PAGES);
    expect(choice?.pages.at(-1)).toBe(MAX_PRINT_PAGES - 1);
  });

  it("keeps the plain wording for a document that fits", () => {
    void showPrintDialog(layer, { name: "small.pdf", pageCount: 12, currentPage: 0 });
    expect(layer.textContent).toContain("All 12 pages");
    expect(layer.textContent).not.toContain("at most");
  });
});

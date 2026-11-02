import { expect, test, type Page } from "@playwright/test";

// The annotation bar and the annotations on the pages, in a real browser with the policy of the app.
// The Rust side is the stand-in of e2e/fake-tauri.js, which keeps annotations the way the engine
// does (it finds the boxes of marked text with a copy of the engine's rule) and draws them into the
// pages it serves, so that what is seen on the canvas is what the window asked for.

type Call = { cmd: string; args: Record<string, unknown> };
type Stand = {
  calls: Call[];
  dialog: string[];
  violations: string[];
  title: string;
  dirty: Record<string, boolean>;
  saves: { doc: number }[];
  quits: number;
  stays: number;
  annotFailure: { cmd: string; error: { code: string; message: string }; once?: boolean } | null;
  update: { found: unknown };
  emit(name: string, payload: unknown): void;
};

declare global {
  interface Window {
    __E2E__: Stand;
  }
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ path: "e2e/fake-tauri.js" });
  await page.goto("/");
});

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(() => window.__E2E__.violations);
  expect(violations, "the content security policy blocked something").toEqual([]);
});

const calls = (page: Page, cmd: string) =>
  page.evaluate((name) => window.__E2E__.calls.filter((c) => c.cmd === name), cmd);

async function openFile(page: Page, path: string, name: string) {
  await page.evaluate((p) => (window.__E2E__.dialog = [p]), path);
  await page.locator("#open-button").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText(name);
  // The first page is drawn and the list of its annotations has arrived.
  await expect(page.locator(".viewer-page canvas").first()).toBeAttached();
  await expect.poll(() => calls(page, "list_annotations").then((c) => c.length)).toBeGreaterThan(0);
}

const openBook = async (page: Page) => {
  await page.locator("#empty-open").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("book.pdf");
  await expect.poll(() => calls(page, "list_annotations").then((c) => c.length)).toBeGreaterThan(0);
};

async function openBar(page: Page) {
  await page.locator("#annotate-button").click();
  await expect(page.locator("#annotbar")).toBeVisible();
}

/** A point on a page, given in points from its top left, as a position on the screen. */
async function onPage(page: Page, index: number, x: number, y: number, size = { w: 612, h: 792 }) {
  const box = await page.locator(`.viewer-page[data-page="${index}"]`).boundingBox();
  if (!box) throw new Error("the page is not on screen");
  return { x: box.x + (x / size.w) * box.width, y: box.y + (y / size.h) * box.height };
}

async function drag(page: Page, from: { x: number; y: number }, to: { x: number; y: number }) {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 4 });
  await page.mouse.move(to.x, to.y, { steps: 4 });
  await page.mouse.up();
}

/** The colour of the picture of a page at a point given in points from its top left. */
async function pixelAt(page: Page, index: number, x: number, y: number, width = 612) {
  return page.evaluate(
    ({ index, x, y, width }) => {
      const canvas = document.querySelector<HTMLCanvasElement>(`.viewer-page[data-page="${index}"] canvas`);
      if (!canvas) throw new Error("no canvas");
      const k = canvas.width / width;
      const d = canvas.getContext("2d")?.getImageData(Math.round(x * k), Math.round(y * k), 1, 1).data;
      return d ? [d[0], d[1], d[2]] : [0, 0, 0];
    },
    { index, x, y, width },
  );
}

const isYellow = (c: number[]) => c[0] > 235 && c[1] > 210 && c[2] < 110;
const isWhite = (c: number[]) => c.every((v) => v > 240);

test("the bar opens, every control has a name, and the arrow keys move along it", async ({ page }) => {
  await openBook(page);
  const bar = page.locator("#annotbar");
  await expect(bar).toBeHidden();
  await openBar(page);
  await expect(bar).toHaveAttribute("role", "toolbar");
  await expect(bar).toHaveAccessibleName("Annotations");
  await expect(page.locator("#annotate-button")).toHaveAttribute("aria-pressed", "true");

  const names = ["Select", "Highlight", "Underline", "Strike out", "Note", "Pen", "Stamp"];
  for (const name of names) await expect(bar.getByRole("button", { name, exact: true })).toBeVisible();
  await expect(bar.getByRole("button", { name: "Select", exact: true })).toHaveAttribute("aria-pressed", "true");
  // The colours are a group of radio buttons with names.
  const colours = bar.getByRole("radiogroup", { name: "Colour" }).getByRole("radio");
  await expect(colours).toHaveCount(6);
  await expect(colours.nth(0)).toHaveAccessibleName("Yellow");
  await expect(colours.nth(0)).toHaveAttribute("aria-checked", "true");
  await colours.nth(2).click();
  await expect(colours.nth(2)).toHaveAttribute("aria-checked", "true");
  await expect(colours.nth(0)).toHaveAttribute("aria-checked", "false");
  // Delete, undo and redo have names, and are off while there is nothing to do.
  await expect(bar.getByRole("button", { name: "Undo" })).toBeDisabled();
  await expect(bar.getByRole("button", { name: /Delete the selected annotation/ })).toBeDisabled();

  // One stop in the Tab order, and the arrow keys go along the buttons.
  const stops = await bar.locator("button[tabindex='0']").count();
  expect(stops).toBe(1);
  await bar.getByRole("button", { name: "Select", exact: true }).focus();
  await page.keyboard.press("ArrowRight");
  await expect(bar.getByRole("button", { name: "Highlight", exact: true })).toBeFocused();
  await page.keyboard.press("End");
  await expect(bar.getByRole("button", { name: /Delete the selected/ })).not.toBeFocused();
  await page.keyboard.press("Home");
  await expect(bar.getByRole("button", { name: "Select", exact: true })).toBeFocused();
  await page.keyboard.press("ArrowLeft");
  // It wraps round to the last button that can be reached.
  await expect(bar.locator("button:focus")).toHaveCount(1);
  expect(await bar.locator("button[tabindex='0']").count()).toBe(1);

  // A plain letter picks a tool when the document has the focus.
  await page.locator(".viewer").focus();
  await page.keyboard.press("h");
  await expect(bar.getByRole("button", { name: "Highlight", exact: true })).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("p");
  await expect(bar.getByRole("button", { name: "Pen", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect(bar.getByRole("group", { name: "Line width" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(bar.getByRole("button", { name: "Select", exact: true })).toHaveAttribute("aria-pressed", "true");
  await page.screenshot({ path: "e2e/artifacts/annotation-bar.png" });
});

test("selected text is highlighted, drawn, and taken back and made again with undo and redo", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator("#tool-highlight").click();
  // "quick" in the second line: characters 18 to 22 of the page text.
  await drag(page, await onPage(page, 0, 84.5, 123), await onPage(page, 0, 113.5, 123));
  await expect.poll(() => calls(page, "add_markup").then((c) => c.length)).toBe(1);
  const [markup] = await calls(page, "add_markup");
  expect(markup.args).toMatchObject({ page: 0, kind: "highlight", color: [255, 235, 59, 255], start: 18, end: 23 });

  // The page shows it (the top row of the characters' boxes is paper in the stand-in's picture).
  await expect.poll(async () => isYellow(await pixelAt(page, 0, 100, 118.5))).toBe(true);
  // The text is not selected any more, and the document has a change that is not saved.
  await expect(page.locator(".hl-select")).toHaveCount(0);
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
  await expect.poll(() => page.evaluate(() => window.__E2E__.title)).toMatch(/^• book\.pdf/);
  await expect.poll(() => page.evaluate(() => Object.values(window.__E2E__.dirty)[0])).toBe(true);
  // The annotation is on the page as a box a screen reader can reach.
  await expect(page.locator(".annot")).toHaveCount(1);
  await expect(page.locator(".annot")).toHaveAttribute("aria-label", "Highlight, page 1");

  await expect(page.getByRole("button", { name: "Undo highlight" })).toBeEnabled();
  await page.keyboard.press("Control+z");
  await expect.poll(async () => isWhite(await pixelAt(page, 0, 100, 118.5))).toBe(true);
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
  await expect(page.locator(".annot")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => Object.values(window.__E2E__.dirty)[0])).toBe(false);
  await page.keyboard.press("Control+y");
  await expect.poll(async () => isYellow(await pixelAt(page, 0, 100, 118.5))).toBe(true);
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
  // Redo restored the same annotation, it did not make another.
  expect((await calls(page, "add_markup")).length).toBe(1);
  expect((await calls(page, "restore_annotation")).length).toBe(1);
});

test("a selection can also be marked after it was made, and a colour is kept", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator("#annotbar").getByRole("radio", { name: "Green" }).click();
  // Select first (no tool): then pick the tool, which marks what is selected.
  await drag(page, await onPage(page, 0, 84.5, 123), await onPage(page, 0, 113.5, 123));
  await expect(page.locator(".hl-select").first()).toBeAttached();
  await page.locator("#tool-underline").click();
  await expect.poll(() => calls(page, "add_markup").then((c) => c.length)).toBe(1);
  const [markup] = await calls(page, "add_markup");
  expect(markup.args).toMatchObject({ kind: "underline", color: [126, 217, 87, 255], start: 18, end: 23 });
  await expect(page.locator(".annot")).toHaveAttribute("aria-label", "Underline, page 1");
});

test("right-to-left text is marked over exactly the letters that were selected", async ({ page }) => {
  await openFile(page, "/docs/rtl.pdf", "rtl.pdf");
  await openBar(page);
  await page.locator("#tool-highlight").click();
  // Letters 3 to 6 in reading order sit from x = 387 down to x = 337, the first letter at the right.
  await drag(page, await onPage(page, 0, 386, 85), await onPage(page, 0, 337.5, 85));
  await expect.poll(() => calls(page, "add_markup").then((c) => c.length)).toBe(1);
  const [markup] = await calls(page, "add_markup");
  expect(markup.args).toMatchObject({ page: 0, start: 3, end: 7 });
  // One box over those letters: the engine's rule, as the stand-in copies it, found a single line.
  const [box] = await page.evaluate(() => {
    const el = document.querySelector<HTMLElement>(".annot");
    return el ? [{ left: el.style.left, width: el.style.width }] : [];
  });
  expect(parseFloat(box.left)).toBeCloseTo((337 / 612) * 100, 0);
  expect(parseFloat(box.width)).toBeCloseTo((50 / 612) * 100, 0);
  await expect.poll(async () => isYellow(await pixelAt(page, 0, 350, 75.5))).toBe(true);
  // The letter next to it, outside the selection, is not shaded.
  expect(isWhite(await pixelAt(page, 0, 395, 75.5))).toBe(true);
  expect(isWhite(await pixelAt(page, 0, 330, 75.5))).toBe(true);
});

for (const zoom of ["0.5", "1", "2"]) {
  test(`the pen draws where the pointer goes at ${Number(zoom) * 100} percent, on a landscape page`, async ({ page }) => {
    await openFile(page, "/docs/turned.pdf", "turned.pdf");
    await openBar(page);
    await page.locator("#zoom-select").selectOption(zoom);
    await page.locator("#tool-pen").click();
    await page.locator(".viewer").focus();
    const size = { w: 792, h: 612 };
    const from = await onPage(page, 0, 100, 100, size);
    const to = await onPage(page, 0, 300, 200, size);
    const view = page.viewportSize() ?? { width: 1100, height: 760 };
    expect(to.x).toBeLessThan(view.width);
    expect(to.y).toBeLessThan(view.height);
    await drag(page, from, to);
    await expect.poll(() => calls(page, "add_ink").then((c) => c.length)).toBe(1);
    const [ink] = await calls(page, "add_ink");
    const strokes = ink.args.strokes as [number, number][][];
    expect(strokes).toHaveLength(1);
    const tolerance = 3.5 / Number(zoom);
    const first = strokes[0][0];
    const last = strokes[0][strokes[0].length - 1];
    expect(Math.abs(first[0] - 100)).toBeLessThan(tolerance);
    expect(Math.abs(first[1] - 100)).toBeLessThan(tolerance);
    expect(Math.abs(last[0] - 300)).toBeLessThan(tolerance);
    expect(Math.abs(last[1] - 200)).toBeLessThan(tolerance);
    expect(ink.args.width).toBe(3);
    // The line is on the page, in the colour of the bar.
    await expect(page.locator(".annot")).toHaveAttribute("aria-label", "Drawing, page 1");
    await expect.poll(async () => (await pixelAt(page, 0, 200, 150, 792)).reduce((a, b) => a + b, 0)).toBeLessThan(300);
  });
}

test("a click with the note tool opens a box for the text, which stays text", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.keyboard.press("Escape");
  await page.locator(".viewer").focus();
  await page.keyboard.press("n");
  await expect(page.locator("#tool-note")).toHaveAttribute("aria-pressed", "true");
  const at = await onPage(page, 0, 300, 400);
  await page.mouse.click(at.x, at.y);
  const editor = page.getByRole("dialog", { name: "New note" });
  await expect(editor).toBeVisible();
  const text = "Hello <b>world</b> & <img src=x onerror=alert(1)>";
  await editor.getByRole("textbox", { name: "Text of the note" }).fill(text);
  await page.keyboard.press("Control+Enter");
  await expect(editor).toHaveCount(0);
  await expect.poll(() => calls(page, "add_note").then((c) => c.length)).toBe(1);
  const [note] = await calls(page, "add_note");
  expect(note.args).toMatchObject({ page: 0, text });
  expect(Math.abs((note.args.x as number) - 290)).toBeLessThan(3);
  expect(Math.abs((note.args.y as number) - 390)).toBeLessThan(3);
  // The note is picked, the pointer is back to picking, and the text is text everywhere.
  await expect(page.locator("#tool-select")).toHaveAttribute("aria-pressed", "true");
  const box = page.locator(".annot");
  await expect(box).toHaveAttribute("aria-pressed", "true");
  await expect(box).toHaveAttribute("aria-label", `Note, page 1: ${text}`);
  await expect(page.locator(".annot-layer b, .annot-layer img, .annot-popovers img")).toHaveCount(0);
  await expect(page.locator(".tab-dirty")).toHaveCount(1);

  // Delete takes it off, and Undo puts it back.
  await page.keyboard.press("Delete");
  await expect(page.locator(".annot")).toHaveCount(0);
  await page.keyboard.press("Control+z");
  await expect(page.locator(".annot")).toHaveCount(1);
  await page.keyboard.press("Control+z");
  await expect(page.locator(".annot")).toHaveCount(0);
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
});

test("Escape leaves a note box without making a note, and the keyboard can place one", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator(".viewer").focus();
  await page.keyboard.press("n");
  const at = await onPage(page, 0, 200, 200);
  await page.mouse.click(at.x, at.y);
  await expect(page.getByRole("dialog", { name: "New note" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "New note" })).toHaveCount(0);
  expect((await calls(page, "add_note")).length).toBe(0);
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
  // The tool is still the note tool: Enter places a note in the middle of what is shown.
  await page.locator(".viewer").focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("dialog", { name: "New note" })).toBeVisible();
  await page.keyboard.type("from the keyboard");
  await page.getByRole("button", { name: "Save" }).click();
  await expect.poll(() => calls(page, "add_note").then((c) => c.length)).toBe(1);
  const [note] = await calls(page, "add_note");
  expect(note.args.text).toBe("from the keyboard");
});

test("a stamp is chosen from a menu and placed with a click", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator("#tool-stamp").click();
  await expect(page.getByRole("menu", { name: "Stamps" })).toBeVisible();
  await page.getByRole("menuitemradio", { name: "Confidential" }).click();
  await expect(page.locator("#tool-stamp")).toHaveAttribute("aria-pressed", "true");
  const at = await onPage(page, 0, 300, 300);
  await page.mouse.click(at.x, at.y);
  await expect.poll(() => calls(page, "add_stamp").then((c) => c.length)).toBe(1);
  const [stamp] = await calls(page, "add_stamp");
  expect(stamp.args).toMatchObject({ page: 0, stamp: "confidential" });
  // Centred on the click, within a point or two.
  const x = stamp.args.x as number;
  expect(x).toBeGreaterThan(300 - 130);
  expect(x).toBeLessThan(300 - 70);
  await expect(page.locator(".annot")).toHaveAttribute("aria-label", "Stamp, page 1: CONFIDENTIAL");
  await expect.poll(async () => isWhite(await pixelAt(page, 0, 300, 300))).toBe(false);
});

test("annotations a file already has are shown, can be changed, and removed with a warning", async ({ page }) => {
  await openFile(page, "/docs/annotated.pdf", "annotated.pdf");
  const boxes = page.locator(".annot");
  await expect(boxes).toHaveCount(4);
  await expect(boxes.nth(0)).toHaveAttribute("aria-label", "Highlight by Alice, page 1: Check this title");
  // Text from the file reaches the window as text only: nothing in it was made into an element.
  await expect(boxes.nth(1)).toHaveAttribute("aria-label", /^Note by <b>Bob<\/b>, page 1: <img src=x onerror=alert\(1\)> a note from the file$/);
  await expect(page.locator(".annot-layer b, .annot-layer img")).toHaveCount(0);
  await expect(page.locator(".tab-dirty")).toHaveCount(0);

  // Pick the highlight and change its comment with the keyboard.
  await boxes.nth(0).focus();
  await expect(boxes.nth(0)).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("Enter");
  const editor = page.getByRole("dialog", { name: "Comment" });
  await expect(editor.getByRole("textbox")).toHaveValue("Check this title");
  await editor.getByRole("textbox").fill("Checked");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => calls(page, "update_annotation").then((c) => c.length)).toBe(1);
  expect((await calls(page, "update_annotation"))[0].args).toMatchObject({ page: 0, text: "Checked", color: null });
  await expect(boxes.nth(0)).toHaveAttribute("aria-label", "Highlight by Alice, page 1: Checked");
  await expect(page.locator(".tab-dirty")).toHaveCount(1);

  // A drawing from a file cannot be brought back: deleting it asks.
  await boxes.nth(2).focus();
  await page.keyboard.press("Delete");
  const question = page.getByRole("alertdialog");
  await expect(question).toContainText("Delete this annotation?");
  await question.getByRole("button", { name: "Cancel" }).click();
  expect((await calls(page, "remove_annotation")).length).toBe(0);
  await boxes.nth(2).focus();
  await page.keyboard.press("Delete");
  await page.getByRole("alertdialog").getByRole("button", { name: "Delete" }).click();
  await expect.poll(() => calls(page, "remove_annotation").then((c) => c.length)).toBe(1);
  await expect(page.locator(".annot")).toHaveCount(3);
  // Undo says plainly that it cannot, and the document stays as it is.
  await page.locator(".viewer").focus();
  await page.keyboard.press("Control+z");
  await expect(page.getByRole("alertdialog")).toContainText("The change could not be taken back");
  await page.getByRole("button", { name: "OK" }).click();
  await expect(page.locator(".annot")).toHaveCount(3);
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
});

test("a note can be moved with the arrow keys and with the pointer, and put back with undo", async ({ page }) => {
  await openFile(page, "/docs/annotated.pdf", "annotated.pdf");
  const note = page.locator(".annot[data-kind='note']");
  await note.focus();
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Shift+ArrowDown");
  await expect.poll(() => calls(page, "update_annotation").then((c) => c.length)).toBe(2);
  const moves = (await calls(page, "update_annotation")).map((c) => [c.args.dx, c.args.dy]);
  expect(moves).toEqual([
    [4, 0],
    [0, 24],
  ]);
  // Drag by 60 pixels to the right.
  const box = await note.boundingBox();
  if (!box) throw new Error("no box");
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + 30, box.y + box.height / 2, { steps: 3 });
  await page.mouse.move(box.x + box.width / 2 + 60, box.y + box.height / 2, { steps: 3 });
  await page.mouse.up();
  await expect.poll(() => calls(page, "update_annotation").then((c) => c.length)).toBe(3);
  const drag3 = (await calls(page, "update_annotation"))[2].args;
  expect(drag3.dx as number).toBeGreaterThan(30);
  expect(drag3.dy as number).toBeCloseTo(0, 0);
  await page.locator(".viewer").focus();
  await page.keyboard.press("Control+z");
  await expect.poll(() => calls(page, "update_annotation").then((c) => c.length)).toBe(4);
  expect((await calls(page, "update_annotation"))[3].args.dx).toBeCloseTo(-(drag3.dx as number), 3);
});

test("a change with unsaved annotations is asked about when the tab is closed, when the app quits and when an update installs", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator("#tool-highlight").click();
  await drag(page, await onPage(page, 0, 84.5, 123), await onPage(page, 0, 113.5, 123));
  await expect(page.locator(".tab-dirty")).toHaveCount(1);

  // Closing the tab asks, and Cancel keeps the tab and the change.
  await page.getByRole("button", { name: "Close book.pdf" }).click();
  const closing = page.getByRole("alertdialog");
  await expect(closing).toContainText('Save changes to "book.pdf"?');
  await closing.getByRole("button", { name: "Cancel" }).click();
  await expect(page.locator(".tab-title")).toHaveCount(1);
  await expect(page.locator(".tab-dirty")).toHaveCount(1);

  // The app asks the window before it quits (the close button of the window, Cmd+Q).
  await page.evaluate(() => window.__E2E__.emit("quit-requested", null));
  const quitting = page.getByRole("alertdialog");
  await expect(quitting).toContainText('Save changes to "book.pdf"?');
  await quitting.getByRole("button", { name: "Cancel" }).click();
  await expect.poll(() => page.evaluate(() => window.__E2E__.stays)).toBe(1);
  expect(await page.evaluate(() => window.__E2E__.quits)).toBe(0);

  // An update does not install over the change: it offers to save first, or to wait.
  await page.evaluate(() => {
    window.__E2E__.update.found = { version: "0.2.0", currentVersion: "0.1.0", notes: "x", method: "automatic" };
  });
  await page.locator("#menu-button").click();
  await page.getByRole("menuitem", { name: "Check for Updates…" }).click();
  await page.getByRole("button", { name: "Download and Install" }).click();
  const update = page.getByRole("alertdialog");
  await expect(update).toContainText("Save your changes first");
  await update.getByRole("button", { name: "Install later" }).click();
  expect((await calls(page, "install_update")).length).toBe(0);

  // Quit without saving ends the app; Save and quit saves first.
  await page.evaluate(() => window.__E2E__.emit("quit-requested", null));
  await page.getByRole("alertdialog").getByRole("button", { name: "Save and quit" }).click();
  await expect.poll(() => page.evaluate(() => window.__E2E__.quits)).toBe(1);
  expect(await page.evaluate(() => window.__E2E__.saves.length)).toBe(1);
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
});

test("Ctrl+S saves and clears the mark, and undo after a save brings the mark back", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator("#tool-highlight").click();
  await drag(page, await onPage(page, 0, 84.5, 123), await onPage(page, 0, 113.5, 123));
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
  await page.keyboard.press("Control+s");
  await expect.poll(() => page.evaluate(() => window.__E2E__.saves.length)).toBe(1);
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => window.__E2E__.title)).toBe("book.pdf - Patient Partridge");
  await page.keyboard.press("Control+z");
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
  await page.keyboard.press("Control+y");
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
});

test("a change that fails says so, makes no mark, and leaves the document as it was", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.evaluate(() => {
    window.__E2E__.annotFailure = { cmd: "add_markup", error: { code: "annotation_refused", message: "no" }, once: true };
  });
  await page.locator("#tool-highlight").click();
  await drag(page, await onPage(page, 0, 84.5, 123), await onPage(page, 0, 113.5, 123));
  const sheet = page.getByRole("alertdialog");
  await expect(sheet).toContainText("The annotation was not changed");
  await sheet.getByRole("button", { name: "OK" }).click();
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
  await expect(page.locator(".annot")).toHaveCount(0);
  // The next try works.
  await drag(page, await onPage(page, 0, 84.5, 123), await onPage(page, 0, 113.5, 123));
  await expect(page.locator(".annot")).toHaveCount(1);
});

test("toolbar buttons, the stamp menu and the note box work in the dark theme and at a narrow width", async ({ page }) => {
  await page.setViewportSize({ width: 560, height: 700 });
  await openBook(page);
  await openBar(page);
  await page.locator("#menu-button").click();
  await page.locator('#theme-choice label:has-text("Dark")').click();
  await page.keyboard.press("Escape");
  await page.screenshot({ path: "e2e/artifacts/annotation-bar-dark-narrow.png" });
  const bar = await page.locator("#annotbar").boundingBox();
  expect(bar?.width ?? 0).toBeLessThanOrEqual(560);
});

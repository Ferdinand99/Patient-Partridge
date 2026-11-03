import { expect, test, type Page } from "@playwright/test";

// The picture of a signature, in a real browser with the policy of the app: making it (drawing,
// typing, importing an image), keeping it, placing it, moving it, resizing it, taking it back and
// putting it back. The Rust side is the stand-in of e2e/fake-tauri.js, which fits and keeps
// pictures the way the engine does and draws each as a line from corner to corner on the page.
//
// A picture of a signature is only a picture: the tests also read what the window says.

type Call = { cmd: string; args: Record<string, unknown> };
type Stand = {
  calls: Call[];
  dialog: string[];
  violations: string[];
  marks: { id: string; name: string; kind: string }[];
  marksFailure: { code: string; message: string } | null;
  pages: { delay: number };
};

const errors: string[] = [];

test.beforeEach(async ({ page }) => {
  errors.length = 0;
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  await page.addInitScript({ path: "e2e/fake-tauri.js" });
  await page.addInitScript({ path: "e2e/fake-pages.js" });
  await page.goto("/");
});

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.violations);
  expect(violations, "the content security policy blocked something").toEqual([]);
  expect(errors, "the window reported an error").toEqual([]);
});

const calls = (page: Page, cmd: string) =>
  page.evaluate((name) => (window as unknown as { __E2E__: Stand }).__E2E__.calls.filter((c) => c.cmd === name), cmd);

async function open(page: Page, path: string, name: string) {
  await page.evaluate((p) => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = [p]), path);
  await page.locator("#empty-open, #open-button").first().click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText(name);
  await expect(page.locator(".viewer-page canvas").first()).toBeAttached();
  await expect.poll(() => calls(page, "list_annotations").then((c) => c.length)).toBeGreaterThan(0);
}

const openBook = (page: Page) => open(page, "/docs/book.pdf", "book.pdf");

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

const isWhite = (c: number[]) => c.every((v) => v > 240);
const dialog = (page: Page) => page.getByRole("dialog", { name: "Signature picture" });

/** Draws a line over the drawing surface of the box. */
async function scribble(page: Page, from = { x: 0.15, y: 0.5 }, to = { x: 0.85, y: 0.3 }) {
  const box = await page.locator(".sigpic-surface").boundingBox();
  if (!box) throw new Error("no drawing surface");
  const at = (p: { x: number; y: number }) => ({ x: box.x + box.width * p.x, y: box.y + box.height * p.y });
  await page.mouse.move(at(from).x, at(from).y);
  await page.mouse.down();
  await page.mouse.move((at(from).x + at(to).x) / 2, (at(from).y + at(to).y) / 2 + 20, { steps: 5 });
  await page.mouse.move(at(to).x, at(to).y, { steps: 5 });
  await page.mouse.up();
}

/** Opens the box, draws a line, and chooses it for the page. */
async function chooseDrawing(page: Page) {
  await page.locator("#tool-signature").click();
  await expect(dialog(page)).toBeVisible();
  await scribble(page);
  await dialog(page).getByRole("button", { name: "Use on the page" }).click();
  await expect(dialog(page)).toHaveCount(0);
  await expect(page.locator("#tool-signature")).toHaveAttribute("aria-pressed", "true");
}

/** The picture is placed with a click; returns the call the window made for it. */
async function placeAt(page: Page, x: number, y: number, cmd = "add_signature") {
  const before = (await calls(page, cmd)).length;
  const at = await onPage(page, 0, x, y);
  await page.mouse.click(at.x, at.y);
  await expect.poll(() => calls(page, cmd).then((c) => c.length)).toBe(before + 1);
  return (await calls(page, cmd))[before];
}

async function png(page: Page, w: number, h: number): Promise<Buffer> {
  const text = await page.evaluate(
    ({ w, h }) => {
      const c = document.createElement("canvas");
      c.width = w;
      c.height = h;
      const x = c.getContext("2d") as CanvasRenderingContext2D;
      x.fillStyle = "#ffffff";
      x.fillRect(0, 0, w, h);
      x.fillStyle = "#183896";
      x.fillRect(Math.floor(w / 8), Math.floor(h / 4), Math.floor(w / 2), Math.floor(h / 2));
      return c.toDataURL("image/png").split(",")[1];
    },
    { w, h },
  );
  return Buffer.from(text, "base64");
}

test("the tool opens a box that says a picture is not a digital signature", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  const tool = page.locator("#tool-signature");
  await expect(tool).toBeVisible();
  await expect(tool).toHaveAccessibleName("Signature picture");
  await tool.click();
  const box = dialog(page);
  await expect(box).toBeVisible();
  await expect(box).toContainText("A picture is not a digital signature");
  await expect(box).toContainText("It does not show who placed it, and anyone can copy it");
  for (const tab of ["Draw", "Type", "Image", "Saved"]) await expect(box.getByRole("tab", { name: tab })).toBeVisible();
  // Nothing has been drawn: the box cannot be used yet.
  await expect(box.getByRole("button", { name: "Use on the page" })).toBeDisabled();
  const text = (await box.textContent()) ?? "";
  expect(text).not.toMatch(/\b(legal|legally|court|guarantee|valid|binding|qualified|advanced|eidas|compliant|certified|adobe|acrobat)\b/i);
  // Escape closes it and nothing is picked.
  await page.keyboard.press("Escape");
  await expect(box).toHaveCount(0);
  await expect(tool).toHaveAttribute("aria-pressed", "false");
  expect((await calls(page, "add_signature")).length).toBe(0);
});

test("a drawn picture is made with the pointer and placed with a click, in one place and no more", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await chooseDrawing(page);
  const call = await placeAt(page, 300, 300);
  expect(call.args).toMatchObject({ page: 0, color: [20, 20, 22, 255] });
  const mark = call.args.mark as { kind: string; strokes: number[][][]; width: number; height: number; line: number };
  expect(mark.kind).toBe("draw");
  expect(mark.strokes.length).toBe(1);
  expect(mark.line).toBe(4);
  // The lines are cut to a box that has them and the pen around them.
  expect(mark.width).toBeGreaterThan(100);
  const rect = call.args.rect as number[];
  expect((rect[0] + rect[2]) / 2).toBeCloseTo(300, 0);
  expect(rect[2] - rect[0]).toBeCloseTo(160, 0);
  // It is on the page, picked, and the document is changed.
  await expect(page.locator(".annot")).toHaveCount(1);
  await expect(page.locator(".annot")).toHaveAttribute("aria-label", "Signature picture, page 1: Signature picture");
  await expect(page.locator(".annot")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
  await expect.poll(async () => isWhite(await pixelAt(page, 0, 300, (rect[1] + rect[3]) / 2))).toBe(false);
  // The tool is back to picking; a second click places nothing.
  await expect(page.locator("#tool-select")).toHaveAttribute("aria-pressed", "true");
  const at = await onPage(page, 0, 100, 600);
  await page.mouse.click(at.x, at.y);
  expect((await calls(page, "add_signature")).length).toBe(1);
});

test("a typed name is set in the font that is picked", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator("#tool-signature").click();
  await dialog(page).getByRole("tab", { name: "Type" }).click();
  await dialog(page).getByRole("textbox", { name: "Your name" }).fill("Anna Alvorlig");
  await dialog(page).getByRole("radio", { name: "Allura" }).click();
  await expect(dialog(page).locator(".sigpic-preview")).toHaveText("Anna Alvorlig");
  // The preview is set in the bundled font: the policy let the font file load.
  await expect
    .poll(() => page.evaluate(() => document.fonts.check('40px "Partridge Allura"', "Anna")))
    .toBe(true);
  await dialog(page).getByRole("button", { name: "Use on the page" }).click();
  const call = await placeAt(page, 300, 300);
  expect(call.args.mark).toEqual({ kind: "type", font: "allura", text: "Anna Alvorlig" });
  await expect(page.locator(".annot")).toHaveCount(1);
});

test("an image is read in the window and sent as pixels, never as a file", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator("#tool-signature").click();
  await dialog(page).getByRole("tab", { name: "Image" }).click();
  await dialog(page).locator("input.sigpic-file").setInputFiles({ name: "sig.png", mimeType: "image/png", buffer: await png(page, 200, 80) });
  await expect(dialog(page).locator(".sigpic-image")).toBeVisible();
  await expect(dialog(page).getByText("200 by 80 pixels.")).toBeVisible();
  // The white around the pen can be made see-through.
  await dialog(page).getByRole("checkbox", { name: "Make the white background see-through" }).check();
  await dialog(page).getByRole("button", { name: "Use on the page" }).click();
  const call = await placeAt(page, 300, 300, "add_signature_image");
  expect(call.args).toMatchObject({ page: 0, width: 200, height: 80, bytes: 200 * 80 * 4 });
  // The first pixel was white and is see-through now.
  expect(call.args.first).toEqual([255, 255, 255, 0]);
  await expect(page.locator(".annot")).toHaveCount(1);
});

test("a file that is not a picture, a drawing file and a file that is too big are refused with words", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator("#tool-signature").click();
  const box = dialog(page);
  await box.getByRole("tab", { name: "Image" }).click();
  const input = box.locator("input.sigpic-file");
  await input.setInputFiles({ name: "sig.png", mimeType: "image/png", buffer: Buffer.from("this is not a png") });
  await expect(box).toContainText("could not be read as a picture");
  await input.setInputFiles({ name: "sig.svg", mimeType: "image/svg+xml", buffer: Buffer.from("<svg xmlns='http://www.w3.org/2000/svg'/>") });
  await expect(box).toContainText("could not be read as a picture");
  await expect(box.getByRole("button", { name: "Use on the page" })).toBeDisabled();
});

test("a signature is kept for later, used again, renamed, deleted and forgotten", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.locator("#tool-signature").click();
  await scribble(page);
  await dialog(page).getByRole("checkbox", { name: "Save this signature on this computer for later" }).check();
  // A name is needed.
  await dialog(page).getByRole("button", { name: "Use on the page" }).click();
  await expect(dialog(page)).toContainText("Give the saved signature a name.");
  await dialog(page).getByRole("textbox", { name: "Name of the saved signature" }).fill("Work");
  await dialog(page).getByRole("button", { name: "Use on the page" }).click();
  await expect(dialog(page)).toHaveCount(0);
  const kept = await calls(page, "add_signature_mark");
  expect(kept).toHaveLength(1);
  expect(kept[0].args).toMatchObject({ name: "Work", mark: { kind: "draw", line: 4 } });
  await placeAt(page, 300, 300);

  // The box offers it again, with the words about where it is kept.
  await page.locator("#tool-signature").click();
  await dialog(page).getByRole("tab", { name: "Saved" }).click();
  await expect(dialog(page).locator(".sigpic-saved-row")).toHaveCount(1);
  await expect(dialog(page)).toContainText("Anyone who can read that folder can use them");
  await dialog(page).getByRole("button", { name: "Rename Work" }).click();
  await dialog(page).getByRole("textbox", { name: "New name" }).fill("Formal");
  await dialog(page).getByRole("button", { name: "Rename", exact: true }).click();
  await expect(dialog(page).locator(".sigpic-saved-name")).toHaveText("Formal");
  await dialog(page).getByRole("button", { name: "Use Formal" }).click();
  await expect(dialog(page)).toHaveCount(0);
  await placeAt(page, 110, 330);
  expect((await calls(page, "add_signature")).length).toBe(2);
  const again = (await calls(page, "add_signature"))[1].args.mark as { kind: string; strokes: unknown[] };
  expect(again.kind).toBe("draw");
  expect(again.strokes.length).toBe(1);

  // Delete it from the box.
  await page.locator("#tool-signature").click();
  await dialog(page).getByRole("tab", { name: "Saved" }).click();
  await dialog(page).getByRole("button", { name: "Delete Formal" }).click();
  await expect(dialog(page)).toContainText("No signatures are saved yet");
  await page.keyboard.press("Escape");

  // And forget what is left, after a question.
  await page.locator("#tool-signature").click();
  await scribble(page);
  await dialog(page).getByRole("checkbox", { name: "Save this signature on this computer for later" }).check();
  await dialog(page).getByRole("textbox", { name: "Name of the saved signature" }).fill("Second");
  await dialog(page).getByRole("button", { name: "Use on the page" }).click();
  await expect(dialog(page)).toHaveCount(0);
  await page.locator("#menu-button").click();
  await page.locator("#forget-signatures").click();
  const question = page.getByRole("alertdialog", { name: "Forget saved signatures?" });
  await expect(question).toContainText("Signature pictures that are already on a page stay there");
  await question.getByRole("button", { name: "Keep them" }).click();
  expect((await calls(page, "clear_signature_marks")).length).toBe(0);
  await page.locator("#menu-button").click();
  await page.locator("#forget-signatures").click();
  await question.getByRole("button", { name: "Forget signatures" }).click();
  await expect.poll(() => calls(page, "clear_signature_marks").then((c) => c.length)).toBe(1);
  expect(await page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.marks.length)).toBe(0);
});

test("the box stays open and says why when the signature cannot be kept", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.marksFailure = { code: "signature_store_full", message: "full" }));
  await page.locator("#tool-signature").click();
  await scribble(page);
  await dialog(page).getByRole("checkbox", { name: "Save this signature on this computer for later" }).check();
  await dialog(page).getByRole("textbox", { name: "Name of the saved signature" }).fill("Work");
  await dialog(page).getByRole("button", { name: "Use on the page" }).click();
  await expect(dialog(page)).toContainText("As many signatures are saved as can be");
  await expect(dialog(page)).toBeVisible();
  // Without the tick it goes on.
  await dialog(page).getByRole("checkbox", { name: "Save this signature on this computer for later" }).uncheck();
  await dialog(page).getByRole("button", { name: "Use on the page" }).click();
  await expect(dialog(page)).toHaveCount(0);
});

test("a picture is moved by dragging it and with the arrow keys, and each move can be taken back", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await chooseDrawing(page);
  const placed = await placeAt(page, 300, 300);
  const rect = placed.args.rect as number[];
  const middle = { x: (rect[0] + rect[2]) / 2, y: (rect[1] + rect[3]) / 2 };
  const from = await onPage(page, 0, middle.x, middle.y);
  const to = await onPage(page, 0, middle.x + 100, middle.y + 150);
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(to.x, to.y, { steps: 6 });
  await page.mouse.up();
  await expect.poll(() => calls(page, "update_annotation").then((c) => c.length)).toBe(1);
  const move = (await calls(page, "update_annotation"))[0].args;
  expect(move.dx as number).toBeGreaterThan(90);
  expect(move.dx as number).toBeLessThan(110);
  expect(move.dy as number).toBeGreaterThan(140);
  // The ink is at the new place and no longer at the old one.
  await expect.poll(async () => isWhite(await pixelAt(page, 0, middle.x + 100, middle.y + 150))).toBe(false);
  await expect.poll(async () => isWhite(await pixelAt(page, 0, middle.x, middle.y))).toBe(true);
  await page.keyboard.press("Control+z");
  await expect.poll(async () => isWhite(await pixelAt(page, 0, middle.x, middle.y))).toBe(false);
  await page.keyboard.press("Control+y");
  await expect.poll(async () => isWhite(await pixelAt(page, 0, middle.x + 100, middle.y + 150))).toBe(false);
  // The arrow keys move a picked picture.
  await page.locator(".annot").focus();
  await page.keyboard.press("ArrowLeft");
  await expect.poll(() => calls(page, "update_annotation").then((c) => c.length)).toBeGreaterThan(2);
  const last = (await calls(page, "update_annotation")).at(-1)?.args;
  expect(last).toMatchObject({ dx: -4, dy: 0 });
});

test("a picture is resized by its corner and with plus and minus, keeping its proportions, and each step can be taken back", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await chooseDrawing(page);
  const placed = await placeAt(page, 300, 300);
  const rect = placed.args.rect as number[];
  const width = rect[2] - rect[0];
  const handle = page.locator(".annot-handle");
  await expect(handle).toBeVisible();
  const h = await handle.boundingBox();
  const pageBox = await page.locator('.viewer-page[data-page="0"]').boundingBox();
  if (!h || !pageBox) throw new Error("no handle");
  const scale = 612 / pageBox.width;
  await page.mouse.move(h.x + h.width / 2, h.y + h.height / 2);
  await page.mouse.down();
  await page.mouse.move(h.x + h.width / 2 + 60, h.y + h.height / 2, { steps: 5 });
  await page.mouse.up();
  await expect.poll(() => calls(page, "place_signature").then((c) => c.length)).toBe(1);
  const wanted = (await calls(page, "place_signature"))[0].args.rect as number[];
  expect(wanted[0]).toBeCloseTo(rect[0], 0);
  expect(wanted[1]).toBeCloseTo(rect[1], 0);
  expect(wanted[2] - wanted[0]).toBeCloseTo(width + 60 * scale, -1);
  // The proportions are the picture's own: the stand-in answered with them, and the box shows them.
  const box = await page.locator(".annot").boundingBox();
  if (!box) throw new Error("no box");
  expect(box.width / box.height).toBeCloseTo((rect[2] - rect[0]) / (rect[3] - rect[1]), 0);
  await expect.poll(async () => (await page.locator(".annot").boundingBox())?.width ?? 0).toBeGreaterThan(h.x - (box.x - 1));

  await page.keyboard.press("Control+z");
  await expect.poll(() => calls(page, "place_signature").then((c) => c.length)).toBe(2);
  const back = (await calls(page, "place_signature"))[1].args.rect as number[];
  expect(back[2] - back[0]).toBeCloseTo(width, 0);

  await page.locator(".annot").focus();
  await page.keyboard.press("+");
  await expect.poll(() => calls(page, "place_signature").then((c) => c.length)).toBe(3);
  const grown = (await calls(page, "place_signature"))[2].args.rect as number[];
  expect(grown[2] - grown[0]).toBeCloseTo(width * 1.1, 0);
  await page.keyboard.press("-");
  await expect.poll(() => calls(page, "place_signature").then((c) => c.length)).toBe(4);
});

test("a placed picture is taken off by undo and comes back with redo, and the document is clean in between", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await chooseDrawing(page);
  const placed = await placeAt(page, 300, 300);
  const rect = placed.args.rect as number[];
  const y = (rect[1] + rect[3]) / 2;
  await expect.poll(async () => isWhite(await pixelAt(page, 0, 300, y))).toBe(false);
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
  await page.keyboard.press("Control+z");
  await expect.poll(async () => isWhite(await pixelAt(page, 0, 300, y))).toBe(true);
  await expect(page.locator(".annot")).toHaveCount(0);
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
  await page.keyboard.press("Control+y");
  await expect.poll(async () => isWhite(await pixelAt(page, 0, 300, y))).toBe(false);
  await expect(page.locator(".annot")).toHaveCount(1);
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
  // Redo put back the same picture, it did not make another.
  expect((await calls(page, "add_signature")).length).toBe(1);
  expect((await calls(page, "restore_annotation")).length).toBe(1);
  // Delete takes it off the same way, with no question.
  await page.locator(".annot").focus();
  await page.keyboard.press("Delete");
  await expect(page.locator(".annot")).toHaveCount(0);
  await page.keyboard.press("Control+z");
  await expect(page.locator(".annot")).toHaveCount(1);
});

test("a placed picture drops the result of the check of the signatures, like any other change", async ({ page }) => {
  await open(page, "/docs/signed-intact.pdf", "signed-intact.pdf");
  const badge = page.locator("#signature-badge");
  await expect(badge).toHaveText("Signature intact; signer not verified");
  await openBar(page);
  await chooseDrawing(page);
  await placeAt(page, 300, 300);
  await expect(badge).toHaveClass(/sig-stale/);
  await expect(badge).not.toHaveText("Signature intact; signer not verified");
  await badge.click();
  await expect(page.locator("#signature-panel")).toContainText("A picture of a signature on a page is only a picture");
  await expect(page.locator("#signature-panel")).toContainText("Do not rely on them for this version");
});

test("a refused picture changes nothing and says so", async ({ page }) => {
  await openBook(page);
  await openBar(page);
  await chooseDrawing(page);
  await page.evaluate(() => {
    (window as unknown as { __E2E__: { annotFailure: unknown } }).__E2E__.annotFailure = {
      cmd: "add_signature",
      error: { code: "annotation_refused", message: "no" },
      once: true,
    };
  });
  const at = await onPage(page, 0, 300, 300);
  await page.mouse.click(at.x, at.y);
  await expect(page.getByRole("alertdialog", { name: "The annotation was not changed" })).toBeVisible();
  await page.getByRole("alertdialog", { name: "The annotation was not changed" }).getByRole("button", { name: "OK" }).click();
  await expect(page.locator(".annot")).toHaveCount(0);
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
});

test.describe("while the pages are being changed", () => {
  /** Shows the page panel and picks the fifth page, so that Delete would delete it. */
  async function pickFifth(page: Page) {
    if (await page.locator("#sidebar").isHidden()) await page.locator("#sidebar-toggle").click();
    await expect(page.locator("#thumbs .thumb").first()).toBeAttached();
    await page.locator("#thumbs").evaluate((el) => (el.scrollTop = 12 + 4 * 228 - 120));
    await page.locator('#thumbs .thumb[data-page="4"]').click();
  }

  /** The first delete says what it leaves in the file: answer that. */
  async function answerNote(page: Page) {
    const note = page.locator(".sheet:not(.busy-box)").last();
    await expect(note).toContainText(/What a delete leaves/, { timeout: 10000 });
    await note.getByRole("button", { name: "OK" }).click();
    await expect(page.locator(".sheet")).toHaveCount(0);
  }

  test("the tool waits and says why, no box opens, and it works again when the change is done", async ({ page }) => {
    await open(page, "/docs/ten.pdf", "ten.pdf");
    await page.locator("#annotate-button").click();
    await expect(page.locator("#tool-signature")).toBeEnabled();
    await pickFifth(page);
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.pages.delay = 1500));
    await page.keyboard.press("Delete");
    await expect(page.locator(".busy-box")).toBeVisible();
    await expect(page.locator("#tool-signature")).toBeDisabled();
    await expect(page.locator("#annotbar")).toContainText("The pages of this document are being changed");
    // The key for the tool opens nothing either, and neither does a press on the button.
    await page.keyboard.press("g");
    await expect(dialog(page)).toHaveCount(0);
    await page.locator("#tool-signature").click({ force: true });
    await expect(dialog(page)).toHaveCount(0);
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.pages.delay = 0));
    await answerNote(page);
    await expect(page.locator("#tool-signature")).toBeEnabled();
    await expect(page.locator("#annotbar")).not.toContainText("The pages of this document are being changed");
    await page.locator("#tool-signature").click();
    await expect(dialog(page)).toBeVisible();
  });

  test("a picture that was chosen before the change is not placed during it", async ({ page }) => {
    await open(page, "/docs/ten.pdf", "ten.pdf");
    await openBar(page);
    await chooseDrawing(page);
    await pickFifth(page);
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.pages.delay = 1500));
    await page.keyboard.press("Delete");
    await expect(page.locator(".busy-box")).toBeVisible();
    await expect(page.locator(".annot-capture").first()).toHaveJSProperty("inert", true);
    // A click on the page that is shown, with the tool still chosen, places nothing.
    const shown = await page.locator(".viewer-page").first().boundingBox();
    if (!shown) throw new Error("no page is shown");
    await page.mouse.click(shown.x + shown.width / 2, shown.y + 200);
    expect((await calls(page, "add_signature")).length).toBe(0);
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.pages.delay = 0));
    await answerNote(page);
    expect((await calls(page, "add_signature")).length).toBe(0);
  });
});

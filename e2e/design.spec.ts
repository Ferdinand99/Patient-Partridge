import { expect, test, type Page } from "@playwright/test";

// The form designer ("Edit form"), in a real browser, with the same content security policy as the
// app and a stand-in for the Rust side (e2e/fake-tauri.js with e2e/fake-pages.js on top, which keeps
// the widgets of /docs/design.pdf and follows the rules of the engine). What the tests read is what a
// person sees, and what the window sent to the app process. The real engine is tried in
// crates/engine/tests/author.rs.

type Call = { cmd: string; args: Record<string, unknown> };
type Widget = { page: number; name: string; kind: string; rect: number[]; export: string; props: Record<string, unknown> };
type Stand = {
  violations: string[];
  dialog: string[];
  pages: {
    calls: Call[];
    failNext: { cmd: string; error: { code: string; message: string } } | null;
    delay: number;
    lastDoc: number;
    designWidgets(doc: number): Widget[];
  };
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ path: "e2e/fake-tauri.js" });
  await page.addInitScript({ path: "e2e/fake-pages.js" });
  await page.goto("/");
});

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.violations);
  expect(violations, "the content security policy blocked something").toEqual([]);
});

const stand = <T>(page: Page, f: (s: Stand) => T): Promise<T> =>
  page.evaluate(`(${f.toString()})(window.__E2E__)`) as Promise<T>;

async function openDesign(page: Page) {
  const path = "/docs/design.pdf";
  await page.evaluate((p) => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = [p]), path);
  await page.locator("#empty-open").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("design.pdf");
  await expect(page.locator(".viewer-canvas").first()).toBeAttached();
  await page.locator("#design-button").click();
  await expect(page.locator("#designbar")).toBeVisible();
  await expect(page.locator("#design-panel")).toBeVisible();
}

const applies = (page: Page) =>
  stand(page, (s) => s.pages.calls.filter((c) => c.cmd === "author_apply").map((c) => (c.args as { ops: unknown[] }).ops));

const widgets = (page: Page) => stand(page, (s) => s.pages.designWidgets(s.pages.lastDoc));

const pageBox = async (page: Page) => {
  const box = await page.locator('.viewer-page[data-page="0"]').boundingBox();
  if (!box) throw new Error("the page is not on screen");
  return box;
};

/** A point given in points from the top left of the first page, as a point of the screen. */
async function at(page: Page, x: number, y: number) {
  const box = await pageBox(page);
  const k = box.width / 612;
  return { x: box.x + x * k, y: box.y + y * k, k };
}

async function place(page: Page, tool: string, x: number, y: number) {
  await page.locator(`#dtool-${tool}`).click();
  const p = await at(page, x, y);
  await page.mouse.click(p.x, p.y);
}

async function rested(page: Page, calls: number) {
  // The changes rest for a moment, then go as one batch.
  await expect.poll(async () => (await applies(page)).length, { timeout: 4000 }).toBe(calls);
}

test("opens a bar and a panel, and keeps the annotation bar and the form designer apart", async ({ page }) => {
  await openDesign(page);
  await expect(page.locator("#design-button")).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator("#dtool-select")).toHaveAttribute("aria-pressed", "true");
  await page.locator("#annotate-button").click();
  await expect(page.locator("#designbar")).toBeHidden();
  await expect(page.locator("#annotbar")).toBeVisible();
  await page.locator("#design-button").click();
  await expect(page.locator("#annotbar")).toBeHidden();
  await expect(page.locator("#designbar")).toBeVisible();
  await page.locator("#design-button").click();
  await expect(page.locator("#designbar")).toBeHidden();
  await expect(page.locator("#design-panel")).toBeHidden();
});

test("places a text field with a click, names it, picks it and sends it as one change", async ({ page }) => {
  await openDesign(page);
  await place(page, "text", 250, 450);
  const box = page.locator(".design-box");
  await expect(box).toHaveCount(1);
  await expect(box).toHaveAttribute("data-name", "Text 1");
  await expect(box).toHaveAttribute("aria-pressed", "true");
  await expect(box.locator(".design-handle")).toHaveCount(8);
  await expect(page.locator("#design-panel input[id$='-name']")).toHaveValue("Text 1");
  // The tool is back to picking after one field.
  await expect(page.locator("#dtool-select")).toHaveAttribute("aria-pressed", "true");
  await rested(page, 1);
  const [ops] = await applies(page);
  expect(ops).toHaveLength(1);
  const op = ops[0] as { op: string; field: { name: string; kind: string; props: unknown; widgets: { page: number; rect: number[]; export: string }[] } };
  expect(op).toMatchObject({ op: "add", field: { name: "Text 1", kind: "text", props: {} } });
  expect(op.field.widgets).toHaveLength(1);
  expect(op.field.widgets[0].page).toBe(0);
  op.field.widgets[0].rect.forEach((v, i) => expect(Math.abs(v - [170, 439, 330, 461][i])).toBeLessThan(1));
  expect(await widgets(page)).toHaveLength(1);
});

test("sizes a box by dragging", async ({ page }) => {
  await openDesign(page);
  await page.locator("#dtool-signature").click();
  const a = await at(page, 100, 300);
  const b = await at(page, 360, 350);
  await page.keyboard.down("Alt");
  await page.mouse.move(a.x, a.y);
  await page.mouse.down();
  await page.mouse.move(b.x, b.y, { steps: 5 });
  await expect(page.locator(".design-ghost").first()).toBeVisible();
  await page.mouse.up();
  await page.keyboard.up("Alt");
  await rested(page, 1);
  const [ops] = await applies(page);
  const op = ops[0] as { field: { kind: string; widgets: { rect: number[] }[] } };
  expect(op.field.kind).toBe("signature");
  op.field.widgets[0].rect.forEach((v, i) => expect(v).toBeCloseTo([100, 300, 360, 350][i], 0));
});

test("moves a box with the pointer and snaps to the end of a printed word, and Alt turns the snapping off", async ({ page }) => {
  await openDesign(page);
  await place(page, "text", 250, 450);
  await rested(page, 1);
  await expect(page.locator(".design-box")).toHaveCount(1);
  const grab = async (alt: boolean, left: number, top: number) => {
    const box = page.locator(".design-box");
    const bb = await box.boundingBox();
    const pb = await pageBox(page);
    if (!bb) throw new Error("no box");
    const k = pb.width / 612;
    // The word "Name:" ends at 102 across. Carry the left edge of the box to `left`.
    const from = { x: bb.x + bb.width / 2, y: bb.y + bb.height / 2 };
    const to = { x: from.x - (bb.x - pb.x - left * k), y: from.y - (bb.y - pb.y - top * k) };
    if (alt) await page.keyboard.down("Alt");
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(to.x, to.y, { steps: 6 });
    await page.mouse.up();
    if (alt) await page.keyboard.up("Alt");
  };
  await grab(false, 105, 140);
  await rested(page, 2);
  const [, second] = await applies(page);
  const moved = second[0] as { op: string; rect: number[] };
  expect(moved.op).toBe("move");
  // The left edge caught on the end of the word (102), not where the pointer left it (105).
  expect(moved.rect[0]).toBeCloseTo(102, 0);
  await grab(true, 109, 200);
  await rested(page, 3);
  const [, , third] = await applies(page);
  const free = third[0] as { rect: number[] };
  expect(free.rect[0]).toBeCloseTo(109, 0);
});

test("resizes with a handle and nudges with the arrow keys", async ({ page }) => {
  await openDesign(page);
  await place(page, "text", 250, 450);
  await rested(page, 1);
  const handle = page.locator('.design-box .design-handle[data-handle="se"]');
  const hb = await handle.boundingBox();
  if (!hb) throw new Error("no handle");
  const k = (await pageBox(page)).width / 612;
  await page.keyboard.down("Alt");
  await page.mouse.move(hb.x + hb.width / 2, hb.y + hb.height / 2);
  await page.mouse.down();
  await page.mouse.move(hb.x + hb.width / 2 + 20 * k, hb.y + hb.height / 2 + 10 * k, { steps: 4 });
  await page.mouse.up();
  await page.keyboard.up("Alt");
  await rested(page, 2);
  const [, second] = await applies(page);
  const resized = (second[0] as { rect: number[] }).rect;
  expect(resized[2] - resized[0]).toBeCloseTo(180, 0);
  expect(resized[3] - resized[1]).toBeCloseTo(32, 0);
  // Three presses of an arrow key are one change.
  await page.locator(".design-box").focus();
  for (let i = 0; i < 3; i++) await page.keyboard.press("ArrowRight");
  await rested(page, 3);
  const [, , third] = await applies(page);
  expect(third).toHaveLength(1);
  expect((third[0] as { rect: number[] }).rect[0]).toBeCloseTo(resized[0] + 3, 0);
});

test("takes back and makes again what was placed, with the usual Undo and Redo", async ({ page }) => {
  await openDesign(page);
  await place(page, "checkbox", 300, 400);
  await rested(page, 1);
  await expect(page.locator(".design-box")).toHaveCount(1);
  await expect(page.locator("#design-undo")).toBeEnabled();
  await page.locator("#design-undo").click();
  await expect(page.locator(".design-box")).toHaveCount(0);
  await expect(page.locator("#design-redo")).toBeEnabled();
  expect(await widgets(page)).toHaveLength(0);
  await page.locator("#design-redo").click();
  await expect(page.locator(".design-box")).toHaveCount(1);
  expect(await widgets(page)).toHaveLength(1);
  // The keyboard does the same.
  await page.locator(".viewer").focus();
  await page.keyboard.press("Control+z");
  await expect(page.locator(".design-box")).toHaveCount(0);
});

test("adds the clicks of the radio tool to one group until Escape, in one change", async ({ page }) => {
  await openDesign(page);
  await page.locator("#dtool-radio").click();
  for (const x of [100, 160, 220]) {
    const p = await at(page, x, 350);
    await page.mouse.click(p.x, p.y);
  }
  await expect(page.locator(".design-box")).toHaveCount(3);
  await expect(page.locator("#dtool-radio")).toHaveAttribute("aria-pressed", "true");
  await rested(page, 1);
  const [ops] = await applies(page);
  expect((ops as { op: string }[]).map((o) => o.op)).toEqual(["add", "addWidget", "addWidget"]);
  await page.keyboard.press("Escape");
  await expect(page.locator("#dtool-select")).toHaveAttribute("aria-pressed", "true");
});

test("refuses a name that is taken or has a dot, says why, and sends nothing for it", async ({ page }) => {
  await openDesign(page);
  await place(page, "text", 200, 250);
  await rested(page, 1);
  await place(page, "text", 200, 380);
  await rested(page, 2);
  const name = page.locator("#design-panel input[id$='-name']");
  await expect(name).toHaveValue("Text 2");
  await name.fill("Text 1");
  await name.press("Tab");
  await expect(page.locator(".design-error")).toContainText("already a field called");
  await name.fill("a.b");
  await name.press("Tab");
  await expect(page.locator(".design-error")).toContainText("dot");
  await name.fill("last_name");
  await name.press("Tab");
  await expect(page.locator(".design-error")).toBeHidden();
  await rested(page, 3);
  const [, , third] = await applies(page);
  expect(third).toEqual([expect.objectContaining({ op: "update", name: "Text 2", newName: "last_name" })]);
  expect((await widgets(page)).map((w) => w.name).sort()).toEqual(["Text 1", "last_name"]);
});

test("changes the properties of a field: the tooltip, required, the choices and the default", async ({ page }) => {
  await openDesign(page);
  await place(page, "combo", 200, 300);
  await rested(page, 1);
  const panel = page.locator("#design-panel");
  const choices = panel.locator("textarea[id$='-choices']");
  await panel.locator("input[id$='-tooltip']").fill("Pick a country");
  await panel.locator("input[id$='-required']").check();
  await choices.fill("Norge\nSverige\nÅland");
  // The change is made when the box is left; the list of the default is made from the choices.
  await choices.press("Tab");
  await panel.locator("select[id$='-defaultChoice']").selectOption("Sverige");
  await expect.poll(async () => (await widgets(page))[0]?.props.defaultValue, { timeout: 5000 }).toBe("Sverige");
  expect((await widgets(page))[0].props).toMatchObject({
    tooltip: "Pick a country",
    required: true,
    choices: ["Norge", "Sverige", "Åland"],
    defaultValue: "Sverige",
  });
  // What the font cannot draw is refused where it is typed.
  await choices.fill("Norge\n€");
  await choices.press("Tab");
  await expect(page.locator(".design-error")).toContainText("cannot be drawn");
});

test("puts the fields of the page in a new Tab order", async ({ page }) => {
  await openDesign(page);
  await place(page, "text", 200, 200);
  await rested(page, 1);
  await place(page, "text", 200, 300);
  await rested(page, 2);
  const items = page.locator(".design-order-item");
  await expect(items).toHaveCount(2);
  await expect(items.first()).toContainText("Text 1");
  await items.first().getByRole("button", { name: /later in the Tab order/ }).click();
  await rested(page, 3);
  const [, , third] = await applies(page);
  expect(third).toEqual([
    {
      op: "reorder",
      page: 0,
      order: [
        { name: "Text 2", widget: 0 },
        { name: "Text 1", widget: 0 },
      ],
    },
  ]);
  await expect(page.locator(".design-order-item").first()).toContainText("Text 2");
});

test("deletes the picked field with the Delete key and the button", async ({ page }) => {
  await openDesign(page);
  await place(page, "list", 200, 200);
  await rested(page, 1);
  await page.locator(".design-box").focus();
  await page.keyboard.press("Delete");
  await expect(page.locator(".design-box")).toHaveCount(0);
  await rested(page, 2);
  const [, second] = await applies(page);
  expect(second).toEqual([{ op: "remove", name: "List 1" }]);
  expect(await widgets(page)).toHaveLength(0);
});

test("tells why when the engine refuses a change, and shows the fields as the document has them", async ({ page }) => {
  await openDesign(page);
  await place(page, "text", 200, 200);
  await rested(page, 1);
  await stand(page, (s) => {
    s.pages.failNext = { cmd: "author_apply", error: { code: "author_refused", message: "The field has to be inside the page." } };
  });
  await place(page, "text", 200, 400);
  await expect(page.getByRole("alertdialog")).toContainText("The field has to be inside the page.");
  await expect(page.getByRole("alertdialog")).toContainText("That change to the form was not made");
  await page.getByRole("alertdialog").locator(".btn-primary").click();
  // Only the field the document has is on the page.
  await expect(page.locator(".design-box")).toHaveCount(1);
});

test("says why a document cannot take fields", async ({ page }) => {
  await page.evaluate(() => {
    (window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/design.pdf"];
  });
  await page.locator("#empty-open").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("design.pdf");
  await stand(page, (s) => {
    s.pages.failNext = { cmd: "author_check", error: { code: "author_unsupported", message: "this document is encrypted, so fields cannot be added to it" } };
  });
  await page.locator("#design-button").click();
  await expect(page.locator(".notice")).toContainText("encrypted");
  await page.locator(".notice .btn-primary").click();
  await expect(page.locator("#designbar")).toBeHidden();
});

test("the boxes wait while a page tool works on the document, and the controls for filling in are out of the way", async ({ page }) => {
  await openDesign(page);
  await place(page, "text", 200, 200);
  await rested(page, 1);
  if (await page.locator("#sidebar").isHidden()) await page.locator("#sidebar-toggle").click();
  await stand(page, (s) => {
    s.pages.delay = 600;
  });
  await page.locator("#pt-blank").click();
  await expect(page.locator(".design-layer").first()).toHaveJSProperty("inert", true);
  await stand(page, (s) => {
    s.pages.delay = 0;
  });
  await expect(page.locator(".design-layer").first()).toHaveJSProperty("inert", false, { timeout: 5000 });
});

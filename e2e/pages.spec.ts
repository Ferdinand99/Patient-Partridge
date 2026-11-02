import { expect, test, type Page } from "@playwright/test";

// The page tools, in a real browser, with the same content security policy as the app and a
// stand-in for the Rust side (e2e/fake-tauri.js with e2e/fake-pages.js on top). The documents are
// made up and every page has a number, drawn as a shade, so a thumbnail or a page on screen says
// which page it is. What the tests read is what a person would see, and what the window sent to the
// app process.

type Call = { cmd: string; args: Record<string, unknown> };
type Stand = {
  calls: Call[];
  dialog: string[];
  violations: string[];
  title: string;
  pages: {
    calls: Call[];
    saves: { doc: number; labels: number[]; sizes: number[][]; as?: boolean }[];
    extracts: { doc: number; labels: number[] }[];
    inserts: { path: string; at: number; password: string | null }[];
    dirty: [number, boolean][];
    cancels: number;
    lastDoc: number;
    failNext: { cmd: string; error: { code: string; message: string } } | null;
    delay: number;
    delayOnly: number | null;
    working: boolean;
    labels(doc: number): number[];
    sizes(doc: number): number[][];
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

/** The shade the stand-in draws on the left of a page with this number. */
const shade = (id: number) => 40 + ((id * 37) % 190);

async function openDoc(page: Page, path: string): Promise<number> {
  await page.evaluate((p) => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = [p]), path);
  await page.locator("#empty-open, #open-button").first().click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText(path.split("/").pop() as string);
  await expect(page.locator(".viewer-canvas").first()).toBeAttached();
  if (await page.locator("#sidebar").isHidden()) await page.locator("#sidebar-toggle").click();
  await expect(page.locator("#pagetools")).toBeVisible();
  await expect(page.locator("#thumbs .thumb").first()).toBeAttached();
  return page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.pages.lastDoc);
}

const labels = (page: Page, doc: number) =>
  page.evaluate((d) => (window as unknown as { __E2E__: Stand }).__E2E__.pages.labels(d), doc);
const sizes = (page: Page, doc: number) =>
  page.evaluate((d) => (window as unknown as { __E2E__: Stand }).__E2E__.pages.sizes(d), doc);
const saves = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.pages.saves);
const extracts = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.pages.extracts);
const inserts = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.pages.inserts);
const cancels = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.pages.cancels);
const title = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.title);
const calls = (page: Page, cmd: string) =>
  page.evaluate((c) => (window as unknown as { __E2E__: Stand }).__E2E__.pages.calls.filter((x) => x.cmd === c).map((x) => x.args), cmd);

const thumb = (page: Page, index: number) => page.locator(`#thumbs .thumb[data-page="${index}"]`);
const picked = (page: Page) =>
  page.locator('#thumbs .thumb[aria-pressed="true"]').evaluateAll((els) => els.map((e) => Number((e as HTMLElement).dataset.page)));

/** The shade of the left of a thumbnail's picture, once it has been drawn. */
const thumbShade = (page: Page, index: number) =>
  thumb(page, index)
    .locator("canvas")
    .evaluate((c: HTMLCanvasElement) => {
      if (c.width === 0) return -1;
      return c.getContext("2d")?.getImageData(1, Math.floor(c.height / 2), 1, 1).data[0] ?? -1;
    });

/** The numbers of the pages the thumbnails show, from their pictures, for the first few. */
async function shownLabels(page: Page, count: number): Promise<number[]> {
  const found: number[] = [];
  for (let i = 0; i < count; i++) {
    await reveal(page, i);
    await expect.poll(() => thumbShade(page, i), { message: `thumbnail ${i} is drawn` }).toBeGreaterThan(0);
    found.push(await thumbShade(page, i));
  }
  return found;
}

const tool = (page: Page, id: string) => page.locator(`#${id}`);
const notice = (page: Page) => page.locator(".sheet").last();
const dirty = (page: Page) => page.locator(".tab-dirty");

/** The first delete and the first insert in a document each say what they leave out: answer that. */
async function answerNote(page: Page, about: RegExp) {
  await expect(notice(page)).toContainText(about);
  await notice(page).getByRole("button", { name: "OK" }).click();
  await expect(page.locator(".sheet")).toHaveCount(0);
}

/** Scrolls the list so that the thumbnail exists (only the ones near the visible part do). */
async function reveal(page: Page, index: number) {
  await page.locator("#thumbs").evaluate((el, i) => (el.scrollTop = Math.max(0, 12 + i * 228 - 120)), index);
  await expect(thumb(page, index)).toBeAttached();
}

async function press(page: Page, index: number, mods: ("Control" | "Shift")[] = []) {
  await reveal(page, index);
  for (const m of mods) await page.keyboard.down(m);
  await thumb(page, index).click();
  for (const m of mods.slice().reverse()) await page.keyboard.up(m);
}

test("the page panel shows the page tools, which wait for pages to be picked", async ({ page }) => {
  await openDoc(page, "/docs/ten.pdf");
  await expect(page.locator("#pagetools")).toHaveAttribute("role", "toolbar");
  for (const id of ["pt-left", "pt-right", "pt-delete", "pt-crop", "pt-extract"]) {
    await expect(tool(page, id)).toBeDisabled();
    await expect(tool(page, id)).toHaveAccessibleName(/\w+/);
  }
  await expect(tool(page, "pt-blank")).toBeEnabled();
  await expect(tool(page, "pt-insert")).toBeEnabled();
  await press(page, 2);
  for (const id of ["pt-left", "pt-right", "pt-delete", "pt-crop", "pt-extract"]) await expect(tool(page, id)).toBeEnabled();
  // The tools are not there in the table of contents.
  await page.locator("#side-tab-contents").click();
  await expect(page.locator("#pagetools")).toBeHidden();
  await page.locator("#side-tab-pages").click();
  await expect(page.locator("#pagetools")).toBeVisible();
});

test("pages are picked with the mouse and the keyboard", async ({ page }) => {
  await openDoc(page, "/docs/ten.pdf");
  await press(page, 1);
  expect(await picked(page)).toEqual([1]);
  await press(page, 3, ["Control"]);
  expect(await picked(page)).toEqual([1, 3]);
  await press(page, 5, ["Shift"]);
  expect(await picked(page)).toEqual([3, 4, 5]);
  // A plain click goes to the page and picks it alone.
  await press(page, 2);
  expect(await picked(page)).toEqual([2]);
  await expect(page.locator("#page-input")).toHaveValue("3");

  // Keyboard: arrows pick and move the focus, Shift extends, Escape lets go.
  await thumb(page, 2).focus();
  await page.keyboard.press("ArrowDown");
  expect(await picked(page)).toEqual([3]);
  await expect(thumb(page, 3)).toBeFocused();
  await page.keyboard.press("Shift+ArrowDown");
  await page.keyboard.press("Shift+ArrowDown");
  expect(await picked(page)).toEqual([3, 4, 5]);
  await expect(page.locator("#pt-status")).toHaveText("3 pages picked");
  await page.keyboard.press("Escape");
  expect(await picked(page)).toEqual([]);
  await page.keyboard.press("Control+a");
  await expect.poll(() => picked(page).then((p) => p.length)).toBeGreaterThan(0);
  await expect(page.locator("#pt-status")).toHaveText("10 pages picked");
  // Only the page the keyboard is on is in the tab order; every page says whether it is picked.
  const stops = await page.locator("#thumbs .thumb").evaluateAll((els) => els.filter((e) => (e as HTMLElement).tabIndex === 0).length);
  expect(stops).toBe(1);
  await expect(thumb(page, 0)).toHaveAttribute("aria-label", "Page 1, picked");
});

test("Delete removes the picked pages, the tab and the title show the change, and Ctrl+Z and Ctrl+Y take it back and make it again", async ({
  page,
}) => {
  const doc = await openDoc(page, "/docs/ten.pdf");
  await press(page, 3);
  await press(page, 5, ["Control"]);
  await page.keyboard.press("Delete");
  await expect(page.locator("#page-total")).toHaveText("/ 8");
  await answerNote(page, /What a delete leaves/);
  expect(await labels(page, doc)).toEqual([0, 1, 2, 4, 6, 7, 8, 9]);
  expect(await calls(page, "pages_delete")).toEqual([{ doc, pages: [3, 5] }]);
  await expect(dirty(page)).toBeVisible();
  expect(await title(page)).toContain(String.fromCharCode(0x2022));
  // The page that took the place is picked; the main view shows 8 pages.
  await expect(page.locator(".viewer-pages")).toBeAttached();
  expect(await shownLabels(page, 5)).toEqual([0, 1, 2, 4, 6].map(shade));

  await page.keyboard.press("Control+z");
  await expect(page.locator("#page-total")).toHaveText("/ 10");
  expect(await labels(page, doc)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  await expect(dirty(page)).toHaveCount(0);
  await page.keyboard.press("Control+y");
  await expect(page.locator("#page-total")).toHaveText("/ 8");
  expect(await labels(page, doc)).toEqual([0, 1, 2, 4, 6, 7, 8, 9]);
  await expect(dirty(page)).toBeVisible();
  // Each of the two steps went through the copy the app kept, with the number it gave.
  const restores = await calls(page, "pages_restore");
  expect(restores).toHaveLength(2);
  expect(restores[1].snapshot).not.toBe(restores[0].snapshot);
});

test("the last page cannot be deleted", async ({ page }) => {
  const doc = await openDoc(page, "/docs/ten.pdf");
  await page.locator("#thumbs").focus();
  await thumb(page, 0).focus();
  await page.keyboard.press("Control+a");
  await page.keyboard.press("Delete");
  await expect(notice(page)).toContainText("at least one page");
  await notice(page).getByRole("button", { name: "OK" }).click();
  expect(await calls(page, "pages_delete")).toEqual([]);
  expect((await labels(page, doc)).length).toBe(10);
});

test("dragging moves the picked pages to the place under the pointer", async ({ page }) => {
  const doc = await openDoc(page, "/docs/ten.pdf");
  await press(page, 0);
  const from = (await thumb(page, 0).boundingBox())!;
  const target = (await thumb(page, 2).boundingBox())!;
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 3);
  await page.mouse.down();
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 3 + 30, { steps: 3 });
  // While dragging, a line shows where the page would go, and the list says so.
  await page.mouse.move(target.x + target.width / 2, target.y + target.height * 0.75, { steps: 6 });
  await expect(page.locator("#thumbs .thumb-drop")).toBeVisible();
  await expect(page.locator("#thumbs")).toHaveAttribute("data-dragging", "");
  await page.mouse.up();
  await expect.poll(() => labels(page, doc)).toEqual([1, 2, 0, 3, 4, 5, 6, 7, 8, 9]);
  await expect(page.locator("#thumbs .thumb-drop")).toHaveCount(0);
  expect(await calls(page, "pages_reorder")).toEqual([{ doc, order: [1, 2, 0, 3, 4, 5, 6, 7, 8, 9] }]);
  // The moved page is picked where it went, and the thumbnails are the new order.
  await expect.poll(() => picked(page)).toEqual([2]);
  expect(await shownLabels(page, 4)).toEqual([1, 2, 0, 3].map(shade));
  // The click at the end of the drag did not send the main view anywhere else.
  await expect(dirty(page)).toBeVisible();
  await page.keyboard.press("Control+z");
  await expect.poll(() => labels(page, doc)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
});

test("dragging a page onto the place it is at moves nothing", async ({ page }) => {
  const doc = await openDoc(page, "/docs/ten.pdf");
  const box = (await thumb(page, 1).boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 3);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 3 + 25, { steps: 4 });
  await expect(page.locator("#thumbs .thumb-drop")).toHaveCount(0);
  await page.mouse.up();
  expect(await calls(page, "pages_reorder")).toEqual([]);
  expect((await labels(page, doc)).join()).toBe("0,1,2,3,4,5,6,7,8,9");
});

test("Alt and the arrow keys move the picked pages one place", async ({ page }) => {
  const doc = await openDoc(page, "/docs/ten.pdf");
  await press(page, 2);
  await press(page, 3, ["Control"]);
  await page.keyboard.press("Alt+ArrowDown");
  await expect.poll(() => labels(page, doc)).toEqual([0, 1, 4, 2, 3, 5, 6, 7, 8, 9]);
  await expect.poll(() => picked(page)).toEqual([3, 4]);
  await page.keyboard.press("Alt+ArrowUp");
  await expect.poll(() => labels(page, doc)).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
  await expect(page.locator("#pt-status")).toHaveText("2 pages picked");
});

test("R and Shift+R turn the picked pages, and the page in the main view changes shape", async ({ page }) => {
  const doc = await openDoc(page, "/docs/ten.pdf");
  await press(page, 1);
  const before = (await page.locator('.viewer-page[data-page="1"]').boundingBox())!;
  await page.keyboard.press("r");
  await expect.poll(async () => (await sizes(page, doc))[1]).toEqual([792, 612]);
  const after = (await page.locator('.viewer-page[data-page="1"]').boundingBox())!;
  expect(after.width).toBeGreaterThan(after.height);
  expect(before.width).toBeLessThan(before.height);
  expect(await calls(page, "pages_rotate")).toEqual([{ doc, pages: [1], turns: 1 }]);
  await page.keyboard.press("Shift+R");
  await expect.poll(async () => (await sizes(page, doc))[1]).toEqual([612, 792]);
  expect((await calls(page, "pages_rotate"))[1]).toEqual({ doc, pages: [1], turns: 3 });
  // Taking the second turn back turns the page right again, the first back puts it as it was.
  await page.keyboard.press("Control+z");
  await expect.poll(async () => (await sizes(page, doc))[1]).toEqual([792, 612]);
  await page.keyboard.press("Control+z");
  await expect.poll(async () => (await sizes(page, doc))[1]).toEqual([612, 792]);
  await expect(dirty(page)).toHaveCount(0);
});

test("the buttons of the bar do what the keys do", async ({ page }) => {
  const doc = await openDoc(page, "/docs/ten.pdf");
  await press(page, 4);
  await tool(page, "pt-right").click();
  await expect.poll(async () => (await sizes(page, doc))[4]).toEqual([792, 612]);
  await tool(page, "pt-left").click();
  await expect.poll(async () => (await sizes(page, doc))[4]).toEqual([612, 792]);
  await tool(page, "pt-delete").click();
  await expect.poll(() => labels(page, doc)).toEqual([0, 1, 2, 3, 5, 6, 7, 8, 9]);
});

test("a blank page goes after the picked pages, or after the page in view", async ({ page }) => {
  const doc = await openDoc(page, "/docs/mixed.pdf");
  await press(page, 2);
  await tool(page, "pt-blank").click();
  await expect.poll(async () => (await labels(page, doc)).length).toBe(6);
  // It takes the size of the page before it.
  expect((await calls(page, "pages_insert_blank"))[0]).toEqual({ doc, at: 3, width: 300, height: 300 });
  expect((await labels(page, doc)).slice(0, 5)).toEqual([0, 1, 2, 5, 3]);
  await expect.poll(() => picked(page)).toEqual([3]);
  await page.keyboard.press("Control+z");
  await expect.poll(async () => (await labels(page, doc)).length).toBe(5);
});

test.describe("cropping", () => {
  test("the box says what a crop does, and the frame and the numbers work together", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 3);
    await tool(page, "pt-crop").click();
    const dialog = page.getByRole("dialog", { name: "Crop pages" });
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("does not remove anything");
    await expect(dialog).toContainText("still in the file");
    await expect(page.locator("#crop-left")).toBeFocused();
    // Drag the right edge in by 50 pixels of the picture and the top left corner in a little.
    const stage = (await page.locator(".crop-stage").boundingBox())!;
    const handle = async (name: string) => (await page.locator(`.crop-handle[data-handle="${name}"]`).boundingBox())!;
    const e = await handle("e");
    await page.mouse.move(e.x + e.width / 2, e.y + e.height / 2);
    await page.mouse.down();
    await page.mouse.move(e.x + e.width / 2 - 50, e.y + e.height / 2, { steps: 4 });
    await page.mouse.up();
    const scale = Math.min(340 / 612, 340 / 792);
    const right = Number(await page.locator("#crop-right").inputValue());
    // 36 points to begin with, and 50 pixels of picture more.
    expect(right).toBeCloseTo(((36 + 50 / scale) * 25.4) / 72, 0);
    expect(stage.width).toBeGreaterThan(100);
    // Type the others.
    await page.locator("#crop-left").fill("10");
    await page.locator("#crop-top").fill("5");
    await page.locator("#crop-bottom").fill("0");
    await dialog.getByRole("button", { name: "Crop", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    const [call] = await calls(page, "pages_crop");
    const margins = call.margins as number[];
    const mm = 72 / 25.4;
    expect(call.doc).toBe(doc);
    expect(call.pages).toEqual([3]);
    expect(margins[0]).toBeCloseTo(10 * mm, 2);
    expect(margins[1]).toBeCloseTo(5 * mm, 2);
    expect(margins[3]).toBeCloseTo(0, 2);
    expect(margins[2]).toBeGreaterThan(36);
    // The page is smaller by exactly that; the others are not.
    const size = (await sizes(page, doc))[3];
    expect(size[0]).toBeCloseTo(612 - margins[0] - margins[2], 1);
    expect(size[1]).toBeCloseTo(792 - margins[1] - margins[3], 1);
    expect((await sizes(page, doc))[4]).toEqual([612, 792]);
    // And it goes back.
    await page.keyboard.press("Control+z");
    await expect.poll(async () => (await sizes(page, doc))[3]).toEqual([612, 792]);
    await page.keyboard.press("Control+y");
    await expect.poll(async () => (await sizes(page, doc))[3]).toEqual(size);
  });

  test("it can crop all pages, and Escape or Cancel changes nothing", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 0);
    await tool(page, "pt-crop").click();
    const dialog = page.getByRole("dialog", { name: "Crop pages" });
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await tool(page, "pt-crop").click();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    expect(await calls(page, "pages_crop")).toEqual([]);
    await tool(page, "pt-crop").click();
    await dialog.getByLabel("All 10 pages").check();
    await dialog.getByRole("button", { name: "Crop", exact: true }).click();
    const [call] = await calls(page, "pages_crop");
    expect(call.pages).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    expect((await sizes(page, doc)).every(([w, h]) => w < 612 && h < 792)).toBe(true);
  });

  test("a crop with nothing to cut cannot be made", async ({ page }) => {
    await openDoc(page, "/docs/ten.pdf");
    await press(page, 0);
    await tool(page, "pt-crop").click();
    const dialog = page.getByRole("dialog", { name: "Crop pages" });
    for (const side of ["left", "top", "right", "bottom"]) await page.locator(`#crop-${side}`).fill("0");
    await expect(dialog.getByRole("button", { name: "Crop", exact: true })).toBeDisabled();
  });
});

test.describe("pages from other files", () => {
  test("a file is added after the picked page and its pages are picked", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 4);
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/second.pdf", "/docs/third.pdf"]));
    await tool(page, "pt-insert").click();
    await expect.poll(async () => (await labels(page, doc)).length).toBe(15);
    await answerNote(page, /What comes with added pages/);
    expect(await labels(page, doc)).toEqual([0, 1, 2, 3, 4, 10, 11, 12, 13, 14, 5, 6, 7, 8, 9]);
    expect(await inserts(page)).toEqual([
      { path: "/docs/second.pdf", at: 5, password: null },
      { path: "/docs/third.pdf", at: 7, password: null },
    ]);
    await expect(page.locator("#pt-status")).toHaveText("5 pages picked");
    await expect(dirty(page)).toBeVisible();
    // One step takes back the last file, one more the first.
    await page.keyboard.press("Control+z");
    await expect.poll(async () => (await labels(page, doc)).length).toBe(12);
    await page.keyboard.press("Control+z");
    await expect.poll(async () => (await labels(page, doc)).length).toBe(10);
  });

  test("a protected file asks for its password, again if it is wrong, and then asks about the protection", async ({
    page,
  }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/locked.pdf"]));
    await tool(page, "pt-insert").click();
    const box = notice(page);
    await expect(box).toContainText("password protected");
    await box.getByLabel("Password").fill("nope");
    await box.getByRole("button", { name: "Add the pages" }).click();
    await expect(notice(page)).toContainText("not correct");
    await notice(page).getByLabel("Password").fill("secret");
    await notice(page).getByRole("button", { name: "Add the pages" }).click();
    // The password is right; the pages would lose their protection.
    await expect(notice(page)).toContainText("without that protection");
    await expect(notice(page)).toContainText('"locked.pdf" is protected by a password');
    expect((await labels(page, doc)).length).toBe(10);
    await notice(page).getByRole("button", { name: "Add the pages anyway" }).click();
    await expect.poll(async () => (await labels(page, doc)).length).toBe(12);
  });

  test("saying no to the protection, the signature or the password adds nothing", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    for (const [file, text] of [
      ["/docs/locked.pdf", "password"],
      ["/docs/signed.pdf", "is signed"],
    ]) {
      await page.evaluate((f) => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = [f]), file);
      await tool(page, "pt-insert").click();
      if (text === "password") {
        await notice(page).getByLabel("Password").fill("secret");
        await notice(page).getByRole("button", { name: "Add the pages" }).click();
      }
      await expect(notice(page)).toContainText(text === "password" ? "without that protection" : "without the signature");
      await notice(page).getByRole("button", { name: "Cancel" }).click();
    }
    expect((await labels(page, doc)).length).toBe(10);
    await expect(dirty(page)).toHaveCount(0);
  });

  test("a signed file's pages come only when the window says the signature may go", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/signed.pdf"]));
    await tool(page, "pt-insert").click();
    await expect(notice(page)).toContainText("The pages come without the signature");
    await notice(page).getByRole("button", { name: "Add the pages anyway" }).click();
    await expect.poll(async () => (await labels(page, doc)).length).toBe(11);
  });

  test("a file that cannot be used is told, and nothing changes", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/bad.pdf"]));
    await tool(page, "pt-insert").click();
    await expect(notice(page)).toContainText("could not be added");
    // The fault is the other file's, and it is named: the document is not called damaged.
    await expect(notice(page)).toContainText('"bad.pdf" could not be used');
    await expect(notice(page)).toContainText('"ten.pdf" was not changed');
    await expect(notice(page)).not.toContainText("damaged");
    await notice(page).getByRole("button", { name: "OK" }).click();
    expect((await labels(page, doc)).length).toBe(10);
    await expect(dirty(page)).toHaveCount(0);
  });

  test("a bad second file leaves the document open with the changes it has not saved", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 2);
    await page.keyboard.press("Delete");
    await expect.poll(async () => (await labels(page, doc)).length).toBe(9);
    await answerNote(page, /What a delete leaves/);
    await expect(dirty(page)).toBeVisible();
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/bad.pdf"]));
    await tool(page, "pt-insert").click();
    await expect(notice(page)).toContainText('"bad.pdf" could not be used');
    await notice(page).getByRole("button", { name: "OK" }).click();
    // The tab is still there, with its pages and its unsaved change.
    await expect(page.locator(".tab-title")).toHaveCount(1);
    expect((await labels(page, doc)).length).toBe(9);
    await expect(dirty(page)).toBeVisible();
  });

  test("a change in the middle has no Stop button, Escape does not end it, and nothing is lost", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 4);
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.pages.delay = 1500));
    await page.keyboard.press("Delete");
    const box = page.locator(".busy-box");
    await expect(box).toBeVisible();
    await expect(box).toContainText("cannot be stopped half way");
    await expect(box.getByRole("button")).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(box).toBeVisible();
    await expect.poll(async () => (await labels(page, doc)).length, { timeout: 8000 }).toBe(9);
    expect(await cancels(page)).toBe(0);
    await expect(page.locator(".tab-title")).toHaveCount(1);
    await expect(dirty(page)).toBeVisible();
  });

  test("while a change runs, the annotation tools wait and say why, and work again when it is done", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await page.locator("#annotate-button").click();
    await expect(page.locator("#tool-pen")).toBeEnabled();
    await press(page, 4);
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.pages.delay = 1500));
    await page.keyboard.press("Delete");
    await expect(page.locator(".busy-box")).toBeVisible();
    // The tools are off, the bar says why, and a tool that is pressed anyway is not picked.
    await expect(page.locator("#tool-pen")).toBeDisabled();
    await expect(page.locator("#annotbar")).toContainText("The pages of this document are being changed");
    await page.locator("#tool-pen").click({ force: true });
    await expect(page.locator("#tool-pen")).not.toHaveAttribute("aria-pressed", "true");
    await expect.poll(async () => (await labels(page, doc)).length, { timeout: 8000 }).toBe(9);
    await answerNote(page, /What a delete leaves/);
    await expect(page.locator("#tool-pen")).toBeEnabled();
    await expect(page.locator("#annotbar")).not.toContainText("The pages of this document are being changed");
    await page.locator("#tool-pen").click();
    await expect(page.locator("#tool-pen")).toHaveAttribute("aria-pressed", "true");
  });

  test("while a change runs, the other tabs, the keys and the menu answer, and the other document is saved and closed", async ({
    page,
  }) => {
    const ten = await openDoc(page, "/docs/ten.pdf");
    const second = await openDoc(page, "/docs/mixed.pdf");
    // Another document with a change that is not saved.
    await press(page, 0);
    await page.keyboard.press("Delete");
    await expect.poll(async () => (await labels(page, second)).length).toBe(4);
    await answerNote(page, /What a delete leaves/);
    await expect(dirty(page)).toBeVisible();

    await page.locator(".tab-title", { hasText: "ten.pdf" }).click();
    await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("ten.pdf");
    await press(page, 4);
    await page.evaluate((d) => {
      const stand = (window as unknown as { __E2E__: Stand }).__E2E__.pages;
      stand.delayOnly = d;
      stand.delay = 5000;
    }, ten);
    await page.keyboard.press("Delete");
    const box = page.locator(".busy-box");
    await expect(box).toBeVisible();
    await expect(box).toContainText("ten.pdf");
    await expect(page.locator("#notice-layer")).toBeHidden();

    // A real click on the other tab goes through, and so does the menu.
    await page.locator(".tab-title", { hasText: "mixed.pdf" }).click();
    await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("mixed.pdf");
    await page.locator("#menu-button").click();
    await expect(page.locator("#menu")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator("#menu")).toBeHidden();
    await expect(box).toBeVisible();

    // The keys act on the document in front: it is saved, and then closed.
    await page.keyboard.press("Control+s");
    await expect.poll(async () => (await saves(page)).map((s) => s.doc)).toEqual([second]);
    await expect(dirty(page)).toHaveCount(0);
    await expect(box).toBeVisible();
    await page.keyboard.press("Control+w");
    await expect(page.locator(".tab-title")).toHaveCount(1);
    await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("ten.pdf");
    await expect(page.locator(".sheet:not(.busy-box)")).toHaveCount(0);

    // The change itself went on through all of it and ends.
    await expect.poll(async () => (await labels(page, ten)).length, { timeout: 15000 }).toBe(9);
    await expect(box).toHaveCount(0);
    await answerNote(page, /What a delete leaves/);
  });

  test("a slow file shows progress and a Stop button that really stops, and the document is as it was", async ({
    page,
  }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/slow.pdf"]));
    await tool(page, "pt-insert").click();
    const box = page.locator(".busy-box");
    await expect(box).toBeVisible();
    await expect(box).toContainText("Adding pages");
    await expect(box.locator("progress")).toBeVisible();
    // The window is not held: the box sits at the foot of it, outside the layer of the message boxes, and the
    // tools of the bar wait.
    await expect(page.locator("#notice-layer")).toBeHidden();
    await expect(tool(page, "pt-delete")).toBeDisabled();
    await expect.poll(() => box.locator("progress").evaluate((p: HTMLProgressElement) => p.value)).toBeGreaterThan(0);
    await box.getByRole("button", { name: "Stop" }).click();
    await expect(box).toHaveCount(0);
    expect(await cancels(page)).toBe(1);
    expect((await labels(page, doc)).length).toBe(10);
    await expect(dirty(page)).toHaveCount(0);
    // Quiet: no error box, and the bar is usable again.
    await expect(page.locator(".sheet")).toHaveCount(0);
    await expect(tool(page, "pt-blank")).toBeEnabled();
  });

  test("a slow file that is let finish is added once", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/slow.pdf"]));
    await tool(page, "pt-insert").click();
    await expect.poll(async () => (await labels(page, doc)).length, { timeout: 8000 }).toBe(14);
    await expect(page.locator(".busy-box")).toHaveCount(0);
  });
});

test.describe("copying pages out", () => {
  test("the picked pages go to a new file, and the document is not changed", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 6);
    await press(page, 2, ["Control"]);
    await tool(page, "pt-extract").click();
    await expect(notice(page)).toContainText("The pages were copied");
    await expect(notice(page)).toContainText('"part.pdf"');
    await expect(notice(page)).toContainText("no password");
    await notice(page).getByRole("button", { name: "OK" }).click();
    expect(await extracts(page)).toEqual([{ doc, labels: [2, 6] }]);
    expect((await labels(page, doc)).length).toBe(10);
    await expect(dirty(page)).toHaveCount(0);
  });

  test("a protected document and a signed one ask before the new file loses the password or the signature", async ({
    page,
  }) => {
    const doc = await openDoc(page, "/docs/locked-doc.pdf");
    await press(page, 1);
    await tool(page, "pt-extract").click();
    await expect(notice(page)).toContainText("The new file will not be protected");
    await expect(notice(page)).toContainText("anyone who gets it can read it");
    await notice(page).getByRole("button", { name: "Cancel" }).click();
    expect(await extracts(page)).toEqual([]);
    await tool(page, "pt-extract").click();
    await notice(page).getByRole("button", { name: "Copy the pages anyway" }).click();
    await expect(notice(page)).toContainText("The pages were copied");
    await notice(page).getByRole("button", { name: "OK" }).click();
    expect(await extracts(page)).toEqual([{ doc, labels: [1] }]);

    await openDoc(page, "/docs/signed-doc.pdf");
    await press(page, 0);
    await tool(page, "pt-extract").click();
    await expect(notice(page)).toContainText("The pages come without the signature");
    await notice(page).getByRole("button", { name: "Copy the pages anyway" }).click();
    await expect(notice(page)).toContainText("The pages were copied");
  });
});

test.describe("saving", () => {
  test("a changed document is saved with the ordinary save, with the pages as they are now", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 1);
    await page.keyboard.press("Delete");
    await expect.poll(async () => (await labels(page, doc)).length).toBe(9);
    await answerNote(page, /What a delete leaves/);
    await press(page, 0);
    await page.keyboard.press("Alt+ArrowDown");
    await expect.poll(async () => (await labels(page, doc))[1]).toBe(0);
    await page.keyboard.press("Control+s");
    await expect(dirty(page)).toHaveCount(0);
    const written = await saves(page);
    expect(written).toHaveLength(1);
    expect(written[0].labels).toEqual([2, 0, 3, 4, 5, 6, 7, 8, 9]);
    // The window sent nothing else to write a file: only the save command of the app.
    const writers = await page.evaluate(() =>
      (window as unknown as { __E2E__: Stand }).__E2E__.pages.calls
        .map((c) => c.cmd)
        .filter((c) => /save|write|extract/.test(c) && c !== "save_session"),
    );
    expect(writers).toEqual(["save_pdf"]);
  });

  test("Ctrl+S during the first change of a clean document says that the change is under way, and saves nothing", async ({
    page,
  }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 4);
    await page.evaluate((d) => {
      const stand = (window as unknown as { __E2E__: Stand }).__E2E__.pages;
      stand.delayOnly = d;
      stand.delay = 12000;
    }, doc);
    await page.keyboard.press("Delete");
    await expect(page.locator(".busy-box")).toBeVisible();
    // Nothing is unsaved yet: the document turns dirty when the change ends.
    await expect(dirty(page)).toHaveCount(0);
    await page.keyboard.press("Control+s");
    await expect(notice(page)).toContainText('"ten.pdf" is being changed', { timeout: 3000 });
    expect(await saves(page)).toEqual([]);
    await notice(page).getByRole("button", { name: "OK" }).click();
    await expect.poll(async () => (await labels(page, doc)).length, { timeout: 15000 }).toBe(9);
    await answerNote(page, /What a delete leaves/);
    // When the change has ended, the save writes the document as it is.
    await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.pages.delay = 0));
    await press(page, 0);
    await page.keyboard.press("Control+s");
    await expect(dirty(page)).toHaveCount(0);
    const written = await saves(page);
    expect(written).toHaveLength(1);
    expect(written[0].labels).toEqual([0, 1, 2, 3, 5, 6, 7, 8, 9]);
  });

  test("taking the changes back makes the document clean again, and saving a signed document asks first", async ({
    page,
  }) => {
    const doc = await openDoc(page, "/docs/signed-doc.pdf");
    await press(page, 0);
    await tool(page, "pt-right").click();
    await expect(dirty(page)).toBeVisible();
    await page.keyboard.press("Control+s");
    await expect(notice(page)).toContainText("This document is signed");
    await notice(page).getByRole("button", { name: "Cancel" }).click();
    expect(await saves(page)).toEqual([]);
    await page.keyboard.press("Control+z");
    await expect(dirty(page)).toHaveCount(0);
    expect((await sizes(page, doc))[0]).toEqual([612, 792]);
  });
});

test.describe("when it goes wrong", () => {
  test("a refusal of the engine is told in plain words and changes nothing", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 2);
    await page.evaluate(() => {
      (window as unknown as { __E2E__: Stand }).__E2E__.pages.failNext = {
        cmd: "pages_delete",
        error: { code: "page_change_failed", message: "PDFium said no" },
      };
    });
    await page.keyboard.press("Delete");
    await expect(notice(page)).toContainText("The pages could not be changed");
    await expect(notice(page)).toContainText("was not changed");
    await notice(page).getByRole("button", { name: "OK" }).click();
    expect((await labels(page, doc)).length).toBe(10);
    await expect(dirty(page)).toHaveCount(0);
    // The panel works again.
    await page.locator("#thumbs .thumb.thumb").nth(2).focus();
    await page.keyboard.press("Delete");
    await expect.poll(async () => (await labels(page, doc)).length).toBe(9);
  });

  test("when the engine is lost the tab closes with the usual message", async ({ page }) => {
    await openDoc(page, "/docs/ten.pdf");
    await press(page, 2);
    await page.evaluate(() => {
      (window as unknown as { __E2E__: Stand }).__E2E__.pages.failNext = {
        cmd: "pages_rotate",
        error: { code: "engine_stopped", message: "gone" },
      };
    });
    await page.keyboard.press("r");
    await expect(notice(page)).toContainText("The PDF engine stopped");
    await expect(page.locator(".tab-title")).toHaveCount(0);
  });

  test("a change that cannot be taken back says so when the undo is tried", async ({ page }) => {
    const doc = await openDoc(page, "/docs/ten.pdf");
    await press(page, 2);
    await page.evaluate(() => {
      (window as unknown as { __E2E__: Stand }).__E2E__.pages.failNext = null;
    });
    await page.keyboard.press("Delete");
    await expect.poll(async () => (await labels(page, doc)).length).toBe(9);
    await answerNote(page, /What a delete leaves/);
    // The engine let go of the copy (the stand-in forgets it).
    await page.evaluate((d) => {
      (window as unknown as { __E2E__: Stand }).__E2E__.pages.failNext = {
        cmd: "pages_restore",
        error: { code: "snapshot_gone", message: "gone" },
      };
      void d;
    }, doc);
    await page.keyboard.press("Control+z");
    // The words for what happened (the engine let go of the copy), and nothing about being safe.
    await expect(notice(page)).toContainText("cannot be taken back any more");
    await expect(notice(page)).not.toContainText("Nothing was lost");
    await notice(page).getByRole("button", { name: "OK" }).click();
    // Still changed, and still dirty.
    expect((await labels(page, doc)).length).toBe(9);
    await expect(dirty(page)).toBeVisible();
  });
});

test("a document of thousands of pages keeps the panel light and its tools quick", async ({ page }) => {
  const doc = await openDoc(page, "/docs/many.pdf");
  await expect(page.locator("#page-total")).toHaveText("/ 3000");
  const mounted = () => page.locator("#thumbs .thumb").count();
  expect(await mounted()).toBeLessThan(30);
  await press(page, 0);
  await page.keyboard.press("Control+a");
  await expect(page.locator("#pt-status")).toHaveText("3,000 pages picked");
  // Keep the first page and delete the rest.
  await press(page, 0, ["Control"]);
  await expect(page.locator("#pt-status")).toHaveText("2,999 pages picked");
  await expect.poll(() => mounted()).toBeLessThan(30);
  await page.keyboard.press("Delete");
  await expect.poll(async () => (await labels(page, doc)).length).toBe(1);
  await expect(page.locator("#page-total")).toHaveText("/ 1");
  expect(await mounted()).toBeLessThan(30);
});

test("a document that the stand-in of the reader opened is read as it always was", async ({ page }) => {
  // Opening a document from the base stand-in (the made-up book) works as it always did: the page
  // panel still shows its pages and a click goes to the page.
  await page.locator("#empty-open").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("book.pdf");
  await page.locator("#sidebar-toggle").click();
  await expect(thumb(page, 2)).toBeAttached();
  await thumb(page, 2).click();
  await expect(page.locator("#page-input")).toHaveValue("3");
});


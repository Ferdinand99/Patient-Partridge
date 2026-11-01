import { expect, test, type Locator, type Page } from "@playwright/test";

// These tests run the real window code in a real browser, with the same content security policy as
// the app. The Rust side is replaced by e2e/fake-tauri.js, which serves two small made-up
// documents. Every test ends by checking that the policy blocked nothing.

type Stand = {
  calls: { cmd: string; args: Record<string, unknown> }[];
  dialog: string[];
  printed: number;
  violations: string[];
  title: string;
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

async function openBook(page: Page) {
  await page.locator("#empty-open").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("book.pdf");
  // The first page is drawn and its text and links have arrived.
  await expect(page.locator(".page-link").first()).toBeAttached();
}

/** A point on a page, given in points from its top left, as a position on the screen. */
async function onPage(page: Page, index: number, x: number, y: number) {
  const box = await page.locator(`.viewer-page[data-page="${index}"]`).boundingBox();
  if (!box) throw new Error("the page is not on screen");
  return { x: box.x + (x / 612) * box.width, y: box.y + (y / 792) * box.height };
}

async function drag(page: Page, from: { x: number; y: number }, to: { x: number; y: number }) {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 4 });
  await page.mouse.move(to.x, to.y, { steps: 4 });
  await page.mouse.up();
}

const darkness = (loc: Locator) =>
  loc.evaluate((el) => {
    const m = /rgba?\((\d+), (\d+), (\d+)/.exec(getComputedStyle(el).backgroundColor);
    return m ? (Number(m[1]) + Number(m[2]) + Number(m[3])) / 3 : -1;
  });

test("starts empty, in the Hedgerow look, with its own font", async ({ page }) => {
  await expect(page.locator("#empty")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Patient Partridge" })).toBeVisible();
  const font = await page.evaluate(async () => {
    await document.fonts.ready;
    return [...document.fonts].map((f) => `${f.family}:${f.status}`);
  });
  expect(font.some((f) => f.includes("InterVariable") && f.endsWith("loaded"))).toBe(true);
  // The open button is the inverted pill of the design system.
  const pill = await page.locator("#empty-open").evaluate((el) => getComputedStyle(el).borderRadius);
  expect(Number.parseFloat(pill)).toBeGreaterThan(100);
});

test("opens a document in a tab and draws its first page", async ({ page }) => {
  await openBook(page);
  await expect(page.locator("#page-total")).toHaveText("/ 3");
  await expect(page.locator("#page-input")).toHaveValue("1");
  expect(await page.evaluate(() => window.__E2E__.title)).toBe("book.pdf - Patient Partridge");
  // The canvas has dark pixels: the made-up text bars were drawn.
  await expect
    .poll(() =>
      page.locator(".viewer-canvas").first().evaluate((canvas: HTMLCanvasElement) => {
        const data = canvas.getContext("2d")?.getImageData(0, 0, canvas.width, canvas.height).data;
        if (!data) return 0;
        let dark = 0;
        for (let i = 0; i < data.length; i += 16) if (data[i] < 120) dark++;
        return dark;
      }),
    )
    .toBeGreaterThan(50);
});

test("follows the light or dark setting of the system, and the user can force one", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  expect(await darkness(page.locator("body"))).toBeLessThan(40);
  await page.emulateMedia({ colorScheme: "light" });
  expect(await darkness(page.locator("body"))).toBeGreaterThan(200);

  // Force dark while the system is light.
  await page.locator("#menu-button").click();
  await page.locator("#theme-choice label", { hasText: "Dark" }).click();
  expect(await darkness(page.locator("body"))).toBeLessThan(40);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");

  // The choice is there before the first paint after a reload.
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  expect(await darkness(page.locator("body"))).toBeLessThan(40);

  // Back to Auto: the page follows the system again.
  await page.locator("#menu-button").click();
  await page.locator("#theme-choice label", { hasText: "Auto" }).click();
  await expect(page.locator("html")).not.toHaveAttribute("data-theme", /.+/);
  expect(await darkness(page.locator("body"))).toBeGreaterThan(200);
});

test("keeps several documents in tabs", async ({ page }) => {
  await page.evaluate(() => (window.__E2E__.dialog = ["/docs/book.pdf", "/docs/second.pdf"]));
  await page.locator("#empty-open").click();
  await expect(page.locator(".tab-title")).toHaveText(["book.pdf", "second.pdf"]);
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("book.pdf");
  await page.locator(".tab-title", { hasText: "second.pdf" }).click();
  await expect(page.locator("#page-total")).toHaveText("/ 2");
  // Only the document in front is on screen.
  await expect(page.locator(".viewer:visible")).toHaveCount(1);
  await page.keyboard.press("Control+w");
  await expect(page.locator(".tab-title")).toHaveText(["book.pdf"]);
  await expect(page.locator("#page-total")).toHaveText("/ 3");
  await page.keyboard.press("Control+w");
  await expect(page.locator("#empty")).toBeVisible();
  expect((await calls(page, "close_pdf")).length).toBe(2);
});

test("shows thumbnails and the table of contents in the side panel", async ({ page }) => {
  await openBook(page);
  await page.keyboard.press("Control+b");
  await expect(page.locator("#sidebar")).toBeVisible();
  await expect(page.locator(".thumb")).toHaveCount(3);
  await expect
    .poll(() => page.locator(".thumb-canvas").evaluateAll((all) => (all as HTMLCanvasElement[]).filter((c) => c.width > 0).length))
    .toBe(3);
  await page.locator('.thumb[data-page="2"]').click();
  await expect(page.locator("#page-input")).toHaveValue("3");
  await expect(page.locator('.thumb[aria-current="page"]')).toHaveAttribute("data-page", "2");

  await page.locator("#side-tab-contents").click();
  await expect(page.locator(".outline-link")).toHaveText(["Chapter 1", "Section 1.1", "Chapter 3"]);
  // An entry that leads to the web is plain text.
  await expect(page.locator(".outline-text")).toHaveText("On the web");
  await page.locator(".outline-link", { hasText: "Section 1.1" }).click();
  await expect(page.locator("#page-input")).toHaveValue("2");
});

test("selects text with the mouse and copies it", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openBook(page);
  // From the start of the first line to the end of the second one.
  const start = await onPage(page, 0, 72 + 1, 105);
  const end = await onPage(page, 0, 72 + 6 * 42 - 1, 123);
  await drag(page, start, end);
  await expect(page.locator(".hl-select")).toHaveCount(2);
  await page.keyboard.press("Control+c");
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toBe("Hello Partridge\nA quick brown fox jumps over the lazy dog.");
  // Escape clears the selection.
  await page.keyboard.press("Escape");
  await expect(page.locator(".hl-select")).toHaveCount(0);
});

test("selects a word on a double click and everything with Ctrl+A", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await openBook(page);
  const word = await onPage(page, 0, 72 + 6 * 9, 105);
  await page.mouse.dblclick(word.x, word.y);
  await expect(page.locator(".hl-select")).toHaveCount(1);
  await page.keyboard.press("Control+c");
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe("Partridge");

  await page.keyboard.press("Control+a");
  await expect.poll(() => page.locator(".hl-select").count()).toBeGreaterThan(2);
  await page.keyboard.press("Control+c");
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toContain("The end of the book");
});

test("searches the document, highlights the matches and moves between them", async ({ page }) => {
  await openBook(page);
  await page.keyboard.press("Control+f");
  await expect(page.locator("#find-input")).toBeFocused();
  await page.keyboard.type("needle");
  await expect(page.locator("#find-status")).toHaveText("1 of 2");
  await expect(page.locator(".hl-find").first()).toBeAttached();
  await expect(page.locator(".hl-current")).toHaveCount(1);
  await page.keyboard.press("Enter");
  await expect(page.locator("#find-status")).toHaveText("2 of 2");
  await expect(page.locator("#page-input")).toHaveValue("2");
  await page.keyboard.press("Shift+Enter");
  await expect(page.locator("#find-status")).toHaveText("1 of 2");
  await page.keyboard.press("Escape");
  await expect(page.locator("#findbar")).toBeHidden();
  await expect(page.locator(".hl-find")).toHaveCount(0);

  await page.keyboard.press("Control+f");
  await page.keyboard.type("zebra");
  await expect(page.locator("#find-status")).toHaveText("No matches");
});

test("asks before a link is opened, and only opens web addresses", async ({ page }) => {
  await openBook(page);
  const links = page.locator(".page-link");
  await expect(links).toHaveCount(3);

  // A web address: the whole address is shown, Cancel has the focus, and nothing is opened yet.
  await links.nth(0).click();
  await expect(page.getByRole("alertdialog")).toContainText("Open this link in your browser?");
  await expect(page.locator(".sheet-url")).toHaveText("https://example.com/hello?from=pdf");
  await expect(page.getByRole("button", { name: "Cancel" })).toBeFocused();
  expect((await calls(page, "open_link")).length).toBe(0);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  expect((await calls(page, "open_link")).length).toBe(0);

  // Yes: now the Rust side is asked, with the address and the document.
  await links.nth(0).click();
  await page.getByRole("button", { name: "Open in browser" }).click();
  await expect.poll(async () => (await calls(page, "open_link")).length).toBe(1);
  expect((await calls(page, "open_link"))[0].args.url).toBe("https://example.com/hello?from=pdf");

  // A file link is explained and never offered.
  await links.nth(1).click();
  await expect(page.getByRole("alertdialog")).toContainText("This link is not opened");
  await expect(page.getByRole("button", { name: "Open in browser" })).toHaveCount(0);
  await page.getByRole("button", { name: "OK", exact: true }).click();
  expect((await calls(page, "open_link")).length).toBe(1);

  // A link inside the document just goes there.
  await links.nth(2).click();
  await expect(page.locator("#page-input")).toHaveValue("3");
});

test("prints: asks for the pages, prepares pictures, and shows only them on paper", async ({ page }) => {
  await openBook(page);
  await page.keyboard.press("Control+p");
  await expect(page.getByRole("dialog", { name: "Print" })).toBeVisible();
  await page.getByRole("dialog", { name: "Print" }).getByRole("button", { name: "Print", exact: true }).click();
  await expect.poll(() => page.evaluate(() => window.__E2E__.printed)).toBe(1);
  // The pictures are blob: pictures, which the policy allows and nothing else.
  const sources = await page.locator(".print-page").evaluateAll((all) => all.map((i) => (i as HTMLImageElement).src));
  expect(sources).toHaveLength(3);
  expect(sources.every((s) => s.startsWith("blob:"))).toBe(true);

  await page.emulateMedia({ media: "print" });
  await expect(page.locator(".app")).toBeHidden();
  await expect(page.locator(".print-page").first()).toBeVisible();
  const loaded = await page.locator(".print-page").evaluateAll((all) => all.every((i) => (i as HTMLImageElement).naturalWidth > 0));
  expect(loaded).toBe(true);
  await page.emulateMedia({ media: "screen" });
  await expect(page.locator(".app")).toBeVisible();
  // When the system says printing is over, the pictures go.
  await page.evaluate(() => window.dispatchEvent(new Event("afterprint")));
  await expect(page.locator(".print-page")).toHaveCount(0);
});

test("a file that cannot be opened is explained and the app carries on", async ({ page }) => {
  await page.evaluate(() => (window.__E2E__.dialog = ["/docs/nothing.pdf"]));
  await page.locator("#empty-open").click();
  await expect(page.getByRole("alertdialog")).toContainText("cannot be opened");
  await page.getByRole("button", { name: "OK", exact: true }).click();
  await expect(page.locator("#empty")).toBeVisible();
  await page.evaluate(() => (window.__E2E__.dialog = ["/docs/book.pdf"]));
  await page.locator("#empty-open").click();
  await expect(page.locator(".tab-title")).toHaveText(["book.pdf"]);
});

test("selects a whole line of right-to-left text from one end to the other, and copies all of it", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.evaluate(() => (window.__E2E__.dialog = ["/docs/rtl.pdf"]));
  await page.locator("#empty-open").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("rtl.pdf");
  await expect.poll(async () => (await calls(page, "get_page_text")).length).toBeGreaterThan(0);
  // From beyond the right end of the first line to beyond its left end, as a reader drags it.
  const start = await onPage(page, 0, 440, 85);
  const end = await onPage(page, 0, 270, 85);
  await drag(page, start, end);
  await expect(page.locator(".hl-select")).toHaveCount(1);
  await page.keyboard.press("Control+c");
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toBe("אבגדהוזחטי");
  // And the other way round, and the second line too.
  await page.keyboard.press("Escape");
  await drag(page, await onPage(page, 0, 270, 125), await onPage(page, 0, 440, 125));
  await page.keyboard.press("Control+c");
  await expect
    .poll(() => page.evaluate(() => navigator.clipboard.readText()))
    .toBe("וזחטי");
});

test("the second press of a double click cannot answer the question about a link", async ({ page }) => {
  await openBook(page);
  await page.locator(".page-link").first().click();
  const open = page.getByRole("button", { name: "Open in browser" });
  await expect(open).toBeDisabled();
  const box = await open.boundingBox();
  if (!box) throw new Error("the button is not on screen");
  // The second press, a moment later, lands on the button.
  await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
  await expect(page.getByRole("alertdialog")).toBeVisible();
  expect((await calls(page, "open_link")).length).toBe(0);
  // After the delay the same press is an answer.
  await expect(open).toBeEnabled();
  await open.click();
  await expect.poll(async () => (await calls(page, "open_link")).length).toBe(1);
});

test("does not let the window be reloaded, which would orphan the engines of its documents", async ({ page }) => {
  await openBook(page);
  const seen = await page.evaluate(() => {
    const log: { key: string; prevented: boolean }[] = [];
    // Listens after the app, in the bubbling phase: what it sees is what the app left of the event.
    window.addEventListener("keydown", (e) => log.push({ key: e.key, prevented: e.defaultPrevented }));
    window.addEventListener("contextmenu", (e) => log.push({ key: "contextmenu", prevented: e.defaultPrevented }));
    (window as unknown as { __log: typeof log }).__log = log;
    return true;
  });
  expect(seen).toBe(true);
  await page.keyboard.press("F5");
  await page.keyboard.press("Control+r");
  await page.keyboard.press("Control+Shift+r");
  await page.mouse.click(300, 300, { button: "right" });
  const log = await page.evaluate(() => (window as unknown as { __log: { key: string; prevented: boolean }[] }).__log);
  const named = (...keys: string[]) => log.filter((e) => keys.includes(e.key));
  expect(named("F5").length).toBe(1);
  expect(named("r", "R").length).toBe(2);
  expect(named("contextmenu").length).toBe(1);
  expect(named("F5", "r", "R", "contextmenu").every((e) => e.prevented), JSON.stringify(log)).toBe(true);
  // The tab is still there: nothing was reloaded.
  await expect(page.locator(".tab-title")).toHaveText(["book.pdf"]);
});

test("takes screenshots in light and dark (nothing is checked about the look)", async ({ page }) => {
  await openBook(page);
  await page.keyboard.press("Control+b");
  await expect(page.locator(".thumb-canvas").first()).toBeAttached();
  await page.keyboard.press("Control+f");
  await page.keyboard.type("needle");
  await expect(page.locator("#find-status")).toHaveText("1 of 2");
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    // Let the colour transitions of the buttons finish before the picture is taken.
    await page.waitForTimeout(500);
    await page.screenshot({ path: `e2e/artifacts/reader-${scheme}.png` });
  }
});

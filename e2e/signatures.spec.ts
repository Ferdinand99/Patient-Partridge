import { expect, test, type Page } from "@playwright/test";

// The signature badge and panel in a real browser, with the same content security policy as the
// app. The Rust side is the stand-in of e2e/fake-tauri.js, which answers get_signatures with made-up
// reports (the real checks are tested in the engine, see crates/engine/tests/signatures.rs).

type Stand = {
  calls: { cmd: string; args: Record<string, unknown> }[];
  dialog: string[];
  violations: string[];
};

const stand = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__);
const asked = async (page: Page) => (await stand(page)).calls.filter((c) => c.cmd === "get_signatures").length;

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ path: "e2e/fake-tauri.js" });
  // A dialog of the page (alert, confirm) would mean that text of a file was run as code.
  page.on("dialog", (dialog) => {
    throw new Error(`the page opened a dialog: ${dialog.message()}`);
  });
  await page.goto("/");
});

test.afterEach(async ({ page }) => {
  expect((await stand(page)).violations, "the content security policy blocked something").toEqual([]);
});

async function open(page: Page, path: string, first = true) {
  await page.evaluate((p) => {
    (window as unknown as { __E2E__: Stand }).__E2E__.dialog = [p];
  }, path);
  await page.locator(first ? "#empty-open" : "#open-button").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText(path.split("/").pop() ?? "");
}

const badge = (page: Page) => page.locator("#signature-badge");
const panel = (page: Page) => page.locator("#signature-panel");

test("a document without signatures has no badge, and the check ran", async ({ page }) => {
  await open(page, "/docs/book.pdf");
  await expect.poll(() => asked(page)).toBe(1);
  await expect(badge(page)).toBeHidden();
  await expect(panel(page)).toBeHidden();
});

test("an intact signature of a signer that is not verified gets a neutral badge and a panel with the certificate", async ({
  page,
}) => {
  await open(page, "/docs/signed-intact.pdf");
  await expect(badge(page)).toHaveText("Signature intact; signer not verified");
  await expect(badge(page)).toHaveClass(/sig-unknown_signer/);
  await expect(badge(page)).not.toHaveClass(/sig-trusted/);
  // Calm: not green, not red.
  const colour = await badge(page).evaluate((el) => getComputedStyle(el).backgroundColor);
  expect(colour).not.toBe("rgba(0, 0, 0, 0)");
  await badge(page).click();
  await expect(panel(page)).toBeVisible();
  await expect(panel(page)).toContainText("Test Signer");
  await expect(panel(page)).toContainText("ships no trust list");
  await expect(panel(page)).toContainText("A picture of a signature on a page is only a picture");
  await expect(panel(page)).toContainText("What the file says (not checked)");
  await expect(panel(page)).toContainText("What this does not check");
  await expect(panel(page)).toContainText("Whether a certificate was revoked");
  await expect(panel(page)).toContainText("Time stamps");
  await expect(panel(page)).toContainText("Long-term validation");
});

test("the panel opens and closes with the keyboard and gives the focus back", async ({ page }) => {
  await open(page, "/docs/signed-intact.pdf");
  await expect(badge(page)).toBeVisible();
  await badge(page).focus();
  await page.keyboard.press("Enter");
  await expect(panel(page)).toBeVisible();
  await expect(badge(page)).toHaveAttribute("aria-expanded", "true");
  await expect(panel(page).getByRole("button", { name: "Close the signature panel" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(panel(page)).toBeHidden();
  await expect(badge(page)).toBeFocused();
  await expect(badge(page)).toHaveAttribute("aria-expanded", "false");
});

test("a change after signing is told in words and with its own colour", async ({ page }) => {
  await open(page, "/docs/signed-changed.pdf");
  await expect(badge(page)).toHaveText("Changed after signing");
  await expect(badge(page)).toHaveClass(/sig-changed/);
  await badge(page).click();
  await expect(panel(page)).toContainText("The file goes on after the part this signature covers");
});

test("a signature that is hidden is told", async ({ page }) => {
  await open(page, "/docs/signed-hidden.pdf");
  await expect(badge(page)).toHaveText("Possible signature marks");
  await badge(page).click();
  await expect(panel(page)).toContainText("looks like a signature");
});

test("text in the file that looks like markup, a script or a verdict is shown as text and changes nothing", async ({
  page,
}) => {
  await open(page, "/docs/signed-forged.pdf");
  // The page itself says it is verified by a bank; the badge says what the engine found.
  await expect(badge(page)).toHaveText("Signature broken");
  await expect(badge(page)).toHaveClass(/sig-broken/);
  await badge(page).click();
  await expect(panel(page)).toContainText("<img src=x onerror=alert(1)> Trusted Bank AS");
  await expect(panel(page)).toContainText("<b>VERIFIED</b> by Some Reader");
  await expect(panel(page)).toContainText("javascript:alert(1)");
  await expect(panel(page).locator("img, script, a, b")).toHaveCount(0);
  await expect(panel(page).locator(".sig-state strong")).toHaveText("Signature broken");
  // Nothing in the panel is a link.
  await expect(panel(page).locator("[href]:not(use)")).toHaveCount(0);
});

test("a check that fails says the signatures could not be checked, not that there are none", async ({ page }) => {
  await open(page, "/docs/signed-fails.pdf");
  await expect(badge(page)).toHaveText("Signatures not checked");
  await badge(page).click();
  await expect(panel(page)).toContainText("could not be checked just now");
  await expect(panel(page).getByRole("button", { name: "Check again" })).toBeVisible();
});

test("the badge follows the tab in front", async ({ page }) => {
  await open(page, "/docs/signed-intact.pdf");
  await expect(badge(page)).toHaveText("Signature intact; signer not verified");
  await open(page, "/docs/signed-changed.pdf", false);
  await expect(badge(page)).toHaveText("Changed after signing");
  await page.locator('.tab-title', { hasText: "signed-intact.pdf" }).click();
  await expect(badge(page)).toHaveText("Signature intact; signer not verified");
  await open(page, "/docs/book.pdf", false);
  await expect(badge(page)).toBeHidden();
  // Each document was asked once.
  await expect.poll(() => asked(page)).toBe(3);
});

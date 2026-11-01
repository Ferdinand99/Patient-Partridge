import { expect, test, type Page } from "@playwright/test";

// The tabs of the last session come back when the window starts again. The Rust side is the
// stand-in of e2e/fake-tauri.js, which keeps the list in the session storage of the page, so it
// survives a reload the way the file of the real app survives a restart.

type Stand = {
  calls: { cmd: string; args: Record<string, unknown> }[];
  dialog: string[];
  violations: string[];
};

const stand = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__);

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ path: "e2e/fake-tauri.js" });
  await page.goto("/");
});

test.afterEach(async ({ page }) => {
  expect((await stand(page)).violations, "the content security policy blocked something").toEqual([]);
});

const tabs = (page: Page) => page.locator(".tab-title");
const selected = (page: Page) => page.locator('.tab-title[aria-selected="true"]');

test("the tabs come back in order with the same one in view, and a missing or protected file is told once", async ({
  page,
}) => {
  await page.locator("#empty-open").click();
  await expect(selected(page)).toHaveText("book.pdf");
  await page.evaluate(() => {
    (window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/second.pdf"];
  });
  await page.locator("#open-button").click();
  await expect(selected(page)).toHaveText("second.pdf");

  // Since then two files were deleted and one was protected with a password.
  await page.evaluate(() => {
    const stored = JSON.parse(sessionStorage.getItem("e2e-session") ?? "{}") as { paths: string[]; active: number };
    const paths = ["/docs/book.pdf", "/docs/gone.pdf", "/docs/locked.pdf", "/docs/second.pdf", "/docs/gone2.pdf"];
    sessionStorage.setItem("e2e-session", JSON.stringify({ paths, active: paths.indexOf("/docs/second.pdf") }));
    if (stored.paths.length !== 2) throw new Error("the window did not write the list of tabs");
  });

  await page.reload();

  await expect(tabs(page)).toHaveText(["book.pdf", "second.pdf"]);
  await expect(selected(page)).toHaveText("second.pdf");
  // One notice for all of them, and no question about the password.
  const notice = page.getByRole("alertdialog");
  await expect(notice).toHaveCount(1);
  await expect(notice).toContainText("2 tabs from your last session could not be opened");
  await expect(notice).toContainText("1 password protected file was left closed");
  await expect(page.locator('input[type="password"]')).toHaveCount(0);
  await notice.getByRole("button", { name: "OK" }).click();

  // Turned off: the list is gone at once, and nothing is kept while it stays off.
  await page.locator("#menu-button").click();
  await expect(page.getByRole("menuitemcheckbox", { name: "Reopen tabs at start" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
  await page.getByRole("menuitemcheckbox", { name: "Reopen tabs at start" }).click();
  await expect(page.getByRole("menuitemcheckbox", { name: "Reopen tabs at start" })).toHaveAttribute(
    "aria-checked",
    "false",
  );
  await tabs(page).first().click();
  await page.reload();
  await expect(page.locator("#empty")).toBeVisible();
  await expect(tabs(page)).toHaveCount(0);
});

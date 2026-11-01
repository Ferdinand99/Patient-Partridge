import { expect, test, type Page } from "@playwright/test";

// The update pill, the question, the progress and the restart, in a real browser with the policy of
// the app. The Rust side is the stand-in of e2e/fake-tauri.js, which says what the next check finds.

type UpdateStand = {
  settings: { autoUpdate: boolean; method: string; currentVersion: string };
  found: unknown;
  checkError: unknown;
  installError: unknown;
  installDelay: number;
  restarted: number;
  releasesPageOpened: number;
};

type Stand = {
  calls: { cmd: string; args: Record<string, unknown> }[];
  violations: string[];
  update: UpdateStand;
};

/** The stand-in of the Rust side, as the page sees it. */
const stand = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__);

const found = {
  version: "0.2.0",
  currentVersion: "0.1.0",
  notes:
    "## [0.2.0](https://example.com/compare) (2026-11-02)\n\n### Features\n\n* **viewer:** faster pages ([#12](https://example.com/12)) ([abcdef0](https://example.com/c))\n* a <img src=x onerror=alert(1)> note",
  method: "automatic",
};

const requested: string[] = [];

test.beforeEach(async ({ page }) => {
  requested.length = 0;
  page.on("request", (r) => requested.push(r.url()));
  await page.addInitScript({ path: "e2e/fake-tauri.js" });
  await page.goto("/");
});

test.afterEach(async ({ page }) => {
  const violations = (await stand(page)).violations;
  expect(violations, "the content security policy blocked something").toEqual([]);
  // The window itself loads nothing from the internet, also while an update is offered.
  const outside = requested.filter((u) => !/^(http:\/\/127\.0\.0\.1:\d+\/|data:|blob:)/.test(u));
  expect(outside, "the window asked for something outside the app").toEqual([]);
});

async function setUpdate(page: Page, patch: Partial<UpdateStand>) {
  await page.evaluate((p) => Object.assign((window as unknown as { __E2E__: Stand }).__E2E__.update, p), patch);
}

const calls = (page: Page, cmd: string) =>
  page.evaluate(
    (name) => (window as unknown as { __E2E__: Stand }).__E2E__.calls.filter((c) => c.cmd === name),
    cmd,
  );

async function checkFromMenu(page: Page) {
  await page.locator("#menu-button").click();
  await page.getByRole("menuitem", { name: "Check for Updates…" }).click();
}

test("Check for Updates tells the user that nothing new exists", async ({ page }) => {
  await checkFromMenu(page);
  await expect(page.getByRole("alertdialog")).toContainText("Patient Partridge is up to date");
  await expect(page.getByRole("alertdialog")).toContainText("version 0.1.0");
  const manual = (await calls(page, "check_update")).map((c) => c.args);
  expect(manual).toContainEqual({ manual: true });
  await page.getByRole("button", { name: "OK" }).click();
  await expect(page.locator("#update-pill")).toBeHidden();
});

test("an update shows the notes, downloads with progress and asks for the restart", async ({ page }) => {
  // A document is open so that the install can hand it over for the restart.
  await page.locator("#empty-open").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("book.pdf");
  await setUpdate(page, { found, installDelay: 600 });

  await checkFromMenu(page);
  const sheet = page.getByRole("alertdialog");
  await expect(sheet).toContainText("Version 0.2.0 is available");
  await expect(sheet.locator(".update-notes")).toContainText("• viewer: faster pages (#12)");
  await expect(sheet.locator(".update-notes")).toContainText("a  note");
  await expect(sheet.locator("img")).toHaveCount(0);
  await page.screenshot({ path: "e2e/artifacts/update-question.png" });

  await sheet.getByRole("button", { name: "Download and Install" }).click();
  await expect(page.getByRole("alertdialog")).toContainText("Downloading version 0.2.0");
  await expect(page.locator("progress")).toBeVisible();
  await expect(page.locator("#update-pill")).toContainText("Updating");
  await expect(page.getByRole("alertdialog")).toContainText("Installing");
  await page.screenshot({ path: "e2e/artifacts/update-progress.png" });

  await expect(page.getByRole("alertdialog")).toContainText("Restart to update");
  const [install] = await calls(page, "install_update");
  expect(install.args.paths).toEqual(["/docs/book.pdf"]);
  await expect(page.locator("#update-pill")).toContainText("Restart to update");
  await page.getByRole("alertdialog").getByRole("button", { name: "Restart to Update" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.update.restarted)).toBe(1);
});

test("Later keeps a small pill that opens the question again", async ({ page }) => {
  await setUpdate(page, { found });
  await checkFromMenu(page);
  await page.getByRole("button", { name: "Later" }).click();
  const pill = page.locator("#update-pill");
  await expect(pill).toBeVisible();
  await expect(pill).toContainText("Update 0.2.0");
  await pill.click();
  await expect(page.getByRole("alertdialog")).toContainText("Version 0.2.0 is available");
});

test("a failed install after the engines stopped offers a restart", async ({ page }) => {
  await setUpdate(page, {
    found,
    installError: { kind: "other", message: "no room on the disk", enginesStopped: true },
  });
  await checkFromMenu(page);
  await page.getByRole("button", { name: "Download and Install" }).click();
  await expect(page.getByRole("alertdialog")).toContainText("The update could not be installed");
  await expect(page.getByRole("alertdialog")).toContainText("no room on the disk");
  await page.getByRole("button", { name: "Restart", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.update.restarted)).toBe(1);
});

test("a package install gets a notice and a link, not an install button", async ({ page }) => {
  await setUpdate(page, { found: { ...found, method: "manual" } });
  await checkFromMenu(page);
  const sheet = page.getByRole("alertdialog");
  await expect(sheet).toContainText("cannot update itself");
  await expect(sheet.getByRole("button", { name: "Download and Install" })).toHaveCount(0);
  await sheet.getByRole("button", { name: "Open the releases page" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.update.releasesPageOpened)).toBe(1);
  expect(await calls(page, "install_update")).toEqual([]);
});

test("the switch for automatic checks is saved", async ({ page }) => {
  await page.locator("#menu-button").click();
  const toggle = page.getByRole("menuitemcheckbox", { name: "Check for updates automatically" });
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  expect((await calls(page, "set_auto_update")).map((c) => c.args)).toEqual([{ enabled: false }]);
  await page.screenshot({ path: "e2e/artifacts/update-menu.png" });
});

test("an update found by the check at the start only adds the pill", async ({ page }) => {
  await setUpdate(page, { found });
  // The automatic check runs a few seconds after the start.
  await expect(page.locator("#update-pill")).toBeVisible({ timeout: 9000 });
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  const checks = (await calls(page, "check_update")).map((c) => c.args);
  expect(checks).toContainEqual({ manual: false });
});

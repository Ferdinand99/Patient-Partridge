import { expect, test, type Page } from "@playwright/test";

// Signing with a certificate in a real browser with the policy of the app. The Rust side is the
// stand-in of e2e/fake-tauri.js: it opens any certificate file with the password "test", lists the
// signature fields it is told about, and "signs" into a file named like the document with
// " (signed)" before the ending. The real work is tested in the signer, the engine and the app
// process; here the window is looked at: what it says, what it asks for, and what it hands over.

type Call = { cmd: string; args: Record<string, unknown> };
type Stand = {
  calls: Call[];
  dialog: string[];
  violations: string[];
  cert: {
    picked: boolean;
    loadFailure: { code: string; message: string } | null;
    signFailure: { code: string; message: string } | null;
    cancelSave: boolean;
    fields: { page: number; rect: number[]; name: string }[];
    signed: { doc: number; args: Record<string, unknown>; path: string }[];
    forgotten: number;
  };
};

const errors: string[] = [];

test.beforeEach(async ({ page }) => {
  errors.length = 0;
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  page.on("dialog", (dialog) => {
    throw new Error(`the page opened a dialog: ${dialog.message()}`);
  });
  await page.addInitScript({ path: "e2e/fake-tauri.js" });
  await page.goto("/");
});

test.afterEach(async ({ page }) => {
  expect((await stand(page)).violations, "the content security policy blocked something").toEqual([]);
  expect(errors, "the window reported an error").toEqual([]);
});

const stand = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__);
const calls = async (page: Page, cmd: string) => (await stand(page)).calls.filter((c) => c.cmd === cmd);

async function openBook(page: Page) {
  await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/book.pdf"]));
  await page.locator("#empty-open, #open-button").first().click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("book.pdf");
  await expect(page.locator(".viewer-page canvas").first()).toBeAttached();
}

const sheet = (page: Page) => page.locator("form.certsign");

async function openDialog(page: Page) {
  await page.locator("#menu-button").click();
  await page.locator("#sign-button").click();
  await expect(sheet(page)).toBeVisible();
}

async function openCertificate(page: Page, password = "test") {
  await sheet(page).getByRole("button", { name: "Choose certificate file…" }).click();
  await expect(sheet(page).locator(".certsign-file")).toHaveText("anna.p12");
  await sheet(page).locator('input[type="password"]').fill(password);
  await sheet(page).getByRole("button", { name: "Open the file" }).click();
}

/** A drag over the preview of the page, given as fractions of its width and height. */
async function drag(page: Page, from: [number, number], to: [number, number]) {
  const canvas = sheet(page).locator("canvas.certsign-preview");
  await expect(canvas).toBeVisible();
  // The box of the dialog scrolls; the whole preview has to be on the screen for the mouse.
  await canvas.scrollIntoViewIfNeeded();
  const box = await canvas.boundingBox();
  if (!box) throw new Error("the preview is not on screen");
  const at = (f: [number, number]) => ({ x: box.x + box.width * f[0], y: box.y + box.height * f[1] });
  await page.mouse.move(at(from).x, at(from).y);
  await page.mouse.down();
  await page.mouse.move(at(to).x, at(to).y, { steps: 5 });
  await page.mouse.up();
}

test("the entry is there for an open document, and the box says what it does and asks for a file first", async ({ page }) => {
  await page.locator("#menu-button").click();
  await expect(page.locator("#sign-button")).toBeDisabled();
  await page.keyboard.press("Escape");
  await openBook(page);
  await page.locator("#menu-button").click();
  await expect(page.locator("#sign-button")).toBeEnabled();
  await page.keyboard.press("Escape");
  await openDialog(page);
  await expect(sheet(page)).toContainText("does not say whether anyone will accept the signature");
  await expect(sheet(page).locator('input[type="password"]')).toBeDisabled();
  await expect(sheet(page).getByRole("button", { name: "Sign and save a copy…" })).toBeDisabled();
  // Nothing was asked of the app process yet.
  expect(await calls(page, "sign_document")).toHaveLength(0);
});

test("a wrong password is told, the right one opens the file, and the certificate is shown as text", async ({ page }) => {
  await openBook(page);
  await openDialog(page);
  await openCertificate(page, "wrong");
  await expect(sheet(page).locator(".certsign-status")).toContainText("That password is not right");
  await expect(sheet(page).locator('input[type="password"]')).toBeEnabled();
  await sheet(page).locator('input[type="password"]').fill("test");
  await sheet(page).getByRole("button", { name: "Open the file" }).click();
  const summary = sheet(page).locator(".certsign-summary");
  await expect(summary).toContainText("Anna Alvorlig");
  await expect(summary).toContainText("ECDSA P-256");
  await expect(summary).toContainText("2020-01-01 to 2120-01-01");
  // The password field is emptied and shut; the clock and the rule are said.
  await expect(sheet(page).locator('input[type="password"]')).toHaveValue("");
  await expect(sheet(page)).toContainText("taken from this computer's clock");
  await expect(sheet(page)).toContainText("After signing, changing the file breaks the signature");
  await expect(sheet(page).locator(".certsign-warnings")).toBeHidden();
});

test("a certificate that its owner made and that has expired says so in one line each", async ({ page }) => {
  await openBook(page);
  await page.evaluate(() => {
    const c = (window as unknown as { __E2E__: { cert: { certificate: Record<string, unknown> } } }).__E2E__.cert.certificate;
    c.selfSigned = true;
    c.notAfter = 1600000000;
  });
  await openDialog(page);
  await openCertificate(page);
  const lines = sheet(page).locator(".certsign-warnings li");
  await expect(lines).toHaveCount(2);
  await expect(lines.nth(0)).toHaveText(
    "This certificate was made by its owner. Programs that open the file will usually show the signer as unknown.",
  );
  await expect(lines.nth(1)).toContainText("has expired");
  // Neither stops the signing.
  await sheet(page).locator('input[type="radio"][value="none"]').check();
  await expect(sheet(page).getByRole("button", { name: "Sign and save a copy…" })).toBeEnabled();
});

test("a box is dragged on the page, the signature is made, and the signed copy opens with editing off", async ({ page }) => {
  await openBook(page);
  await openDialog(page);
  await openCertificate(page);
  const sign = sheet(page).getByRole("button", { name: "Sign and save a copy…" });
  await expect(sign).toBeDisabled();
  await drag(page, [0.2, 0.7], [0.8, 0.8]);
  await expect(sheet(page).locator(".certsign-box")).toBeVisible();
  await expect(sign).toBeEnabled();
  await sheet(page).getByLabel("Reason (optional)").fill("I have read this");
  await sheet(page).getByLabel("Place (optional)").fill("Oslo");
  await sign.click();

  // What the window handed over: the place, the texts, no picture.
  await expect.poll(async () => (await stand(page)).cert.signed.length).toBe(1);
  const made = (await stand(page)).cert.signed[0].args as {
    place: { kind: string; page: number; rect: number[] };
    name: string;
    reason: string;
    location: string;
    mark: { kind: string };
  };
  expect(made.place.kind).toBe("rect");
  expect(made.place.page).toBe(0);
  const [l, t, r, b] = made.place.rect;
  expect(r - l).toBeGreaterThan(24);
  expect(b - t).toBeGreaterThan(24);
  expect(l).toBeGreaterThan(100);
  expect(made).toMatchObject({ name: "Anna Alvorlig", reason: "I have read this", location: "Oslo", mark: { kind: "none" } });

  // The signed copy is opened in a tab of its own and the person is told, in the words of the panel.
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("book (signed).pdf");
  const told = page.locator(".notice").last();
  await expect(told).toContainText('Signed copy saved as "book (signed).pdf". Signature intact; signer not verified.');
  await expect(told).toContainText("Patient Partridge does not check certificates against a list of trusted issuers.");
  await expect(told).toContainText("editing is off here");
  await told.getByRole("button", { name: "OK" }).click();
  // The certificate was let go by the app process (the stand-in counts the forget only on a cancel).
  expect(await calls(page, "sign_forget_certificate")).toHaveLength(0);
  await expect(page.locator("#signature-badge")).toBeVisible();
});

test("a signature field of the file can be chosen, and none is offered when the file has none", async ({ page }) => {
  await openBook(page);
  await openDialog(page);
  await openCertificate(page);
  await expect(sheet(page).locator('input[type="radio"][value="field"]')).toBeDisabled();
  await sheet(page).getByRole("button", { name: "Cancel" }).click();
  await expect(sheet(page)).toHaveCount(0);

  await page.evaluate(() => {
    (window as unknown as { __E2E__: Stand }).__E2E__.cert.fields = [{ page: 1, rect: [72, 600, 272, 660], name: "approval" }];
  });
  await openDialog(page);
  await openCertificate(page);
  const radio = sheet(page).locator('input[type="radio"][value="field"]');
  await expect(radio).toBeEnabled();
  await radio.check();
  await expect(sheet(page).locator("select")).toHaveValue("approval");
  await sheet(page).getByRole("button", { name: "Sign and save a copy…" }).click();
  await expect.poll(async () => (await stand(page)).cert.signed.length).toBe(1);
  expect((await stand(page)).cert.signed[0].args).toMatchObject({ place: { kind: "field", page: 1, name: "approval" } });
});

test("a refusal is told in words and the box stays for another try", async ({ page }) => {
  await openBook(page);
  await openDialog(page);
  await openCertificate(page);
  await sheet(page).locator('input[type="radio"][value="none"]').check();
  await page.evaluate(() => {
    (window as unknown as { __E2E__: Stand }).__E2E__.cert.signFailure = { code: "sign_already_signed", message: "x" };
  });
  await sheet(page).getByRole("button", { name: "Sign and save a copy…" }).click();
  await expect(sheet(page).locator(".certsign-status")).toContainText("This file already has signatures");
  await expect(sheet(page).locator(".certsign-status")).toContainText(
    "Signing it again would remove them, so Patient Partridge does not do it in this version.",
  );
  await expect(sheet(page).getByRole("button", { name: "Sign and save a copy…" })).toBeEnabled();
  // The person gives the dialog for the name of the copy up: nothing is signed and the box stays.
  await page.evaluate(() => {
    (window as unknown as { __E2E__: Stand }).__E2E__.cert.cancelSave = true;
  });
  await sheet(page).getByRole("button", { name: "Sign and save a copy…" }).click();
  await expect(sheet(page).locator(".certsign-status")).toBeHidden();
  await expect(sheet(page)).toBeVisible();
  expect((await stand(page)).cert.signed).toHaveLength(0);
});

test("cancelling or pressing Escape lets the key go and closes the box", async ({ page }) => {
  await openBook(page);
  await openDialog(page);
  await openCertificate(page);
  await sheet(page).getByRole("button", { name: "Cancel" }).click();
  await expect(sheet(page)).toHaveCount(0);
  await expect.poll(async () => (await stand(page)).cert.forgotten).toBe(1);
  await openDialog(page);
  await page.keyboard.press("Escape");
  await expect(sheet(page)).toHaveCount(0);
  await expect.poll(async () => (await stand(page)).cert.forgotten).toBe(2);
  // The document is as it was: one tab, not signed.
  await expect(page.locator(".tab-title")).toHaveCount(1);
});

test("nothing the box says promises what a signature is worth", async ({ page }) => {
  await openBook(page);
  await openDialog(page);
  await openCertificate(page);
  const words = await sheet(page).evaluate((el) => el.textContent ?? "");
  expect(words).not.toMatch(
    /\b(legal|legally|court|guarantee|guaranteed|valid|validity|binding|qualified|advanced|eidas|compliant|certified|proof|authentic)\b/i,
  );
});

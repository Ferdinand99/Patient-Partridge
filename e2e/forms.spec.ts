import { expect, test, type Page } from "@playwright/test";

// Filling in forms, in a real browser, with the same content security policy as the app and a
// stand-in for the Rust side (e2e/fake-tauri.js with e2e/fake-forms.js on top). The documents are
// made up: a page with every kind of field, a turned page, an XFA form, a form with scripts and a
// form whose texts are markup. The stand-in draws a dark bar for every field that has a value, so
// the canvas shows whether the page was drawn again after a change.

type Stand = {
  calls: { cmd: string; args: Record<string, unknown> }[];
  dialog: string[];
  violations: string[];
  title: string;
  forms: {
    calls: { cmd: string; args: Record<string, unknown> }[];
    saves: number;
    quits: number;
    failNext: unknown;
    delay: number;
    emit(event: string): void;
  };
  update: { found: unknown; installDelay: number };
};

test.beforeEach(async ({ page }) => {
  await page.addInitScript({ path: "e2e/fake-tauri.js" });
  await page.addInitScript({ path: "e2e/fake-forms.js" });
  await page.goto("/");
});

test.afterEach(async ({ page }) => {
  const violations = await page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.violations);
  expect(violations, "the content security policy blocked something").toEqual([]);
});

async function openForm(page: Page, path = "/docs/form.pdf") {
  await page.evaluate((p) => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = [p]), path);
  await page.locator("#empty-open").click();
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText(path.split("/").pop() as string);
  await expect(page.locator(".viewer-canvas").first()).toBeAttached();
}

const sets = (page: Page) =>
  page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.forms.calls.filter((c) => c.cmd === "set_form_field").map((c) => c.args));

const pageBox = async (page: Page) => {
  const box = await page.locator('.viewer-page[data-page="0"]').boundingBox();
  if (!box) throw new Error("the page is not on screen");
  return box;
};

/** The darkness (0 to 255) of the canvas of the page at a point given in points from its top left. */
const shade = (page: Page, x: number, y: number) =>
  page.locator('.viewer-page[data-page="0"] canvas').evaluate(
    (canvas: HTMLCanvasElement, at) => {
      const px = Math.floor((at.x / 612) * canvas.width);
      const py = Math.floor((at.y / 792) * canvas.height);
      return canvas.getContext("2d")?.getImageData(px, py, 1, 1).data[0] ?? -1;
    },
    { x, y },
  );

const fullname = (page: Page) => page.getByRole("textbox", { name: "Full name" });

test("puts a control over each field, where the field is, at every zoom", async ({ page }) => {
  await openForm(page);
  await expect(page.locator(".form-field")).toHaveCount(11);
  const check = async () => {
    const box = await pageBox(page);
    // The field `fullname` is [150, 68, 400, 92] on a 612 by 792 point page.
    const field = await fullname(page).boundingBox();
    if (!field) throw new Error("the field is not on screen");
    const k = box.width / 612;
    expect(field.x).toBeCloseTo(box.x + 150 * k, 0);
    expect(field.y).toBeCloseTo(box.y + 68 * k, 0);
    expect(field.width).toBeCloseTo(250 * k, 0);
    expect(field.height).toBeCloseTo(24 * k, 0);
    return { k, size: await fullname(page).evaluate((el) => Number.parseFloat(getComputedStyle(el).fontSize)) };
  };
  const first = await check();
  // The text of the field is 12 points high, whatever the zoom.
  expect(first.size).toBeCloseTo(12 * first.k, 0);
  await page.locator(".viewer").focus();
  await page.keyboard.press("Control+=");
  await page.keyboard.press("Control+=");
  await expect.poll(async () => (await pageBox(page)).width).toBeGreaterThan((await pageBox(page)).width - 1);
  const zoomed = await check();
  expect(zoomed.k).not.toBeCloseTo(first.k, 1);
  expect(zoomed.size).toBeCloseTo(12 * zoomed.k, 0);
  await page.keyboard.press("Control+1");
  const actual = await check();
  // Actual size is 96 dots to the inch: a point is 4/3 of a pixel.
  expect(actual.k).toBeCloseTo(96 / 72, 1);
});

test("fills in a text field, draws the page again and marks the document as changed", async ({ page }) => {
  await openForm(page);
  // At first nothing is drawn where the field is, and the document is as it was.
  expect(await shade(page, 270, 80)).toBe(255);
  expect(await page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.title)).toBe("form.pdf - Patient Partridge");
  await fullname(page).click();
  await page.keyboard.type("Ada Lovelace");
  // Nothing is sent while the reader is typing.
  expect(await sets(page)).toHaveLength(0);
  await page.keyboard.press("Tab");
  await expect.poll(() => sets(page)).toEqual([
    { doc: expect.any(Number), page: 0, index: 0, name: "fullname", value: { kind: "text", text: "Ada Lovelace" } },
  ]);
  // The new picture has a dark bar there.
  await expect.poll(() => shade(page, 270, 80)).toBeLessThan(100);
  // The window title and the tab show that there is something to save.
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.title)).toBe("• form.pdf - Patient Partridge");
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
  // The focus moved on to the next field in the document.
  await expect(page.getByRole("textbox", { name: "notes" })).toBeFocused();
  // The value is shown in the field when it is entered again.
  await expect(fullname(page)).toHaveValue("Ada Lovelace");
});

test("fills in every kind of field with the keyboard alone", async ({ page }) => {
  await openForm(page);
  await fullname(page).focus();
  await page.keyboard.type("Ada");
  await page.keyboard.press("Enter");
  await page.keyboard.press("Tab");
  // A box of several lines: Enter makes a new line, and Tab leaves it.
  await expect(page.getByRole("textbox", { name: "notes" })).toBeFocused();
  await page.keyboard.type("one");
  await page.keyboard.press("Enter");
  await page.keyboard.type("two");
  await page.keyboard.press("Tab");
  // The read-only field can be reached and read, not changed.
  const locked = page.getByRole("textbox", { name: "locked" });
  await expect(locked).toBeFocused();
  await expect(locked).toHaveAttribute("aria-readonly", "true");
  await page.keyboard.type("zzz");
  await expect(locked).toHaveValue("Fixed");
  await page.keyboard.press("Tab");
  // The check box: Space turns it on.
  const agree = page.getByRole("checkbox", { name: "I agree to the terms" });
  await expect(agree).toBeFocused();
  await page.keyboard.press("Space");
  await expect(agree).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("Tab");
  // The radio buttons are one stop; the arrow keys choose.
  const group = page.getByRole("radiogroup", { name: "Size" });
  await expect(group.getByRole("radio").first()).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await expect(group.getByRole("radio").nth(1)).toBeFocused();
  await expect(group.getByRole("radio").nth(1)).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("Tab");
  // The drop-down list: a letter picks a choice.
  const country = page.getByRole("combobox", { name: "Country" });
  await expect(country).toBeFocused();
  await country.selectOption({ label: "Sweden" });
  await page.keyboard.press("Tab");
  // The drop-down list with a box to type in.
  const city = page.getByRole("combobox", { name: "City" });
  await expect(city).toBeFocused();
  await page.keyboard.type("Tromsø");
  await page.keyboard.press("Tab");
  const colours = page.getByRole("listbox", { name: "Colours" });
  await expect(colours).toBeFocused();
  await colours.selectOption({ label: "Blue" });
  await page.keyboard.press("Tab");
  const toppings = page.getByRole("listbox", { name: "Toppings" });
  await expect(toppings).toBeFocused();
  await toppings.selectOption([{ label: "Ham" }, { label: "Olives" }]);
  await expect.poll(async () => (await sets(page)).map((a) => a.name)).toEqual([
    "fullname", "notes", "agree", "size", "country", "city", "colours", "toppings",
  ]);
  const values = (await sets(page)).map((a) => a.value);
  expect(values[0]).toEqual({ kind: "text", text: "Ada" });
  expect(values[1]).toEqual({ kind: "text", text: "one\ntwo" });
  expect(values[2]).toEqual({ kind: "checked", on: true });
  expect(values[3]).toEqual({ kind: "checked", on: true });
  expect(values[4]).toEqual({ kind: "choice", picked: [1] });
  expect(values[5]).toEqual({ kind: "text", text: "Tromsø" });
  expect(values[6]).toEqual({ kind: "choice", picked: [2] });
  expect(values[7]).toEqual({ kind: "choice", picked: [0, 2] });
  // Nothing was sent for the read-only field.
  expect((await sets(page)).some((a) => a.name === "locked")).toBe(false);
  // Every field with a value is drawn.
  await expect.poll(() => shade(page, 158, 256)).toBeLessThan(100); // check box
  await expect.poll(() => shade(page, 270, 341)).toBeLessThan(100); // country
});

test("names every field for a screen reader and shows what is required", async ({ page }) => {
  await openForm(page);
  // The names come from the tooltips of the fields, and from their names when there is no tooltip.
  await expect(page.getByRole("textbox", { name: "Full name" })).toHaveAttribute("aria-required", "true");
  await expect(page.getByRole("textbox", { name: "notes" })).toBeVisible();
  await expect(page.getByRole("checkbox", { name: "I agree to the terms" })).toBeVisible();
  await expect(page.getByRole("radiogroup", { name: "Size" }).getByRole("radio")).toHaveCount(3);
  await expect(page.getByRole("radio", { name: "Size, choice 2 of 3" })).toBeVisible();
  // A required field that is empty is outlined with a dashed line, and the outline goes when it is filled.
  const outline = () => fullname(page).evaluate((el) => getComputedStyle(el).outlineStyle);
  expect(await outline()).toBe("dashed");
  await fullname(page).fill("x");
  await page.keyboard.press("Tab");
  await expect.poll(outline).toBe("none");
  // The fields that are not required have no outline.
  expect(await page.getByRole("textbox", { name: "locked" }).evaluate((el) => getComputedStyle(el).outlineStyle)).toBe("none");
  // The fields are in the order of the page for the Tab key.
  const order = await page.locator(".form-field").evaluateAll((els) => els.map((e) => e.getAttribute("data-form-index")));
  expect(order).toEqual(["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]);
});

test("takes a change back and makes it again, with the keyboard", async ({ page }) => {
  await openForm(page);
  await page.getByRole("checkbox", { name: "I agree to the terms" }).click();
  await expect.poll(() => shade(page, 158, 256)).toBeLessThan(100);
  await page.keyboard.press("Control+z");
  await expect(page.getByRole("checkbox", { name: "I agree to the terms" })).toHaveAttribute("aria-checked", "false");
  await expect.poll(() => shade(page, 158, 256)).toBe(255);
  // Back to what the file has: nothing to save.
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.title)).toBe("form.pdf - Patient Partridge");
  await page.keyboard.press("Control+y");
  await expect(page.getByRole("checkbox", { name: "I agree to the terms" })).toHaveAttribute("aria-checked", "true");
  await expect.poll(() => shade(page, 158, 256)).toBeLessThan(100);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.title)).toBe("• form.pdf - Patient Partridge");

  // A text: Ctrl+Z in the field is the box's own, outside it the change of the field is taken back.
  await fullname(page).fill("Ada");
  await page.locator(".viewer-page").first().click({ position: { x: 5, y: 5 } });
  await expect.poll(() => shade(page, 270, 80)).toBeLessThan(100);
  await page.keyboard.press("Control+z");
  await expect(fullname(page)).toHaveValue("");
  await expect.poll(() => shade(page, 270, 80)).toBe(255);
});

test("keeps a choice that cannot be taken back, and says why", async ({ page }) => {
  await openForm(page);
  await page.getByRole("radio", { name: "Size, choice 1 of 3" }).click();
  await expect(page.getByRole("radio", { name: "Size, choice 1 of 3" })).toHaveAttribute("aria-checked", "true");
  await page.keyboard.press("Control+z");
  // PDFium cannot turn a radio button off, so the window says so and the choice stays.
  await expect(page.locator(".form-bar-text")).toContainText("cannot be turned off");
  await expect(page.locator(".notice")).toBeVisible();
  await page.locator(".notice button", { hasText: "OK" }).click();
  await expect(page.getByRole("radio", { name: "Size, choice 1 of 3" })).toHaveAttribute("aria-checked", "true");
});

test("does not change a field that is read only, and says when a change is refused", async ({ page }) => {
  await openForm(page);
  await page.getByRole("textbox", { name: "locked" }).click();
  await page.keyboard.type("nope");
  await page.keyboard.press("Tab");
  expect(await sets(page)).toHaveLength(0);
  // A drop-down list cannot be cleared once a choice is made.
  const country = page.getByRole("combobox", { name: "Country" });
  await country.selectOption({ label: "Norway" });
  await expect.poll(async () => (await sets(page)).length).toBe(1);
  await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.forms.failNext = { code: "form_field_refused", message: "no" }));
  await country.selectOption({ label: "Sweden" });
  await expect(page.locator(".form-bar-text")).toContainText("cannot be cleared");
  // The field shows what the engine has again.
  await expect(country).toHaveValue("0");
  // The message goes away by itself, or at a press of the cross.
  await page.locator(".form-bar-close").click();
  await expect(page.locator(".form-bar:not([hidden])")).toHaveCount(0);
});

test("says an XFA form is not filled in, and shows no field", async ({ page }) => {
  await openForm(page, "/docs/xfa.pdf");
  await expect(page.locator(".form-bar-text")).toContainText("XFA form");
  await expect(page.locator(".form-bar-text")).toContainText("cannot fill in");
  await expect(page.locator(".form-field")).toHaveCount(0);
  // The pages are shown, and nothing is marked as changed.
  await expect(page.locator(".viewer-canvas").first()).toBeAttached();
  expect(await page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.title)).toBe("xfa.pdf - Patient Partridge");
});

test("tells once, calmly, that scripts do not run, and still fills the form in", async ({ page }) => {
  await openForm(page, "/docs/scripts.pdf");
  await expect(page.locator(".form-bar-text")).toContainText("does not run");
  await expect(page.locator(".form-bar")).toHaveCount(1);
  // It is a status line, not a question: nothing is covered and nothing waits for an answer.
  await expect(page.locator(".notice")).toHaveCount(0);
  await expect(page.locator(".form-bar")).toHaveAttribute("role", "status");
  await fullname(page).fill("works");
  await page.keyboard.press("Tab");
  await expect.poll(async () => (await sets(page)).length).toBe(1);
  await expect(page.locator(".form-bar")).toHaveCount(1);
});

test("fills in a form on a turned page", async ({ page }) => {
  await openForm(page, "/docs/rotated.pdf");
  const input = fullname(page);
  await expect(input).toHaveCSS("transform", /matrix\(.*\)/);
  // A control that is turned a quarter turn keeps the shape of the field and covers the drawn field.
  const drawn = await input.boundingBox();
  const box = await pageBox(page);
  const k = box.width / 612;
  if (!drawn) throw new Error("the field is not on screen");
  // The field is drawn 24 wide and 250 high (turned), at (68, 150) from the top left of the page.
  expect(drawn.width).toBeCloseTo(24 * k, 0);
  expect(drawn.height).toBeCloseTo(250 * k, 0);
  expect(drawn.x).toBeCloseTo(box.x + 68 * k, 0);
  expect(drawn.y).toBeCloseTo(box.y + 150 * k, 0);
  await input.click();
  await page.keyboard.type("turned");
  await page.keyboard.press("Tab");
  await expect.poll(async () => (await sets(page)).length).toBe(1);
});

test("shows names and values from the PDF as text, never as markup", async ({ page }) => {
  await openForm(page, "/docs/hostile.pdf");
  const markup = '<img src=x onerror="window.pwned=1"><script>window.pwned=2</script>';
  await expect(page.locator(".form-field")).toHaveCount(2);
  await expect(page.locator(".form-layer img, .form-layer script")).toHaveCount(0);
  await expect(page.locator('.form-field[data-form-index="0"]')).toHaveValue(markup);
  await expect(page.locator('.form-field[data-form-index="0"]')).toHaveAttribute("aria-label", markup);
  expect(await page.evaluate(() => (window as unknown as { pwned?: number }).pwned)).toBeUndefined();
});

test("saves a filled form with the keyboard and the document is clean again", async ({ page }) => {
  await openForm(page);
  await fullname(page).fill("Ada");
  await page.keyboard.press("Tab");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.title)).toBe("• form.pdf - Patient Partridge");
  await page.keyboard.press("Control+s");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.forms.saves)).toBe(1);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.title)).toBe("form.pdf - Patient Partridge");
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
});

test("takes screenshots of a form in light and dark (nothing is checked about the look)", async ({ page }) => {
  await openForm(page);
  await fullname(page).fill("Ada Lovelace");
  await page.getByRole("checkbox", { name: "I agree to the terms" }).click();
  await page.getByRole("textbox", { name: "notes" }).focus();
  for (const scheme of ["light", "dark"] as const) {
    await page.emulateMedia({ colorScheme: scheme });
    await page.screenshot({ path: `e2e/artifacts/forms-${scheme}.png` });
  }
});

// ---- text that is typed and not left -------------------------------------------------------------

const stand = (page: Page) => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.forms);
const sequence = (page: Page) =>
  page.evaluate(() =>
    (window as unknown as { __E2E__: Stand }).__E2E__.forms.calls
      .map((c) => c.cmd)
      .filter((c) => c === "set_form_field" || c === "save_pdf" || c === "save_pdf_as"),
  );

/** Types in the field `Full name` and does not leave it: the text is in the box, not in the document. */
async function typeWithoutLeaving(page: Page) {
  await fullname(page).click();
  await page.keyboard.type("Ada");
  expect(await sets(page)).toHaveLength(0);
  // The tab already counts as changed.
  await expect(page.locator(".tab-dirty")).toHaveCount(1);
}

test("Ctrl+S saves what is typed in the field that still has the focus", async ({ page }) => {
  await openForm(page);
  await typeWithoutLeaving(page);
  await page.keyboard.press("Control+s");
  await expect.poll(async () => (await stand(page)).saves).toBe(1);
  // The value went to the document first, and then the document was saved.
  expect(await sequence(page)).toEqual(["set_form_field", "save_pdf"]);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.title)).toBe("form.pdf - Patient Partridge");
  await expect(fullname(page)).toHaveValue("Ada");
});

test("Ctrl+Shift+S saves a copy with what is typed in the field that still has the focus", async ({ page }) => {
  await openForm(page);
  await typeWithoutLeaving(page);
  await page.keyboard.press("Control+Shift+S");
  await expect.poll(async () => (await sequence(page)).length).toBe(2);
  expect(await sequence(page)).toEqual(["set_form_field", "save_pdf_as"]);
});

test("closing a tab with text typed in a field asks whether to save, and does not close it silently", async ({ page }) => {
  await openForm(page);
  await typeWithoutLeaving(page);
  await page.keyboard.press("Control+w");
  await expect(page.getByRole("alertdialog")).toContainText('Save changes to "form.pdf"?');
  // The value was sent before the question.
  expect(await sets(page)).toHaveLength(1);
  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.locator(".tab-title")).toHaveText(["form.pdf"]);
  await page.keyboard.press("Control+w");
  await page.getByRole("alertdialog").getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(async () => (await stand(page)).saves).toBe(1);
  await expect(page.locator(".tab-title")).toHaveCount(0);
});

test("quitting with text typed in a field asks whether to save", async ({ page }) => {
  await openForm(page);
  await typeWithoutLeaving(page);
  await page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.forms.emit("quit-requested"));
  await expect(page.getByRole("alertdialog")).toContainText('Save changes to "form.pdf"?');
  expect(await sets(page)).toHaveLength(1);
  expect((await stand(page)).quits).toBe(0);
  await page.getByRole("button", { name: "Save and quit" }).click();
  await expect.poll(async () => (await stand(page)).quits).toBe(1);
  expect((await stand(page)).saves).toBe(1);
});

test("an update waits for text that is typed in a field, and saves it first", async ({ page }) => {
  await openForm(page);
  await typeWithoutLeaving(page);
  await page.evaluate(() => {
    const e2e = (window as unknown as { __E2E__: Stand & { update: Record<string, unknown> } }).__E2E__;
    e2e.update.found = {
      version: "0.2.0",
      currentVersion: "0.1.0",
      notes: "Notes",
      method: "automatic",
    };
  });
  await page.locator("#menu-button").click();
  await page.getByRole("menuitem", { name: "Check for Updates…" }).click();
  await page.getByRole("alertdialog").getByRole("button", { name: "Download and Install" }).click();
  // The update is downloaded; installing would close the document and lose the typed text.
  await expect(page.getByRole("alertdialog")).toContainText("Save your changes first");
  await expect(page.getByRole("alertdialog")).toContainText("1 document has changes that are not saved");
  expect((await stand(page)).saves).toBe(0);
  await page.getByRole("button", { name: "Save and install" }).click();
  await expect.poll(async () => (await stand(page)).saves).toBe(1);
  expect(await sequence(page)).toEqual(["set_form_field", "save_pdf"]);
  await expect.poll(() =>
    page.evaluate(() =>
      (window as unknown as { __E2E__: Stand }).__E2E__.calls.some((c) => c.cmd === "install_update"),
    ),
  ).toBe(true);
});

test("a tab that goes behind another one sends what is typed in it", async ({ page }) => {
  await openForm(page);
  await typeWithoutLeaving(page);
  await page.evaluate(() => ((window as unknown as { __E2E__: Stand }).__E2E__.dialog = ["/docs/second.pdf"]));
  await page.keyboard.press("Control+o");
  await expect(page.locator('.tab-title[aria-selected="true"]')).toHaveText("second.pdf");
  await expect.poll(async () => (await sets(page)).length).toBe(1);
});

// ---- text the document did not accept --------------------------------------------------------------

test("text the document refused shows at rest, is flagged, and the Save notice names the field and takes the reader to it", async ({ page }) => {
  await openForm(page);
  await page.evaluate(
    () => ((window as unknown as { __E2E__: Stand }).__E2E__.forms.failNext = { code: "invalid_request", message: "the name or the value is too long" }),
  );
  await fullname(page).click();
  await page.keyboard.type("Ada Lovelace");
  await page.keyboard.press("Tab");
  const box = fullname(page);
  await expect(box).toHaveClass(/form-refused/);
  await expect(box).toHaveAttribute("aria-invalid", "true");
  await expect(box).toHaveValue("Ada Lovelace");
  // The reader has left the box, and the text is still there to read: dark on white, with an outline.
  const style = await box.evaluate((el) => {
    const s = getComputedStyle(el);
    return { color: s.color, fill: s.webkitTextFillColor, background: s.backgroundColor, outline: s.outlineStyle, width: s.outlineWidth };
  });
  expect(style.fill).toBe("rgb(17, 17, 17)");
  expect(style.color).toBe("rgb(17, 17, 17)");
  expect(style.background).toBe("rgb(255, 255, 255)");
  expect(style.outline).toBe("solid");
  expect(style.width).toBe("2px");
  // The other fields are as they were.
  await expect(page.getByRole("textbox", { name: "notes" })).not.toHaveClass(/form-refused/);
  // Wait until the short message in the bar is gone: the box still says it.
  await page.locator(".form-bar-close").click();
  await page.keyboard.press("Control+s");
  const notice = page.getByRole("alertdialog");
  await expect(notice).toContainText('The field "Full name" on page 1');
  await expect(notice).toContainText("Escape");
  expect((await stand(page)).saves).toBe(0);
  await notice.getByRole("button", { name: "Go to the field" }).click();
  await expect(box).toBeFocused();
  // Escape puts the old text back, and the document can be saved again.
  await page.keyboard.press("Escape");
  await expect(box).not.toHaveClass(/form-refused/);
  await expect(box).toHaveValue("");
  await expect(page.locator(".tab-dirty")).toHaveCount(0);
});

test("a double click on a Save button does the work once", async ({ page }) => {
  await openForm(page);
  const asks = () =>
    page.evaluate(() => (window as unknown as { __E2E__: Stand }).__E2E__.forms.calls.filter((c) => c.cmd === "save_pdf_as").length);
  await page.evaluate(() => {
    const b = document.getElementById("save-copy-button") as HTMLButtonElement;
    b.click();
    b.click();
  });
  await expect.poll(asks).toBe(1);
  await page.waitForTimeout(200);
  expect(await asks()).toBe(1);
  // The next press is a new question.
  await page.locator("#save-copy-button").click();
  await expect.poll(asks).toBe(2);
});

test("the toolbar keeps every button in the window, down to the smallest width", async ({ page }) => {
  await openForm(page);
  for (const width of [480, 520, 560, 600, 640, 1000]) {
    await page.setViewportSize({ width, height: 700 });
    const rights = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>("#toolbar button")].map((b) => ({ id: b.id, right: b.getBoundingClientRect().right })),
    );
    for (const b of rights) expect(b.right, `${b.id} at ${width}`).toBeLessThanOrEqual(width);
  }
});

test("a key that prints punctuation or an accented letter does not close the tab, a Cyrillic one does", async ({ page }) => {
  await openForm(page);
  const press = (key: string, code: string) =>
    page.evaluate(
      ([k, c]) => document.body.dispatchEvent(new KeyboardEvent("keydown", { key: k, code: c, ctrlKey: true, bubbles: true, cancelable: true })),
      [key, code],
    );
  await press(",", "KeyW");
  await press("\u00e9", "KeyW");
  await expect(page.locator(".tab-title")).toHaveText(["form.pdf"]);
  await press("\u0446", "KeyW");
  await expect(page.locator(".tab-title")).toHaveCount(0);
});

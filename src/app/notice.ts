// A small message box for errors and questions, drawn over the app: the password question, a
// failed file, and the question before a link is opened. If a second one is shown while the first
// is open (a drop or an engine error), the newest is on top and the older ones wait behind it.

export interface NoticeButton {
  id: string;
  label: string;
  primary?: boolean;
  /** The button that has the focus when the box opens, for a question where a quick Enter should
   * give the safe answer. Without it the primary button has the focus. */
  safe?: boolean;
}

export interface NoticeOptions {
  title: string;
  detail: string;
  buttons: NoticeButton[];
  /** Shows a password field. The typed value is returned with the answer. */
  password?: boolean;
  /** A line shown in a box of its own and in a fixed-width font, such as a web address. */
  quote?: string;
}

export interface NoticeResult {
  /** The id of the button pressed, or "cancel" when the box was dismissed with Escape. */
  button: string;
  password?: string;
}

let counter = 0;

export function showNotice(layer: HTMLElement, options: NoticeOptions): Promise<NoticeResult> {
  return new Promise((resolve) => {
    const previousFocus = document.activeElement as HTMLElement | null;
    const id = `notice-${++counter}`;

    const box = document.createElement("form");
    box.className = "sheet notice";
    box.setAttribute("role", "alertdialog");
    box.setAttribute("aria-modal", "true");
    box.setAttribute("aria-labelledby", `${id}-title`);
    box.setAttribute("aria-describedby", `${id}-detail`);

    const title = document.createElement("h2");
    title.className = "sheet-title";
    title.id = `${id}-title`;
    title.textContent = options.title;
    const detail = document.createElement("p");
    detail.className = "sheet-text";
    detail.id = `${id}-detail`;
    detail.textContent = options.detail;
    box.append(title, detail);

    if (options.quote !== undefined) {
      const quote = document.createElement("p");
      quote.className = "sheet-url";
      quote.textContent = options.quote;
      box.append(quote);
    }

    let input: HTMLInputElement | null = null;
    if (options.password) {
      input = document.createElement("input");
      input.type = "password";
      input.className = "field-input sheet-field";
      input.autocomplete = "off";
      input.setAttribute("aria-label", "Password");
      box.append(input);
    }

    const row = document.createElement("div");
    row.className = "sheet-foot";
    const primary = options.buttons.find((b) => b.primary) ?? options.buttons[0];
    const buttons = new Map<string, HTMLButtonElement>();
    for (const b of options.buttons) {
      const el = document.createElement("button");
      el.type = b === primary && input ? "submit" : "button";
      el.className = b.primary ? "btn btn-primary" : "btn btn-secondary";
      el.textContent = b.label;
      // The submit button is handled by the form's submit event, so Enter and a click act alike.
      if (el.type !== "submit") el.addEventListener("click", () => finish(b.id));
      buttons.set(b.id, el);
      row.append(el);
    }
    box.append(row);

    const finish = (button: string) => {
      layer.removeEventListener("keydown", onKey);
      box.remove();
      layer.hidden = layer.childElementCount === 0;
      previousFocus?.focus?.();
      resolve({ button, password: input ? input.value : undefined });
    };
    const onKey = (e: KeyboardEvent) => {
      // Only the box on top answers; the ones behind it wait.
      if (e.key === "Escape" && layer.lastElementChild === box) {
        e.preventDefault();
        e.stopPropagation();
        finish("cancel");
      }
    };
    // Enter in the password field presses the primary button.
    box.addEventListener("submit", (e) => {
      e.preventDefault();
      finish(primary.id);
    });
    layer.addEventListener("keydown", onKey);

    layer.append(box);
    layer.hidden = false;
    // A question about opening something starts on the safe answer, not on the dangerous one.
    const focusButton = options.buttons.find((b) => b.safe) ?? primary;
    (input ?? buttons.get(focusButton.id))?.focus();
  });
}

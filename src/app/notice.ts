// A small message box for errors and the password question. One at a time, drawn over the app.

export interface NoticeButton {
  id: string;
  label: string;
  primary?: boolean;
}

export interface NoticeOptions {
  title: string;
  detail: string;
  buttons: NoticeButton[];
  /** Shows a password field. The typed value is returned with the answer. */
  password?: boolean;
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
    box.className = "notice";
    box.setAttribute("role", "alertdialog");
    box.setAttribute("aria-modal", "true");
    box.setAttribute("aria-labelledby", `${id}-title`);
    box.setAttribute("aria-describedby", `${id}-detail`);

    const title = document.createElement("h2");
    title.id = `${id}-title`;
    title.textContent = options.title;
    const detail = document.createElement("p");
    detail.id = `${id}-detail`;
    detail.textContent = options.detail;
    box.append(title, detail);

    let input: HTMLInputElement | null = null;
    if (options.password) {
      input = document.createElement("input");
      input.type = "password";
      input.className = "notice-input";
      input.autocomplete = "off";
      input.setAttribute("aria-label", "Password");
      box.append(input);
    }

    const row = document.createElement("div");
    row.className = "notice-buttons";
    const primary = options.buttons.find((b) => b.primary) ?? options.buttons[0];
    const buttons = new Map<string, HTMLButtonElement>();
    for (const b of options.buttons) {
      const el = document.createElement("button");
      el.type = b === primary && input ? "submit" : "button";
      el.className = b.primary ? "btn btn-primary" : "btn";
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
      if (e.key === "Escape") {
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
    (input ?? buttons.get(primary.id))?.focus();
  });
}

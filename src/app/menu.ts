// The settings menu: a small panel under the "more" button. It closes when something outside it is
// pressed, when Escape is pressed, and after a choice that does something.

export class MenuController {
  constructor(
    private readonly button: HTMLElement,
    private readonly menu: HTMLElement,
  ) {
    button.addEventListener("click", (e) => {
      e.stopPropagation();
      if (this.isOpen) this.close();
      else this.open();
    });
    menu.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        this.close();
        this.button.focus();
      }
    });
    document.addEventListener("pointerdown", (e) => {
      if (this.isOpen && e.target instanceof Node && !menu.contains(e.target) && !button.contains(e.target)) {
        this.close();
      }
    });
  }

  get isOpen(): boolean {
    return !this.menu.hidden;
  }

  open(): void {
    const rect = this.button.getBoundingClientRect();
    this.menu.style.top = `${Math.round(rect.bottom + 6)}px`;
    this.menu.hidden = false;
    this.button.setAttribute("aria-expanded", "true");
    this.menu
      .querySelector<HTMLElement>('input:checked, button, [tabindex="0"]')
      ?.focus({ preventScroll: true });
  }

  close(): void {
    if (!this.isOpen) return;
    this.menu.hidden = true;
    this.button.setAttribute("aria-expanded", "false");
  }
}

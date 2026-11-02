// The box that shows while a page tool works and takes a moment: what it is doing, how far it has
// come when that is known, and a button that stops it. It sits in the layer of the message boxes.

export interface BusyBoxOptions {
  title: string;
  detail: string;
  cancelLabel: string;
  onCancel(): void;
}

export interface BusyBox {
  /** Tells how much of the work is done. Without a total the bar just moves. */
  progress(done: number, total: number): void;
  /** Takes the box away while a question is shown, and brings it back. */
  hide(): void;
  show(): void;
  close(): void;
}

export function showBusyBox(layer: HTMLElement, options: BusyBoxOptions): BusyBox {
  const previousFocus = document.activeElement as HTMLElement | null;
  const box = document.createElement("form");
  box.className = "sheet notice busy-box";
  box.setAttribute("role", "alertdialog");
  box.setAttribute("aria-modal", "true");
  box.setAttribute("aria-busy", "true");
  const title = document.createElement("h2");
  title.className = "sheet-title";
  title.textContent = options.title;
  const detail = document.createElement("p");
  detail.className = "sheet-text";
  detail.textContent = options.detail;
  const bar = document.createElement("progress");
  bar.className = "sheet-progress";
  bar.setAttribute("aria-label", options.title);
  const foot = document.createElement("div");
  foot.className = "sheet-foot";
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "btn btn-secondary";
  cancel.textContent = options.cancelLabel;
  cancel.addEventListener("click", () => options.onCancel());
  foot.append(cancel);
  box.append(title, detail, bar, foot);
  box.addEventListener("submit", (e) => e.preventDefault());
  box.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      options.onCancel();
    }
  });

  let shown = false;
  const put = () => {
    if (shown) return;
    shown = true;
    layer.append(box);
    layer.hidden = false;
    cancel.focus();
  };
  put();

  return {
    progress(done, total) {
      if (total > 0) {
        bar.max = total;
        bar.value = Math.min(done, total);
        detail.textContent = `${options.detail} ${Math.round((Math.min(done, total) / total) * 100)} %`;
      }
    },
    hide() {
      if (!shown) return;
      shown = false;
      box.remove();
      layer.hidden = layer.childElementCount === 0;
    },
    show: put,
    close() {
      shown = false;
      box.remove();
      layer.hidden = layer.childElementCount === 0;
      if (previousFocus?.isConnected) previousFocus.focus?.();
    },
  };
}

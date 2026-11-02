// The box that shows while a page tool works and takes a moment: what it is doing, how far it has
// come when that is known, and, when the work can be stopped without a cost, a button that stops it.
// It sits in the layer of the message boxes.
//
// A change to the pages cannot be stopped half way: the only way would be to end the engine of the
// document, and every change that is not saved goes with it. Such a box has no button, and Escape
// does nothing in it. Only the work of bringing pages in from another file, or copying pages out,
// can be stopped, and then nothing is lost.

export interface BusyBoxOptions {
  title: string;
  detail: string;
  /** The label of the button that stops the work. Without it the work cannot be stopped. */
  cancelLabel?: string;
  /** Called when the button is pressed or Escape is. Only for work that can be stopped. */
  onCancel?(): void;
}

export interface BusyBox {
  /** Tells how much of the work is done. Without a total the bar just moves. */
  progress(done: number, total: number): void;
  /** Takes the button away from use and says why: the step that is under way cannot be stopped. */
  cannotStop(note: string): void;
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
  box.tabIndex = -1;
  const title = document.createElement("h2");
  title.className = "sheet-title";
  title.textContent = options.title;
  const detail = document.createElement("p");
  detail.className = "sheet-text";
  detail.textContent = options.detail;
  const bar = document.createElement("progress");
  bar.className = "sheet-progress";
  bar.setAttribute("aria-label", options.title);
  box.append(title, detail, bar);

  let cancel: HTMLButtonElement | null = null;
  let stoppable = options.cancelLabel !== undefined && options.onCancel !== undefined;
  let note = "";
  if (stoppable) {
    const foot = document.createElement("div");
    foot.className = "sheet-foot";
    cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "btn btn-secondary";
    cancel.textContent = options.cancelLabel ?? "";
    cancel.addEventListener("click", () => {
      if (stoppable) options.onCancel?.();
    });
    foot.append(cancel);
    box.append(foot);
  }
  box.addEventListener("submit", (e) => e.preventDefault());
  box.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      // Escape never ends anything that cannot be stopped without a cost.
      e.preventDefault();
      e.stopPropagation();
      if (stoppable) options.onCancel?.();
    }
  });

  let shown = false;
  const put = () => {
    if (shown) return;
    shown = true;
    layer.append(box);
    layer.hidden = false;
    (cancel ?? box).focus();
  };
  put();

  let percent = "";
  const text = () => {
    detail.textContent = [options.detail, percent, note].filter((s) => s !== "").join(" ");
  };

  return {
    progress(done, total) {
      if (total > 0) {
        bar.max = total;
        bar.value = Math.min(done, total);
        percent = `${Math.round((Math.min(done, total) / total) * 100)} %`;
        text();
      }
    },
    cannotStop(why) {
      stoppable = false;
      if (cancel) cancel.disabled = true;
      note = why;
      text();
      box.focus();
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

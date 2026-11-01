// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { showNotice } from "./notice";

let layer: HTMLElement;

beforeEach(() => {
  vi.useFakeTimers();
  layer = document.createElement("div");
  layer.hidden = true;
  document.body.append(layer);
});

afterEach(() => {
  vi.useRealTimers();
  document.body.replaceChildren();
});

const buttons = () => [...layer.querySelectorAll<HTMLButtonElement>("button")];
const byLabel = (label: string) => buttons().find((b) => b.textContent === label) as HTMLButtonElement;

function question(armMs?: number) {
  const result = vi.fn();
  void showNotice(layer, {
    title: "Open it?",
    detail: "A question.",
    armMs,
    buttons: [
      { id: "cancel", label: "Cancel", safe: true },
      { id: "open", label: "Open", primary: true },
    ],
  }).then(result);
  return result;
}

describe("a question that cannot be answered by a press that was already on its way", () => {
  it("keeps the primary button disabled until the delay has passed", async () => {
    const result = question(500);
    expect(byLabel("Open").disabled).toBe(true);
    expect(byLabel("Cancel").disabled).toBe(false);
    byLabel("Open").click();
    expect(result).not.toHaveBeenCalled();
    vi.advanceTimersByTime(499);
    expect(byLabel("Open").disabled).toBe(true);
    vi.advanceTimersByTime(1);
    expect(byLabel("Open").disabled).toBe(false);
    byLabel("Open").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(result).toHaveBeenCalledWith(expect.objectContaining({ button: "open" }));
  });

  it("always lets the safe answer through", async () => {
    const result = question(500);
    byLabel("Cancel").click();
    await vi.advanceTimersByTimeAsync(0);
    expect(result).toHaveBeenCalledWith(expect.objectContaining({ button: "cancel" }));
  });

  it("ignores a key that is held down (Enter and Space repeat) during the delay", () => {
    question(500);
    for (const key of ["Enter", " "]) {
      const e = new KeyboardEvent("keydown", { key, repeat: true, bubbles: true, cancelable: true });
      byLabel("Cancel").dispatchEvent(e);
      expect(e.defaultPrevented, key).toBe(true);
    }
    vi.advanceTimersByTime(500);
    const e = new KeyboardEvent("keydown", { key: "Enter", repeat: true, bubbles: true, cancelable: true });
    byLabel("Cancel").dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
  });

  it("does not delay anything unless asked to", () => {
    question();
    expect(byLabel("Open").disabled).toBe(false);
  });
});

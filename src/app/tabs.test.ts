// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { TabStrip } from "./tabs";

let root: HTMLElement;
const select = vi.fn();
const close = vi.fn();
let strip: TabStrip;

const info = (id: number, title: string) => ({ id, title, path: `/files/${title}` });

beforeEach(() => {
  document.body.innerHTML = '<div id="tabs" role="tablist"></div>';
  root = document.getElementById("tabs") as HTMLElement;
  select.mockReset();
  close.mockReset();
  strip = new TabStrip(root, { select, close });
});

describe("TabStrip", () => {
  it("shows a dot, apart from the title, on a tab with changes that are not saved", () => {
    strip.render([info(1, "a.pdf"), { ...info(2, "b.pdf"), dirty: true }], 1);
    const dots = [...root.querySelectorAll<HTMLElement>(".tab-dirty")];
    expect(dots).toHaveLength(1);
    expect(dots[0].closest(".tab")?.getAttribute("data-id")).toBe("2");
    expect(dots[0].getAttribute("aria-label")).toBe("Unsaved changes");
    // The name of the tab and its close button are not changed by it.
    expect([...root.querySelectorAll('[role="tab"]')].map((t) => t.textContent)).toEqual(["a.pdf", "b.pdf"]);
    expect(root.querySelector('[aria-label="Close b.pdf"]')).not.toBeNull();
  });

  it("draws one tab for each document and marks the selected one", () => {
    strip.render([info(1, "a.pdf"), info(2, "b.pdf")], 2);
    const tabs = [...root.querySelectorAll<HTMLElement>('[role="tab"]')];
    expect(tabs.map((t) => t.textContent)).toEqual(["a.pdf", "b.pdf"]);
    expect(tabs.map((t) => t.getAttribute("aria-selected"))).toEqual(["false", "true"]);
    // Only the selected tab is in the tab order; the arrow keys move between tabs.
    expect(tabs.map((t) => t.tabIndex)).toEqual([-1, 0]);
    expect(tabs[0].title).toBe("/files/a.pdf");
  });

  it("selects a tab when it is clicked and closes it with its button", () => {
    strip.render([info(1, "a.pdf"), info(2, "b.pdf")], 1);
    root.querySelectorAll<HTMLElement>('[role="tab"]')[1].click();
    expect(select).toHaveBeenCalledWith(2);
    root.querySelector<HTMLElement>('[aria-label="Close a.pdf"]')?.click();
    expect(close).toHaveBeenCalledWith(1);
    // Closing does not also select the tab.
    expect(select).toHaveBeenCalledTimes(1);
  });

  it("names a long title only by text, never as markup", () => {
    strip.render([info(1, "<img src=x onerror=alert(1)>.pdf")], 1);
    expect(root.querySelector("img")).toBeNull();
    expect(root.querySelector('[role="tab"]')?.textContent).toBe("<img src=x onerror=alert(1)>.pdf");
  });

  it("moves with the arrow keys, wrapping around, and with Home and End", () => {
    strip.render([info(1, "a"), info(2, "b"), info(3, "c")], 3);
    const press = (key: string) => {
      const current = root.querySelector<HTMLElement>('[role="tab"][aria-selected="true"]') as HTMLElement;
      current.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
    };
    press("ArrowRight");
    expect(select).toHaveBeenLastCalledWith(1);
    press("ArrowLeft");
    expect(select).toHaveBeenLastCalledWith(2);
    press("Home");
    expect(select).toHaveBeenLastCalledWith(1);
    press("End");
    expect(select).toHaveBeenLastCalledWith(3);
  });

  it("ignores arrow keys with Ctrl and keys that are not for it", () => {
    strip.render([info(1, "a"), info(2, "b")], 1);
    const tab = root.querySelector<HTMLElement>('[role="tab"]') as HTMLElement;
    tab.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", ctrlKey: true, bubbles: true }));
    tab.dispatchEvent(new KeyboardEvent("keydown", { key: "x", bubbles: true }));
    expect(select).not.toHaveBeenCalled();
  });

  it("closes a tab with the middle button and ignores other buttons", () => {
    strip.render([info(1, "a"), info(2, "b")], 1);
    const second = root.querySelectorAll<HTMLElement>('[role="tab"]')[1];
    second.dispatchEvent(new MouseEvent("auxclick", { button: 2, bubbles: true }));
    expect(close).not.toHaveBeenCalled();
    second.dispatchEvent(new MouseEvent("auxclick", { button: 1, bubbles: true }));
    expect(close).toHaveBeenCalledWith(2);
  });

  it("keeps the focus on the selected tab when it redraws", () => {
    strip.render([info(1, "a"), info(2, "b")], 1);
    (root.querySelector('[role="tab"]') as HTMLElement).focus();
    strip.render([info(1, "a"), info(2, "b")], 2);
    expect(document.activeElement?.textContent).toBe("b");
    // A redraw does not steal the focus from somewhere else.
    document.body.insertAdjacentHTML("beforeend", '<input id="other">');
    (document.getElementById("other") as HTMLElement).focus();
    strip.render([info(1, "a")], 1);
    expect(document.activeElement?.id).toBe("other");
  });

  it("draws nothing when no document is open", () => {
    strip.render([info(1, "a")], 1);
    strip.render([], null);
    expect(root.children.length).toBe(0);
  });
});

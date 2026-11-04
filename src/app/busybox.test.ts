// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import { showBusyBox } from "./busybox";

describe("the box of a tool that works", () => {
  const layer = () => {
    const el = document.createElement("div");
    document.body.append(el);
    return el;
  };

  it("has a button that is taken away for a step that cannot be stopped and given back for the next", () => {
    const onCancel = vi.fn();
    const box = showBusyBox(layer(), { title: "Adding pages", detail: "Wait.", cancelLabel: "Stop", onCancel });
    const stop = () => document.querySelector<HTMLButtonElement>(".busy-box .btn")!;
    stop().click();
    expect(onCancel).toHaveBeenCalledTimes(1);

    box.cannotStop("This step cannot be stopped any more.");
    expect(stop().disabled).toBe(true);
    expect(document.querySelector(".busy-box")?.textContent).toContain("cannot be stopped any more");
    stop().click();
    expect(onCancel).toHaveBeenCalledTimes(1);

    // The next file of the same add can be stopped again, and the old note is gone.
    box.canStop();
    expect(stop().disabled).toBe(false);
    expect(document.querySelector(".busy-box")?.textContent).not.toContain("cannot be stopped any more");
    stop().click();
    expect(onCancel).toHaveBeenCalledTimes(2);
    box.close();
  });

  it("stays without a button when the work cannot be stopped at all", () => {
    const box = showBusyBox(layer(), { title: "Turning pages", detail: "Wait." });
    box.canStop();
    expect(document.querySelector(".busy-box .btn")).toBeNull();
    box.close();
  });
});

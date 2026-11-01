// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { beforeEach, describe, expect, it } from "vitest";
import { THEME_KEY, applyTheme, loadTheme, parseTheme, saveTheme, themeLabel } from "./theme";

beforeEach(() => {
  document.documentElement.removeAttribute("data-theme");
  localStorage.clear();
});

describe("theme choice", () => {
  it("reads only the three known values", () => {
    expect(parseTheme("dark")).toBe("dark");
    expect(parseTheme("light")).toBe("light");
    expect(parseTheme("auto")).toBe("auto");
    for (const bad of [null, undefined, "", "DARK", "blue", "dark "]) {
      expect(parseTheme(bad)).toBe("auto");
    }
  });

  it("puts a forced theme on the page and leaves the page alone for auto", () => {
    const root = document.documentElement;
    applyTheme(root, "dark");
    expect(root.getAttribute("data-theme")).toBe("dark");
    applyTheme(root, "light");
    expect(root.getAttribute("data-theme")).toBe("light");
    applyTheme(root, "auto");
    expect(root.hasAttribute("data-theme")).toBe(false);
  });

  it("saves and loads the choice", () => {
    saveTheme(localStorage, "dark");
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    expect(loadTheme(localStorage)).toBe("dark");
  });

  it("copes with storage that is missing or fails", () => {
    expect(loadTheme(null)).toBe("auto");
    saveTheme(null, "dark");
    const broken = {
      getItem() {
        throw new Error("denied");
      },
      setItem() {
        throw new Error("denied");
      },
    };
    expect(loadTheme(broken)).toBe("auto");
    expect(() => saveTheme(broken, "dark")).not.toThrow();
  });

  it("has a label for each choice", () => {
    expect(["auto", "light", "dark"].map((p) => themeLabel(p as "auto"))).toEqual(["Auto", "Light", "Dark"]);
  });
});

describe("the script that runs before the first paint", () => {
  const script = readFileSync("public/theme-init.js", "utf8");
  const run = () => new Function(script)();

  it("forces the stored theme", () => {
    localStorage.setItem(THEME_KEY, "dark");
    run();
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
  });

  it("leaves auto and unknown values to the system", () => {
    for (const value of ["auto", "purple", ""]) {
      document.documentElement.removeAttribute("data-theme");
      localStorage.setItem(THEME_KEY, value);
      run();
      expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    }
  });

  it("uses the same key and values as the app", () => {
    expect(script).toContain(`"${THEME_KEY}"`);
    expect(script).toContain('"light"');
    expect(script).toContain('"dark"');
  });
});

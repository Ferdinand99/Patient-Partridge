// Runs before the page is painted, so a forced theme does not flash the other one first.
// Must stay in step with src/lib/theme.ts (a test checks that it does).
(function () {
  try {
    var value = localStorage.getItem("partridge.theme");
    if (value === "light" || value === "dark") {
      document.documentElement.setAttribute("data-theme", value);
    }
  } catch (e) {
    // No storage: the system decides.
  }
})();

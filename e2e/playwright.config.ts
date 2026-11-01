import { fileURLToPath } from "node:url";
import { defineConfig, devices } from "@playwright/test";

// Browser tests of the real window code (built by Vite) against a stand-in for the Rust side
// (e2e/fake-tauri.js). Run `npm run build` first, then `npm run e2e`.
export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.ts",
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: true,
  workers: 2,
  retries: 0,
  reporter: [["list"]],
  outputDir: "artifacts/results",
  use: {
    baseURL: "http://127.0.0.1:4173",
    viewport: { width: 1100, height: 760 },
    trace: "off",
  },
  webServer: {
    command: "node e2e/serve.mjs",
    // The repository root: the server looks for dist/ and src-tauri/ from there.
    cwd: fileURLToPath(new URL("..", import.meta.url)),
    url: "http://127.0.0.1:4173",
    reuseExistingServer: false,
    timeout: 20_000,
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"], viewport: { width: 1100, height: 760 } } }],
});

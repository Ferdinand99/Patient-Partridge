import { defineConfig } from "vitest/config";

export default defineConfig({
  clearScreen: false,
  server: { port: 1421, strictPort: true },
  build: { target: "es2022" },
  test: { environment: "node", include: ["src/**/*.test.ts", "scripts/**/*.test.mjs"] },
});

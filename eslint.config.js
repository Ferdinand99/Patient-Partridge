import js from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "node_modules", "src-tauri", "target", "vendor", "e2e/artifacts", "e2e/fake-tauri.js", "e2e/fake-pages.js", "e2e/fake-forms.js", "e2e/serve.mjs"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    rules: {
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_" }],
      "@typescript-eslint/no-explicit-any": "error",
    },
  },
  {
    files: ["scripts/**/*.mjs"],
    languageOptions: {
      globals: { process: "readonly", console: "readonly", URL: "readonly", Buffer: "readonly", fetch: "readonly", setTimeout: "readonly" },
    },
  },
);

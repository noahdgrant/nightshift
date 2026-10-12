// The ESLint rules of the JavaScript/TypeScript rule pack. check.sh runs ESLint with this file
// from the directory it checks, so the globs and the tsconfig.json test are relative to that
// directory, and no config of the repo's own is read. README.md says what each rule enforces.
import { existsSync } from "node:fs";
import { defineConfig } from "eslint/config";
import tseslint from "typescript-eslint";

const all = ["**/*.{js,jsx,mjs,cjs,ts,tsx,mts,cts}"];
const ts = ["**/*.{ts,tsx,mts,cts}"];

// The type-aware rules need the repo's tsconfig.json. Without one, TypeScript files get the
// rules that need no type information.
const typed = existsSync("tsconfig.json");

export default defineConfig(
  { ignores: [".nightshift/rules/"] },
  {
    files: all,
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    linterOptions: { reportUnusedDisableDirectives: "error" },
    plugins: { "@typescript-eslint": tseslint.plugin },
    rules: {
      "@typescript-eslint/ban-ts-comment": "error",
      "@typescript-eslint/consistent-type-assertions": ["error", { assertionStyle: "never" }],
      "@typescript-eslint/no-unused-vars": "error",
      eqeqeq: ["error", "always"],
      "max-depth": ["error", 3],
      "max-lines": ["error", { max: 1000, skipBlankLines: false, skipComments: false }],
      "max-params": ["error", 4],
      "no-console": "error",
      "no-empty": ["error", { allowEmptyCatch: false }],
      "no-nested-ternary": "error",
    },
  },
  {
    files: ts,
    extends: [typed ? tseslint.configs.strictTypeChecked : tseslint.configs.strict],
    rules: {
      "@typescript-eslint/consistent-type-imports": "error",
    },
  },
  typed
    ? {
        files: ts,
        languageOptions: {
          parserOptions: { projectService: true, tsconfigRootDir: process.cwd() },
        },
        rules: {
          "@typescript-eslint/no-floating-promises": "error",
          "@typescript-eslint/no-misused-promises": "error",
          "@typescript-eslint/switch-exhaustiveness-check": "error",
        },
      }
    : {},
  {
    files: ["**/scripts/**"],
    rules: { "no-console": "off" },
  },
);

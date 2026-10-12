// The ESLint rules of the JavaScript/TypeScript rule pack. check.sh runs ESLint with this file
// from the directory it checks, so the globs and the tsconfig.json test are relative to that
// directory, and no config of the repo's own is read. README.md says what each rule enforces.
import { existsSync } from "node:fs";
import { relative, resolve } from "node:path";
import { includeIgnoreFile } from "@eslint/compat";
import { defineConfig } from "eslint/config";
import typescript from "typescript";
import tseslint from "typescript-eslint";

const all = ["**/*.{js,jsx,mjs,cjs,ts,tsx,mts,cts}"];
const ts = ["**/*.{ts,tsx,mts,cts}"];

const globSpecials = /[\\[\](){}!]/g;

function escaped(entry) {
  return entry.replace(/^\.\//, "").replace(/\/+$/, "").replace(globSpecials, "\\$&");
}

function tsconfigGlob(entry) {
  const path = escaped(entry);
  return /\.[^/]*$|[*?]$/.test(path) ? path : `${path}/**`;
}

// The type-aware rules need the repo's tsconfig.json, and only the files it includes are in the
// project: its own include, files and exclude patterns say which, so no file is listed one by
// one. The other TypeScript files, and all of them without a tsconfig.json, get the rules that
// need no type information. A tsconfig.json that only has project references puts every
// TypeScript file in the project.
function project() {
  if (!existsSync("tsconfig.json")) return null;
  const read = typescript.readConfigFile("tsconfig.json", typescript.sys.readFile);
  const parsed = typescript.parseJsonConfigFileContent(read.config ?? {}, typescript.sys, process.cwd());
  const named = parsed.fileNames.filter((f) => /\.(?:ts|tsx|mts|cts)$/.test(f) && !relative(process.cwd(), f).startsWith(".."));
  const raw = parsed.raw ?? {};
  if (named.length === 0) {
    const referencesOnly = (parsed.projectReferences ?? []).length > 0 && (raw.files ?? []).length === 0 && !raw.include;
    return referencesOnly ? { files: ts, ignores: [] } : null;
  }
  const files = (raw.files ?? []).map(escaped);
  const include = raw.include ?? (raw.files ? [] : ["**/*"]);
  const exclude = raw.exclude ?? ["node_modules", "bower_components", "jspm_packages"];
  return {
    files: [...files, ...include.map((i) => [tsconfigGlob(i), ...ts])],
    ignores: exclude.map(tsconfigGlob),
  };
}
const typed = project();
const gitignore = resolve(process.cwd(), ".gitignore");

export default defineConfig(
  { ignores: [".nightshift/rules/"] },
  ...(existsSync(gitignore) ? [includeIgnoreFile(gitignore)] : []),
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
    extends: [tseslint.configs.strict],
  },
  {
    files: ts,
    rules: {
      "@typescript-eslint/consistent-type-imports": "error",
    },
  },
  typed
    ? {
        ...typed,
        extends: [tseslint.configs.strictTypeChecked],
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

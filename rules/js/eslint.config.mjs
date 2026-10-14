// The ESLint rules of the JavaScript/TypeScript rule pack. check.sh runs ESLint with this file
// from the directory it checks, so the globs and the tsconfig.json test are relative to that
// directory, and no config of the repo's own is read. README.md says what each rule enforces.
import { existsSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { includeIgnoreFile } from "@eslint/compat";
import { defineConfig } from "eslint/config";
import { createNodeResolver, importX } from "eslint-plugin-import-x";
import jsxA11y from "eslint-plugin-jsx-a11y";
import react from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";
import typescript from "typescript";
import tseslint from "typescript-eslint";

const all = ["**/*.{js,jsx,mjs,cjs,ts,tsx,mts,cts}"];
const ts = ["**/*.{ts,tsx,mts,cts}"];
const extensions = [".ts", ".tsx", ".mts", ".cts", ".mjs", ".cjs", ".js", ".jsx"];

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

// The React rules turn on when the repo's package.json names react in any dependency field. A
// missing or unreadable package.json is no React.
function usesReact() {
  let pkg;
  try {
    pkg = JSON.parse(readFileSync("package.json", "utf8"));
  } catch {
    return false;
  }
  return ["dependencies", "devDependencies", "peerDependencies"].some((field) => Object.hasOwn(pkg?.[field] ?? {}, "react"));
}

// The entry points other modules may import, as globs of import paths: a JSON array in
// utils/entry-points.json, empty until the repo declares some. no-internal-modules is on only
// when there is at least one.
function entryPoints() {
  const path = new URL("utils/entry-points.json", import.meta.url);
  if (!existsSync(path)) return [];
  const text = readFileSync(path, "utf8");
  if (text.trim() === "") return [];
  let globs;
  try {
    globs = JSON.parse(text);
  } catch (e) {
    throw new Error(`${path.pathname} is not valid JSON (${e.message}); write a JSON array of globs, like ["**/index.js"]`);
  }
  if (!Array.isArray(globs) || !globs.every((g) => typeof g === "string")) {
    throw new Error(`${path.pathname} is not a JSON array of glob strings, like ["**/index.js"]`);
  }
  return globs;
}
const allow = entryPoints();
const gitignore = resolve(process.cwd(), ".gitignore");

export default defineConfig(
  { ignores: [".nightshift/rules/"] },
  ...(existsSync(gitignore) ? [includeIgnoreFile(gitignore)] : []),
  {
    files: all,
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: { ecmaFeatures: { jsx: true } },
      // no-implied-eval checks only the globals the config declares.
      globals: { execScript: "readonly", global: "readonly", setInterval: "readonly", setTimeout: "readonly", window: "readonly" },
    },
    linterOptions: { reportUnusedDisableDirectives: "error" },
    plugins: { "@typescript-eslint": tseslint.plugin, "import-x": importX },
    // import-x follows imports into every file the pack lints, and resolves a `.js` specifier
    // to the `.ts` file TypeScript compiles to it.
    settings: {
      "import-x/extensions": extensions,
      "import-x/resolver-next": [
        createNodeResolver({
          extensions: [...extensions, ".json", ".node"],
          extensionAlias: { ".js": [".ts", ".tsx", ".js", ".jsx"], ".mjs": [".mts", ".mjs"], ".cjs": [".cts", ".cjs"] },
        }),
      ],
    },
    rules: {
      "@typescript-eslint/ban-ts-comment": "error",
      "@typescript-eslint/consistent-type-assertions": ["error", { assertionStyle: "never" }],
      "@typescript-eslint/no-unused-vars": "error",
      eqeqeq: ["error", "always"],
      "import-x/first": "error",
      "import-x/no-cycle": "error",
      ...(allow.length > 0 ? { "import-x/no-internal-modules": ["error", { allow }] } : {}),
      "max-depth": ["error", 3],
      "max-lines": ["error", { max: 1000, skipBlankLines: false, skipComments: false }],
      "max-params": ["error", 4],
      "no-console": "error",
      "no-empty": ["error", { allowEmptyCatch: false }],
      "no-eval": "error",
      "no-implied-eval": "error",
      "no-nested-ternary": "error",
      "no-new-func": "error",
    },
  },
  usesReact()
    ? {
        files: all,
        extends: [jsxA11y.flatConfigs.recommended],
        plugins: { react, "react-hooks": reactHooks },
        rules: {
          "react/no-danger": "error",
          "react-hooks/exhaustive-deps": "error",
          "react-hooks/rules-of-hooks": "error",
        },
      }
    : {},
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

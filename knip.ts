import type { KnipConfig } from "knip";

const config: KnipConfig = {
  // Entries come from the Vite (index.html), Vitest, Playwright and package.json script plugins;
  // standalone browser fixtures are loaded by their own HTML pages.
  entry: ["e2e/*.fixture.{ts,tsx}", "perf/**/*.fixture.{ts,tsx}"],
  project: [
    "src/**/*.{ts,tsx,css}",
    "!src/editor/core/cm/angular/parser.terms.ts",
    "!src/editor/core/cm/jsp/parser.terms.ts",
    "!src/editor/core/cm/astro/parser.terms.ts",
    "!src/editor/core/cm/django/parser.terms.ts",
    "scripts/**/*.ts",
    "perf/**/*.ts",
    "evals/**/*.ts",
    "e2e/**/*.ts",
    "smoke/**/*.ts",
  ],
  // Plugins' web pages are projects of their own (own package.json, checked by plugins/build.sh and CI).
  ignore: ["plugins/**/web-src/**", "vendor/**"],
  ignoreExportsUsedInFile: true,
  // Used by the imported Minerva plugin SDK, which is checked separately.
  ignoreDependencies: ["cordis"],
  // System tools used by formatting and language smoke fixtures.
  ignoreBinaries: ["rustfmt", "composer", "make"],
  compilers: {
    // CSS Modules can share styles through composes, alongside ordinary imports.
    css: (text: string) =>
      [...text.matchAll(/(?:@import|composes:[^;]*?\bfrom)\s+("[^"]+"|'[^']+')/g)]
        .map(([, path]) => `import ${path};`)
        .join("\n"),
  },
};

export default config;

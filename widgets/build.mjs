// Build the distributable bundles for @freedback/widgets.
//
// The canonical source of truth stays `freedback-widgets.js` (an IIFE so the
// `<script src>` path keeps working with NO build step). This script wraps/emits
// the two distributables consumers `npm add` + `import`:
//
//   dist/freedback-widgets.esm.js  — ESM. Importing it registers the elements
//                                    (side effect) AND re-exports the helpers.
//   dist/freedback-widgets.umd.js  — UMD/IIFE for `<script src>` users; bundles
//                                    the canonical IIFE so it behaves identically
//                                    (registers elements, sets window.Freedback,
//                                    and exposes the helpers as a global module).
//
// No runtime dependencies — esbuild is the only (dev) tool, matching the widgets'
// no-framework-deps ethos.
import { build } from "esbuild";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { readFile } from "node:fs/promises";

const here = dirname(fileURLToPath(import.meta.url));
const r = (p) => resolve(here, p);

// The canonical source (`freedback-widgets.js`) is CommonJS (it sets
// `module.exports`), but this package is `"type": "module"`, so esbuild would
// otherwise treat the on-disk `.js` as ESM and find no exports. This plugin
// resolves the canonical import into a plugin-owned namespace (which has no
// governing package.json `type`), so esbuild auto-detects CommonJS from the
// `module.exports` syntax and its exports become the bundle's default export —
// without renaming the file the `<script>` path, Pages, and demos depend on.
const CANONICAL = r("freedback-widgets.js");
const cjsCanonical = {
  name: "cjs-canonical",
  setup(b) {
    b.onResolve({ filter: /(^|\/)freedback-widgets\.js$/ }, (args) => {
      // Only intercept references to the canonical source file.
      if (args.kind === "entry-point" || args.importer) {
        return { path: CANONICAL, namespace: "fb-canonical" };
      }
      return null;
    });
    b.onLoad({ filter: /.*/, namespace: "fb-canonical" }, async () => ({
      contents: await readFile(CANONICAL, "utf8"),
      loader: "js",
      resolveDir: here,
    }));
  },
};

const banner = {
  js: "/* @freedback/widgets — generated bundle. Source of truth: freedback-widgets.js */",
};

async function main() {
  // ESM build: side-effect registration + named/default helper exports.
  await build({
    entryPoints: [r("src/index.js")],
    outfile: r("dist/freedback-widgets.esm.js"),
    bundle: true,
    format: "esm",
    platform: "browser",
    target: ["es2020"],
    legalComments: "inline",
    banner,
    plugins: [cjsCanonical],
  });

  // UMD/IIFE build for <script src>: bundle the canonical IIFE as-is. We expose
  // its CommonJS exports under a `globalName` so `<script>` users who want the
  // helpers can reach them at `window.FreedbackWidgets` too; the element
  // registration + `window.Freedback` identity API run as side effects exactly
  // like loading the raw source file.
  await build({
    entryPoints: [r("freedback-widgets.js")],
    outfile: r("dist/freedback-widgets.umd.js"),
    bundle: true,
    format: "iife",
    globalName: "FreedbackWidgets",
    platform: "browser",
    target: ["es2020"],
    legalComments: "inline",
    banner,
    plugins: [cjsCanonical],
  });

  // Log to stderr so it never pollutes a stdout JSON stream (e.g. when this runs
  // as the `prepack` hook under `npm pack --json`).
  console.error("widgets: built dist/freedback-widgets.esm.js + dist/freedback-widgets.umd.js");
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});

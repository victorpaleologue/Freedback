// Compile the repository's `docs/` Markdown into a browsable HTML section of
// the freedback.net static site (served at /docs/). Run from `docs-tools/`:
//
//   node build-docs.mjs                 # writes ../_site/docs/**
//   OUT=/somewhere node build-docs.mjs  # override the output dir (local preview)
//
// Design goals:
//   * Every page shares the site's look (freedback.css + theme.js), so the docs
//     feel like part of freedback.net, not a raw Markdown dump.
//   * Intra-docs `*.md` links become `*.html` (and `README.md` → `index.html`),
//     so navigation stays inside the compiled site.
//   * Links that escape `docs/` (../CLAUDE.md, code paths, …) resolve to the
//     GitHub blob, so nothing dead-ends.
//   * The ADR folder gets an auto-generated index listing every decision.
import { readFileSync, writeFileSync, mkdirSync, readdirSync, statSync, copyFileSync } from "node:fs";
import { dirname, join, relative, resolve, basename } from "node:path";
import { fileURLToPath } from "node:url";
import { marked } from "marked";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, "..");
const DOCS = join(REPO, "docs");
const OUT = process.env.OUT || join(REPO, "_site", "docs");
const REPO_BLOB = "https://github.com/victorpaleologue/Freedback/blob/main";
const MERMAID_SRC = join(HERE, "vendor", "mermaid.min.js");

marked.setOptions({ gfm: true });

// Render a ```mermaid fence as mermaid's own container (<pre class="mermaid">)
// instead of a highlighted code block, and flag the page so it loads the
// renderer. The diagram text is HTML-escaped; the browser decodes the entities
// back in `textContent`, which is what mermaid reads — so arrows like `-->` /
// `<-->` survive intact. Any other language falls through to the default.
let pageUsesMermaid = false;
marked.use({
  renderer: {
    code({ text, lang }) {
      if ((lang || "").trim() === "mermaid") {
        pageUsesMermaid = true;
        return `<pre class="mermaid">${escapeHtml(text)}</pre>\n`;
      }
      return false; // defer to marked's default code renderer
    },
  },
});

/** Client-side init injected only into pages that contain a mermaid diagram.
 *  Self-hosted (no CDN), theme-picked from the page's current theme at load. */
const MERMAID_INIT = `
  <script src="/docs/mermaid.min.js"></script>
  <script>
    (function () {
      var ns = window.__esbuild_esm_mermaid_nm && window.__esbuild_esm_mermaid_nm.mermaid;
      var mermaid = ns && (ns.default || ns);
      if (!mermaid || !mermaid.initialize) return;
      var root = document.documentElement;
      var pinned = root.getAttribute("data-theme");
      var dark = pinned === "dark" ||
        (!pinned && window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches);
      mermaid.initialize({ startOnLoad: false, theme: dark ? "dark" : "default" });
      mermaid.run();
    })();
  </script>`;

/** Every Markdown file under docs/, repo-relative-ish (relative to docs/). */
function mdFiles(dir = DOCS, acc = []) {
  for (const name of readdirSync(dir)) {
    const abs = join(dir, name);
    if (statSync(abs).isDirectory()) mdFiles(abs, acc);
    else if (name.endsWith(".md")) acc.push(relative(DOCS, abs));
  }
  return acc;
}

/** The set of doc paths (relative to docs/) that WILL exist as HTML, so a link
 *  to one can be rewritten locally rather than sent to GitHub. */
const docSet = new Set(mdFiles());

/** First `# ` heading, else a title-cased filename. */
function titleOf(md, relPath) {
  const m = md.match(/^#\s+(.+)$/m);
  if (m) return m[1].replace(/`/g, "").trim();
  return basename(relPath, ".md");
}

/** Map a doc's source path (relative to docs/) to its output path. `README.md`
 *  becomes `index.html` so a folder dereferences to its index. */
function outPathFor(relPath) {
  const html = relPath.replace(/\.md$/, ".html");
  return html.replace(/(^|\/)README\.html$/, "$1index.html");
}

/** Rewrite one relative link `href` found in the compiled page at `fromRel`
 *  (path relative to docs/): intra-docs `.md` → `.html` (README → index),
 *  directory links → their generated index, out-of-docs links → GitHub blob.
 *  External / anchor / site-absolute hrefs pass through untouched. */
function fixHref(href, fromRel) {
  if (/^([a-z]+:|\/\/|#|\/)/i.test(href)) return href; // external / anchor / site-absolute
  const hashAt = href.indexOf("#");
  const pathPart = hashAt === -1 ? href : href.slice(0, hashAt);
  const suffix = hashAt === -1 ? "" : href.slice(hashAt);
  if (!pathPart) return href;

  const fromDir = dirname(fromRel);
  const targetAbs = resolve(DOCS, fromDir, pathPart);
  const insideDocs = targetAbs === DOCS || targetAbs.startsWith(DOCS + "/");
  const relFromDocs = relative(DOCS, targetAbs);

  if (insideDocs) {
    if (pathPart.endsWith(".md")) {
      return outPathFor(relFromDocs) + suffix;
    }
    // A directory link (e.g. `adr` or `./adr`): point at its generated index.
    let isDir = false;
    try {
      isDir = statSync(targetAbs).isDirectory();
    } catch {
      isDir = false;
    }
    if (isDir) return (relFromDocs ? relFromDocs + "/" : "") + "index.html" + suffix;
    return href; // some other in-docs asset — leave as-is
  }

  // Escapes docs/: send to the GitHub blob so it still resolves.
  const repoRel = relative(REPO, targetAbs);
  return `${REPO_BLOB}/${repoRel}${suffix}`;
}

/** Rewrite every href/src in a compiled HTML fragment. */
function rewriteLinks(html, fromRel) {
  return html.replace(/\b(href|src)="([^"]*)"/g, (m, attr, val) => {
    const fixed = attr === "href" ? fixHref(val, fromRel) : val;
    return `${attr}="${fixed}"`;
  });
}

/** Wrap a rendered fragment in the site chrome. All chrome URLs are
 *  site-absolute (/freedback.css, /docs/, …), so page depth doesn't matter. */
function page(title, bodyHtml, { isIndex = false, hasMermaid = false } = {}) {
  const nav = isIndex
    ? `<p class="doc-nav"><a href="/">← Home</a></p>`
    : `<p class="doc-nav"><a href="/docs/">← All docs</a> &nbsp;·&nbsp; <a href="/">Home</a></p>`;
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>${escapeHtml(title)} — Freedback docs</title>
  <link rel="stylesheet" href="/freedback.css" />
  <script src="/theme.js"></script>
</head>
<body>
  <button class="fb-theme-toggle" data-fb-theme-toggle aria-label="Toggle light / dark theme"><span data-fb-theme-icon>🌙</span></button>
  <main class="doc">
    ${nav}
    ${bodyHtml}
    <footer>
      <p>Source &amp; docs: <a href="https://github.com/victorpaleologue/Freedback">github.com/victorpaleologue/Freedback</a> &nbsp;·&nbsp; <a href="/">freedback.net</a> &nbsp;·&nbsp; <a href="/privacy/">Privacy</a></p>
    </footer>
  </main>${hasMermaid ? MERMAID_INIT : ""}
</body>
</html>
`;
}

function escapeHtml(s) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function write(relOut, html) {
  const abs = join(OUT, relOut);
  mkdirSync(dirname(abs), { recursive: true });
  writeFileSync(abs, html);
}

// --- compile every doc ------------------------------------------------------
const compiled = [];
let anyMermaid = false;
for (const relPath of docSet) {
  const md = readFileSync(join(DOCS, relPath), "utf8");
  const title = titleOf(md, relPath);
  pageUsesMermaid = false; // set by the mermaid code renderer during parse
  const body = rewriteLinks(marked.parse(md), relPath);
  const hasMermaid = pageUsesMermaid;
  anyMermaid = anyMermaid || hasMermaid;
  const outRel = outPathFor(relPath);
  write(outRel, page(title, body, { isIndex: outRel === "index.html", hasMermaid }));
  compiled.push({ relPath, outRel, title });
}

// Self-host the mermaid bundle only if some page actually draws a diagram.
if (anyMermaid) {
  mkdirSync(OUT, { recursive: true });
  copyFileSync(MERMAID_SRC, join(OUT, "mermaid.min.js"));
}

// --- auto ADR index ---------------------------------------------------------
// `adr/` is linked as a directory; generate a listing so it dereferences.
const adrs = compiled
  .filter((c) => c.relPath.startsWith("adr/") && c.outRel !== "adr/index.html")
  .sort((a, b) => a.outRel.localeCompare(b.outRel));
if (adrs.length) {
  const items = adrs
    .map((c) => {
      const href = c.outRel.slice("adr/".length); // relative to adr/
      return `<li><a href="${href}">${escapeHtml(c.title)}</a></li>`;
    })
    .join("\n      ");
  const body = `<h1>Architecture decision records</h1>
    <p class="sub">The non-obvious calls, written up as they were made — in
    chronological order. Each records the context, the decision, the
    alternatives weighed, and the consequences.</p>
    <ul class="doc-list">
      ${items}
    </ul>`;
  write("adr/index.html", page("Architecture decision records", body));
  compiled.push({ relPath: "adr/index.md", outRel: "adr/index.html", title: "ADRs" });
}

console.log(`docs → HTML: ${compiled.length} page(s) written to ${OUT}`);
for (const c of compiled.sort((a, b) => a.outRel.localeCompare(b.outRel))) {
  console.log(`  ${c.outRel}`);
}

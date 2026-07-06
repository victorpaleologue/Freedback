# Vendored assets

## `mermaid.min.js`

Mermaid **11.16.0**, the standalone browser bundle
(`node_modules/mermaid/dist/mermaid.min.js` from `npm i mermaid@11.16.0`).

Vendored (rather than pulled as an npm dependency of `docs-tools`) on purpose:
the docs compiler only needs to *copy* this one self-contained file into the
built site — it never renders Mermaid itself. Keeping it out of `package.json`
means `npm ci` here installs only `marked`, so the Pages build stays lean and
deterministic. The docs pages load it client-side to render any ```mermaid```
diagram; GitHub renders the same fences natively.

To update: `npm i mermaid@<new>` in a scratch dir, copy its
`dist/mermaid.min.js` here, and bump the version above.

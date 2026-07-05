// ESM entry for @freedback/widgets.
//
// Importing this module for its side effect registers the six custom elements
// (`<freedback-stars/thumb/scalar/comment/issue/tag>`) in the browser — exactly as the
// `<script>` build does — and re-exports the pure helper + identity API that the
// canonical IIFE exposes via `module.exports`.
//
// The canonical source of truth is `../freedback-widgets.js` (an IIFE so the
// `<script src>` path keeps working with no build). esbuild bundles it here: the
// IIFE runs (side effect: `customElements.define` + `window.Freedback`), and its
// CommonJS `module.exports` becomes this module's default export, which we
// re-publish as named ESM exports below.
import helpers from "../freedback-widgets.js";

export const {
  baseAnnotation,
  canonicalContent,
  jcs,
  ratingValue,
  textBodies,
  readUrl,
  starBody,
  thumbBody,
  scalarBody,
  textBody,
  buildSignedAnnotation,
  deleteDocument,
  buildSignedDelete,
  dedupFromId,
  getIdentity,
  generateKeyRecord,
  identityFromRecord,
  wrapIdentity,
  unwrapIdentity,
  buildRotationLink,
  exportIdentity,
  importIdentity,
  rotateIdentity,
} = helpers;

export default helpers;

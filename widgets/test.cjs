// Node unit tests for the pure widget helpers (no DOM needed).
// Run: node widgets/test.cjs   (Node 18+ for the WebCrypto signing test)
const assert = require("node:assert");
const fs = require("node:fs");
const path = require("node:path");

// The canonical source is CommonJS (sets `module.exports`), but the package is
// `"type": "module"`, so a plain `require("./freedback-widgets.js")` would have
// Node classify the `.js` as ESM and find no exports. We instead compile the
// source as CommonJS in THIS realm (via `new Function`, not `vm` — so emitted
// Arrays/Objects share the test's prototypes and `deepStrictEqual` works) while
// faking a minimal DOM. This test (a) keeps testing the EXACT canonical file the
// `<script>` path ships, and (b) also exercises the custom-element registration
// + the `freedback:published` / `freedback:error` outcome events (eventsTest).
function loadWidgets() {
  const src = fs.readFileSync(path.join(__dirname, "freedback-widgets.js"), "utf8");
  // A tiny synchronous DOM stub: enough for `customElements.define`, element
  // construction, attribute access, querySelector returning inert stubs, and
  // CustomEvent dispatch. No network — submit() is driven directly in the test.
  const defined = {};
  function makeNode() {
    return {
      _attrs: {},
      _listeners: {},
      innerHTML: "",
      textContent: "",
      children: [],
      setAttribute(k, v) {
        this._attrs[k] = String(v);
      },
      getAttribute(k) {
        return k in this._attrs ? this._attrs[k] : null;
      },
      hasAttribute(k) {
        return k in this._attrs;
      },
      querySelector() {
        return makeNode();
      },
      querySelectorAll() {
        return [];
      },
      appendChild(c) {
        this.children.push(c);
        return c;
      },
      addEventListener(type, fn) {
        (this._listeners[type] ||= []).push(fn);
      },
      removeEventListener(type, fn) {
        this._listeners[type] = (this._listeners[type] || []).filter((f) => f !== fn);
      },
      dispatchEvent(ev) {
        for (const fn of this._listeners[ev.type] || []) fn(ev);
        return true;
      },
    };
  }
  class HTMLElement {
    constructor() {
      Object.assign(this, makeNode());
    }
  }
  class CustomEvent {
    constructor(type, init) {
      this.type = type;
      this.detail = (init || {}).detail;
      this.bubbles = !!(init || {}).bubbles;
      this.composed = !!(init || {}).composed;
    }
  }
  // A swappable fetch stub: tests set `state.__fetch` to control responses.
  const state = { __fetch: null };
  const fetchStub = (...args) =>
    state.__fetch
      ? state.__fetch(...args)
      : Promise.reject(new Error("no fetch stub installed"));
  const mod = { exports: {} };
  // Inject the fake-DOM + CJS env as locals of the wrapped source (same realm).
  // Globals the source needs but we DON'T shadow (crypto, TextEncoder/Decoder)
  // fall through to Node's real globals. `window` is intentionally not injected,
  // so the `window.Freedback` branch is skipped (the test uses module.exports);
  // `typeof window` stays safe because it's only read inside a `typeof` guard.
  const wrapper = new Function(
    "module",
    "exports",
    "HTMLElement",
    "CustomEvent",
    "customElements",
    "document",
    "fetch",
    "btoa",
    "atob",
    src + "\n//# sourceURL=freedback-widgets.js"
  );
  wrapper(
    mod,
    mod.exports,
    HTMLElement,
    CustomEvent,
    { define: (n, c) => (defined[n] = c) },
    { createElement: () => makeNode() },
    fetchStub,
    (s) => Buffer.from(s, "binary").toString("base64"),
    (s) => Buffer.from(s, "base64").toString("binary")
  );
  return { exports: mod.exports, defined, state, CustomEvent, makeNode };
}

const widgets = loadWidgets();
const {
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
  generateKeyRecord,
  identityFromRecord,
  wrapIdentity,
  unwrapIdentity,
  buildRotationLink,
} = widgets.exports;

// baseAnnotation emits the W3C wire shape.
const star = baseAnnotation("assessing", "https://ex/1", starBody(4));
assert.strictEqual(star.type, "Annotation");
assert.strictEqual(star.motivation, "assessing");
assert.strictEqual(star.conformsTo, "https://freedback.net/profile/1");
assert.ok(Array.isArray(star.body) && star.body.length === 1);
assert.ok(Array.isArray(star["@context"]));

// data licensing (ADR 0022): an optional license IRI lands on `rights`.
const LICENSE = "https://creativecommons.org/licenses/by/4.0/";
const licensed = baseAnnotation("assessing", "https://ex/1", starBody(4), LICENSE);
assert.strictEqual(licensed.rights, LICENSE, "baseAnnotation carries the license as rights");
assert.strictEqual(star.rights, undefined, "no license -> no rights key at all");

// ratingValue pulls the numeric value out of a rating body.
assert.strictEqual(ratingValue(star), 4);
assert.strictEqual(ratingValue({ body: [textBody("hi", "commenting")] }), null);

// textBodies extracts comment/tag text by purpose.
const commented = { body: [textBody("nice", "commenting")] };
assert.deepStrictEqual(textBodies(commented, "commenting"), ["nice"]);
assert.deepStrictEqual(textBodies(commented, "tagging"), []);
const tagged = { body: [textBody("rust", "tagging")] };
assert.deepStrictEqual(textBodies(tagged, "tagging"), ["rust"]);

// issue / problem report (ADR 0023): an oa:TextualBody with the standard
// "editing" purpose/motivation — zero new vocabulary.
const issued = { body: [textBody("broken link", "editing")] };
assert.deepStrictEqual(textBodies(issued, "editing"), ["broken link"]);
assert.deepStrictEqual(textBodies(issued, "commenting"), []);
const issueAnn = baseAnnotation("editing", "https://ex/1", textBody("broken link", "editing"));
assert.strictEqual(issueAnn.motivation, "editing");
assert.strictEqual(issueAnn.body[0].purpose, "editing");

// readUrl appends the encoded target and an unbounded page_size: a target
// past the server's default page size (oldest-first) would otherwise lose
// its newest — and thus the viewer's own — items from the widget's own read.
assert.strictEqual(
  readUrl("http://h/index", "https://ex/1"),
  "http://h/index?target=https%3A%2F%2Fex%2F1&page_size=0"
);
assert.strictEqual(readUrl("http://h/index?x=1", "a"), "http://h/index?x=1&target=a&page_size=0");

// --- RFC 8785 JCS cross-language conformance ------------------------------
// This exact string is the output of Rust's `serde_json_canonicalizer` over the
// equivalent `Annotation` (see crates/protocol-lib canonical.rs). If the two
// ever diverge, signatures made in the browser would stop verifying server-side,
// so this is the keystone test for WebCrypto signing (ADR 0013).
const EXPECTED_CANONICAL =
  '{"@context":["http://www.w3.org/ns/anno.jsonld","https://freedback.net/ns/context.jsonld"],' +
  '"body":[{"schema:bestRating":5,"schema:ratingValue":4,"schema:worstRating":1,' +
  '"type":["freedback:StarRating","schema:Rating"]}],' +
  '"conformsTo":"https://freedback.net/profile/1","created":"2026-06-21T10:00:00Z",' +
  '"creator":{"id":"urn:freedback:key:abc"},"motivation":"assessing",' +
  '"target":"https://example.com/item/1","type":"Annotation"}';

const content = canonicalContent(
  "assessing",
  "https://example.com/item/1",
  starBody(4),
  "urn:freedback:key:abc",
  "2026-06-21T10:00:00Z"
);
assert.strictEqual(jcs(content), EXPECTED_CANONICAL, "JCS must byte-match the Rust canonicalizer");

// With a license, `rights` joins the canonical (signed) bytes in JCS key order —
// this exact string is pinned against the Rust canonicalizer in
// crates/protocol-lib/src/canonical.rs (licensed_widget_content_canonical_bytes),
// so a license set in the browser signs/verifies identically server-side.
const EXPECTED_CANONICAL_LICENSED = EXPECTED_CANONICAL.replace(
  '"motivation":"assessing",',
  `"motivation":"assessing","rights":"${LICENSE}",`
);
const licensedContent = canonicalContent(
  "assessing",
  "https://example.com/item/1",
  starBody(4),
  "urn:freedback:key:abc",
  "2026-06-21T10:00:00Z",
  LICENSE
);
assert.strictEqual(
  jcs(licensedContent),
  EXPECTED_CANONICAL_LICENSED,
  "licensed JCS must byte-match the Rust canonicalizer"
);

// Cross-language pin for the ISSUE feedback type (ADR 0023): this exact string
// is also asserted against the Rust canonicalizer in
// crates/protocol-lib/src/canonical.rs (issue_canonical_bytes_and_dedup_id_are_
// pinned), so an issue signed in the browser verifies server-side.
const EXPECTED_CANONICAL_ISSUE =
  '{"@context":["http://www.w3.org/ns/anno.jsonld","https://freedback.net/ns/context.jsonld"],' +
  '"body":[{"format":"text/plain","purpose":"editing","type":"TextualBody",' +
  '"value":"the checkout button does nothing"}],' +
  '"conformsTo":"https://freedback.net/profile/1","created":"2026-06-21T10:00:00Z",' +
  '"creator":{"id":"urn:freedback:key:abc"},"motivation":"editing",' +
  '"target":"https://example.com/item/1","type":"Annotation"}';
const issueContent = canonicalContent(
  "editing",
  "https://example.com/item/1",
  textBody("the checkout button does nothing", "editing"),
  "urn:freedback:key:abc",
  "2026-06-21T10:00:00Z"
);
assert.strictEqual(
  jcs(issueContent),
  EXPECTED_CANONICAL_ISSUE,
  "issue JCS must byte-match the Rust canonicalizer"
);

// JCS invariants: key order independence, number form, array order preserved.
assert.strictEqual(jcs({ b: 1, a: 2 }), '{"a":2,"b":1}');
assert.strictEqual(jcs({ x: 4.0 }), '{"x":4}'); // 4.0 -> "4" like Rust
assert.strictEqual(jcs({ x: 0.5 }), '{"x":0.5}');
assert.strictEqual(jcs([3, 1, 2]), "[3,1,2]");

// --- delete document: exact canonical bytes (right to erasure, ADR 0021) ---
// This exact string mirrors the Rust reference test
// `canonical_bytes_are_stable_and_exclude_signature` in
// crates/protocol-lib/src/erasure.rs (same DEDUP/CREATED fixture, same JCS
// form: keys sorted, signature excluded). If the two ever diverge, a delete
// signed in the browser would stop verifying server-side.
const DEL_DEDUP = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const DEL_CREATED = "2026-07-05T12:00:00Z";
const EXPECTED_DELETE_CANONICAL =
  `{"annotation":"${DEL_DEDUP}","created":"${DEL_CREATED}","type":"Delete"}`;
assert.strictEqual(
  jcs(deleteDocument(DEL_DEDUP, DEL_CREATED)),
  EXPECTED_DELETE_CANONICAL,
  "delete-doc JCS must byte-match the Rust canonical bytes (erasure.rs)"
);
// The document shape itself matches protocol-lib's DeleteRequest.
assert.deepStrictEqual(deleteDocument(DEL_DEDUP, DEL_CREATED), {
  type: "Delete",
  annotation: DEL_DEDUP,
  created: DEL_CREATED,
});

// dedupFromId: the dedup id is the basename of the server-minted item id.
assert.strictEqual(dedupFromId(`http://127.0.0.1:8080/annotations/${DEL_DEDUP}`), DEL_DEDUP);
assert.strictEqual(dedupFromId(`http://h/annotations/${DEL_DEDUP}?x=1#frag`), DEL_DEDUP);
assert.strictEqual(dedupFromId("urn:freedback:mock:7"), "urn:freedback:mock:7");
assert.strictEqual(dedupFromId(null), null);
assert.strictEqual(dedupFromId(""), null);

// Body builders match the canonical wire shape.
assert.deepStrictEqual(Object.keys(starBody(4)).sort(), [
  "schema:bestRating",
  "schema:ratingValue",
  "schema:worstRating",
  "type",
]);
assert.strictEqual(thumbBody(true)["schema:ratingValue"], 1);
assert.strictEqual(thumbBody(false)["schema:ratingValue"], 0);
assert.deepStrictEqual(scalarBody(7, 0, 10), {
  type: ["freedback:ScalarRating", "schema:Rating"],
  "schema:ratingValue": 7,
  "schema:worstRating": 0,
  "schema:bestRating": 10,
});

// --- WebCrypto signing pipeline (ES256 over the JCS bytes) ----------------
// Proves buildSignedAnnotation produces a structurally valid, self-consistent
// signature over exactly jcs(content): re-canonicalize the emitted annotation
// (minus signature) and verify the ES256 signature with the public key. Because
// the signed bytes are JCS (pinned identical to Rust above) and ES256 is
// standard, such a signature also verifies in the Rust server.
async function signingTest() {
  const sc = globalThis.crypto && globalThis.crypto.subtle;
  if (!sc) {
    console.log("widgets: skipping WebCrypto signing test (no crypto.subtle)");
    return;
  }
  const kp = await sc.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const spki = await sc.exportKey("spki", kp.publicKey);
  const digest = await sc.digest("SHA-256", spki);
  const issuerId =
    "urn:freedback:key:" +
    [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
  const pem = (() => {
    const b64 = Buffer.from(new Uint8Array(spki)).toString("base64");
    return `-----BEGIN PUBLIC KEY-----\n${b64.match(/.{1,64}/g).join("\n")}\n-----END PUBLIC KEY-----\n`;
  })();

  const ident = { priv: kp.privateKey, issuerId, kid: pem };
  const ann = await buildSignedAnnotation(
    "assessing",
    "https://example.com/item/1",
    scalarBody(0.7, 0, 1),
    ident,
    "2026-06-21T10:00:00Z"
  );

  assert.strictEqual(ann.signature.alg, "ES256");
  assert.strictEqual(ann.signature.kid, pem);
  assert.strictEqual(ann.creator.id, issuerId);

  // Recompute the signed bytes from the emitted annotation (drop signature).
  const { signature, ...emittedContent } = ann;
  const bytes = new TextEncoder().encode(jcs(emittedContent));
  const sig = Uint8Array.from(
    Buffer.from(signature.sig.replace(/-/g, "+").replace(/_/g, "/"), "base64")
  );
  const ok = await sc.verify({ name: "ECDSA", hash: "SHA-256" }, kp.publicKey, sig, bytes);
  assert.ok(ok, "the detached ES256 signature must verify over the JCS bytes");
  assert.strictEqual(sig.length, 64, "raw R||S signature is 64 bytes");

  // A license rides in the SIGNED content (ADR 0022): the emitted annotation
  // carries `rights` and the detached signature still verifies over it.
  const licAnn = await buildSignedAnnotation(
    "assessing",
    "https://example.com/item/1",
    starBody(4),
    ident,
    "2026-06-21T10:00:00Z",
    "https://creativecommons.org/licenses/by/4.0/"
  );
  assert.strictEqual(licAnn.rights, "https://creativecommons.org/licenses/by/4.0/");
  const { signature: licSig, ...licContent } = licAnn;
  const licBytes = new TextEncoder().encode(jcs(licContent));
  const licRaw = Uint8Array.from(
    Buffer.from(licSig.sig.replace(/-/g, "+").replace(/_/g, "/"), "base64")
  );
  assert.ok(
    await sc.verify({ name: "ECDSA", hash: "SHA-256" }, kp.publicKey, licRaw, licBytes),
    "the signature covers the rights field"
  );

  // --- signed delete document (right to erasure, ADR 0021) -----------------
  // buildSignedDelete must use the IDENTICAL encoding to annotation signing:
  // kid = SPKI PEM, sig = base64url (no pad) over the JCS bytes of the doc
  // WITHOUT its `signature` field — exactly what the Rust server verifies.
  const DEDUP = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
  const del = await buildSignedDelete(DEDUP, ident, "2026-07-05T12:00:00Z");
  assert.strictEqual(del.type, "Delete");
  assert.strictEqual(del.annotation, DEDUP);
  assert.strictEqual(del.created, "2026-07-05T12:00:00Z");
  assert.strictEqual(del.signature.alg, "ES256");
  assert.strictEqual(del.signature.kid, pem, "delete kid is the same SPKI PEM as annotations");
  assert.ok(!/[+/=]/.test(del.signature.sig), "sig is base64url with no padding");

  // Recompute the signed bytes (doc minus signature) and verify — and pin that
  // those bytes are exactly the Rust canonical form.
  const { signature: delSig, ...delContent } = del;
  const delCanonical = jcs(delContent);
  assert.strictEqual(
    delCanonical,
    `{"annotation":"${DEDUP}","created":"2026-07-05T12:00:00Z","type":"Delete"}`,
    "the signed delete bytes are the Rust canonical bytes (signature excluded)"
  );
  const delBytes = new TextEncoder().encode(delCanonical);
  const delRaw = Uint8Array.from(
    Buffer.from(delSig.sig.replace(/-/g, "+").replace(/_/g, "/"), "base64")
  );
  assert.ok(
    await sc.verify({ name: "ECDSA", hash: "SHA-256" }, kp.publicKey, delRaw, delBytes),
    "the delete's detached ES256 signature must verify over the JCS bytes"
  );
  assert.strictEqual(delRaw.length, 64, "raw R||S delete signature is 64 bytes");
}

// --- identity export / import / rotation (issue #27) ----------------------
// The portable issuer id is the public key; it must survive a password-encrypted
// export → import round-trip, and a wrong password must fail closed. Rotation
// must yield a *new* signing key (new issuer id) while leaving past signatures
// verifiable and emitting a link statement the new key signs over the old id.
async function identityTest() {
  const sc = globalThis.crypto && globalThis.crypto.subtle;
  if (!sc) {
    console.log("widgets: skipping identity mgmt test (no crypto.subtle)");
    return;
  }

  // A fresh extractable key record stands in for what IndexedDB would hold.
  const rec = await generateKeyRecord(sc);
  const ident = await identityFromRecord(sc, rec);
  assert.ok(ident.issuerId.startsWith("urn:freedback:key:"), "issuer id is the key digest");
  assert.ok(ident.kid.includes("BEGIN PUBLIC KEY"), "kid is the SPKI PEM");

  // Password-wrap → unwrap round-trips the SAME issuer id (portable identity).
  const blob = await wrapIdentity(sc, rec, "correct horse battery staple");
  assert.strictEqual(blob.type, "freedback-identity");
  assert.strictEqual(blob.alg, "ES256");
  // The blob carries no plaintext key material (only ciphertext + public spki).
  const blobStr = JSON.stringify(blob);
  assert.ok(!/"d"\s*:/.test(blobStr), "wrapped blob must not leak the private JWK 'd'");

  const restored = await unwrapIdentity(sc, blob, "correct horse battery staple");
  const restoredIdent = await identityFromRecord(sc, restored);
  assert.strictEqual(restoredIdent.issuerId, ident.issuerId, "import restores the same issuer id");
  assert.strictEqual(restoredIdent.kid, ident.kid, "import restores the same kid PEM");

  // The restored private key still signs annotations the original key would have.
  const ann = await buildSignedAnnotation(
    "assessing",
    "https://example.com/item/1",
    starBody(5),
    restoredIdent,
    "2026-06-21T10:00:00Z"
  );
  const { signature, ...emitted } = ann;
  const bytes = new TextEncoder().encode(jcs(emitted));
  const pub = await sc.importKey(
    "spki",
    new Uint8Array(restored.spki),
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["verify"]
  );
  const sig = Uint8Array.from(Buffer.from(signature.sig.replace(/-/g, "+").replace(/_/g, "/"), "base64"));
  assert.ok(
    await sc.verify({ name: "ECDSA", hash: "SHA-256" }, pub, sig, bytes),
    "restored key produces a verifying signature"
  );

  // Wrong password must fail closed (AES-GCM tag mismatch), not silently return.
  await assert.rejects(
    () => unwrapIdentity(sc, blob, "wrong password"),
    /wrong password or corrupt backup/,
    "a bad password must be rejected"
  );

  // Rotation yields a NEW signing key (different issuer id) and a link the new
  // key signs vouching for the old issuer id, keeping history attributable.
  const newRec = await generateKeyRecord(sc);
  const newIdent = await identityFromRecord(sc, newRec);
  assert.notStrictEqual(newIdent.issuerId, ident.issuerId, "rotation produces a new issuer id");

  const link = await buildRotationLink(sc, ident, newRec);
  assert.strictEqual(link.statement.oldIssuer, ident.issuerId, "link carries the old issuer id");
  assert.strictEqual(link.statement.newIssuer, newIdent.issuerId, "link carries the new issuer id");
  assert.strictEqual(link.signature.kid, newIdent.kid, "link is signed by the NEW key");

  // Verify the link signature with the new public key over the canonical bytes.
  const linkBytes = new TextEncoder().encode(jcs(link.statement));
  const newPub = await sc.importKey(
    "spki",
    new Uint8Array(newRec.spki),
    { name: "ECDSA", namedCurve: "P-256" },
    true,
    ["verify"]
  );
  const linkSig = Uint8Array.from(
    Buffer.from(link.signature.sig.replace(/-/g, "+").replace(/_/g, "/"), "base64")
  );
  assert.ok(
    await sc.verify({ name: "ECDSA", hash: "SHA-256" }, newPub, linkSig, linkBytes),
    "rotation link verifies under the new key"
  );

  // The OLD key's past signature still verifies independently of rotation.
  assert.ok(
    await sc.verify({ name: "ECDSA", hash: "SHA-256" }, pub, sig, bytes),
    "past self-signed annotations stay valid after rotation"
  );
}

// --- outcome events (gap #4) ----------------------------------------------
// submit() dispatches `freedback:published` on success (detail = the server
// response + the sent annotation) and `freedback:error` on failure (detail =
// the error), additively to the existing `.fb-agg`/`.fb-status` DOM behavior.
// We drive the registered <freedback-stars> class directly with a stubbed fetch
// (no DOM environment beyond the sandbox stub; the helper path is also covered
// above). data-sign is off, so the OAuth/anonymous publish path is exercised.
async function eventsTest() {
  const Stars = widgets.defined["freedback-stars"];
  assert.ok(Stars, "custom elements registered in the sandbox");
  assert.ok(widgets.defined["freedback-issue"], "<freedback-issue> is registered (ADR 0023)");

  // 1) Success: a 200 publish must fire freedback:published with the response.
  const sent = [];
  widgets.state.__fetch = (url, opts) => {
    sent.push({ url, body: JSON.parse(opts.body) });
    return Promise.resolve({
      ok: true,
      status: 200,
      json: () => Promise.resolve({ id: "urn:stored:1", stored: true }),
    });
  };
  const okEl = new Stars();
  okEl.setAttribute("data-target", "https://example.com/item/ev");
  okEl.setAttribute("data-publish", "https://feedback.example/annotations/");
  // refresh() early-returns without data-read, so no extra fetch is needed.
  let published = null;
  let erroredOnOk = false;
  okEl.addEventListener("freedback:published", (e) => (published = e.detail));
  okEl.addEventListener("freedback:error", () => (erroredOnOk = true));

  await okEl.submit("assessing", starBody(5));
  assert.ok(published, "freedback:published fired on a successful publish");
  assert.strictEqual(published.response.id, "urn:stored:1", "detail carries the server response");
  assert.strictEqual(published.annotation.type, "Annotation", "detail carries the sent annotation");
  assert.strictEqual(published.annotation.body[0]["schema:ratingValue"], 5);
  assert.ok(!erroredOnOk, "no error event on success");
  assert.strictEqual(sent.length, 1, "exactly one publish POST");

  // 1b) data-license → the published annotation carries `rights` (ADR 0022);
  // without the attribute, no `rights` key is sent at all (case 1 above).
  assert.strictEqual(sent[0].body.rights, undefined, "no data-license -> no rights");
  const licEl = new Stars();
  licEl.setAttribute("data-target", "https://example.com/item/ev");
  licEl.setAttribute("data-publish", "https://feedback.example/annotations/");
  licEl.setAttribute("data-license", "https://creativecommons.org/licenses/by/4.0/");
  await licEl.submit("assessing", starBody(3));
  assert.strictEqual(sent.length, 2, "second publish POST");
  assert.strictEqual(
    sent[1].body.rights,
    "https://creativecommons.org/licenses/by/4.0/",
    "data-license lands on the built annotation's rights"
  );

  // 2) Failure: a non-OK publish must fire freedback:error with the error.
  widgets.state.__fetch = () =>
    Promise.resolve({ ok: false, status: 401, text: () => Promise.resolve("nope") });
  const badEl = new Stars();
  badEl.setAttribute("data-target", "https://example.com/item/ev");
  badEl.setAttribute("data-publish", "https://feedback.example/annotations/");
  let errored = null;
  let publishedOnErr = false;
  badEl.addEventListener("freedback:error", (e) => (errored = e.detail));
  badEl.addEventListener("freedback:published", () => (publishedOnErr = true));

  await badEl.submit("assessing", starBody(1));
  assert.ok(errored && errored.error, "freedback:error fired on a failed publish");
  assert.match(String(errored.error.message), /publish failed: 401/, "detail carries the error");
  assert.ok(!publishedOnErr, "no published event on failure");

  // 3) Erasure success: a 204 DELETE must fire freedback:deleted with the
  // dedup id + response (right to erasure, ADR 0021). data-token selects the
  // bearer-authorized delete path (the signed path needs IndexedDB, which the
  // Node sandbox doesn't have; buildSignedDelete is covered in signingTest).
  const DEDUP = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
  const deletes = [];
  widgets.state.__fetch = (url, opts) => {
    deletes.push({ url, method: opts.method, headers: opts.headers, body: JSON.parse(opts.body) });
    return Promise.resolve({ ok: false, status: 204, text: () => Promise.resolve("") });
  };
  const delEl = new Stars();
  delEl.setAttribute("data-target", "https://example.com/item/ev");
  delEl.setAttribute("data-publish", "https://feedback.example/annotations/");
  delEl.setAttribute("data-token", "test-bearer");
  let deleted = null;
  let erroredOnDel = false;
  delEl.addEventListener("freedback:deleted", (e) => (deleted = e.detail));
  delEl.addEventListener("freedback:error", () => (erroredOnDel = true));

  await delEl.erase(DEDUP);
  assert.ok(deleted, "freedback:deleted fired on a 204 delete");
  assert.strictEqual(deleted.annotation, DEDUP, "detail carries the erased dedup id");
  assert.strictEqual(deleted.response.status, 204, "detail carries the DELETE response");
  assert.ok(!erroredOnDel, "no error event on a successful delete");
  assert.strictEqual(deletes.length, 1, "exactly one DELETE request");
  assert.strictEqual(deletes[0].method, "DELETE");
  assert.strictEqual(deletes[0].url, `https://feedback.example/annotations/${DEDUP}`);
  assert.strictEqual(deletes[0].headers.authorization, "Bearer test-bearer");
  assert.strictEqual(deletes[0].body.type, "Delete", "body is the delete document");
  assert.strictEqual(deletes[0].body.annotation, DEDUP);
  assert.ok(deletes[0].body.created, "the delete document carries created");
  assert.strictEqual(deletes[0].body.signature, undefined, "bearer path sends an unsigned doc");

  // 4) Erasure failure: a 403 must fire freedback:error (no deleted event).
  widgets.state.__fetch = () =>
    Promise.resolve({ ok: false, status: 403, text: () => Promise.resolve("not the creator") });
  const delBad = new Stars();
  delBad.setAttribute("data-target", "https://example.com/item/ev");
  delBad.setAttribute("data-publish", "https://feedback.example/annotations/");
  delBad.setAttribute("data-token", "test-bearer");
  let delErrored = null;
  let deletedOnErr = false;
  delBad.addEventListener("freedback:error", (e) => (delErrored = e.detail));
  delBad.addEventListener("freedback:deleted", () => (deletedOnErr = true));

  await delBad.erase(DEDUP);
  assert.ok(delErrored && delErrored.error, "freedback:error fired on a failed delete");
  assert.match(String(delErrored.error.message), /delete failed: 403/, "detail carries the error");
  assert.ok(!deletedOnErr, "no deleted event on failure");
}

signingTest()
  .then(identityTest)
  .then(eventsTest)
  .then(() => console.log("widgets: all helper + JCS + signing + erasure + identity + event tests passed"))
  .catch((e) => {
    console.error(e);
    process.exit(1);
  });

// Node unit tests for the pure widget helpers (no DOM needed).
// Run: node widgets/test.cjs   (Node 18+ for the WebCrypto signing test)
const assert = require("node:assert");
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
  generateKeyRecord,
  identityFromRecord,
  wrapIdentity,
  unwrapIdentity,
  buildRotationLink,
} = require("./freedback-widgets.js");

// baseAnnotation emits the W3C wire shape.
const star = baseAnnotation("assessing", "https://ex/1", starBody(4));
assert.strictEqual(star.type, "Annotation");
assert.strictEqual(star.motivation, "assessing");
assert.strictEqual(star.conformsTo, "https://freedback.org/profile/1");
assert.ok(Array.isArray(star.body) && star.body.length === 1);
assert.ok(Array.isArray(star["@context"]));

// ratingValue pulls the numeric value out of a rating body.
assert.strictEqual(ratingValue(star), 4);
assert.strictEqual(ratingValue({ body: [textBody("hi", "commenting")] }), null);

// textBodies extracts comment/tag text by purpose.
const commented = { body: [textBody("nice", "commenting")] };
assert.deepStrictEqual(textBodies(commented, "commenting"), ["nice"]);
assert.deepStrictEqual(textBodies(commented, "tagging"), []);
const tagged = { body: [textBody("rust", "tagging")] };
assert.deepStrictEqual(textBodies(tagged, "tagging"), ["rust"]);

// readUrl appends the encoded target.
assert.strictEqual(readUrl("http://h/index", "https://ex/1"), "http://h/index?target=https%3A%2F%2Fex%2F1");
assert.strictEqual(readUrl("http://h/index?x=1", "a"), "http://h/index?x=1&target=a");

// --- RFC 8785 JCS cross-language conformance ------------------------------
// This exact string is the output of Rust's `serde_json_canonicalizer` over the
// equivalent `Annotation` (see crates/protocol-lib canonical.rs). If the two
// ever diverge, signatures made in the browser would stop verifying server-side,
// so this is the keystone test for WebCrypto signing (ADR 0013).
const EXPECTED_CANONICAL =
  '{"@context":["http://www.w3.org/ns/anno.jsonld","https://freedback.org/ns/context.jsonld"],' +
  '"body":[{"schema:bestRating":5,"schema:ratingValue":4,"schema:worstRating":1,' +
  '"type":["freedback:StarRating","schema:Rating"]}],' +
  '"conformsTo":"https://freedback.org/profile/1","created":"2026-06-21T10:00:00Z",' +
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

// JCS invariants: key order independence, number form, array order preserved.
assert.strictEqual(jcs({ b: 1, a: 2 }), '{"a":2,"b":1}');
assert.strictEqual(jcs({ x: 4.0 }), '{"x":4}'); // 4.0 -> "4" like Rust
assert.strictEqual(jcs({ x: 0.5 }), '{"x":0.5}');
assert.strictEqual(jcs([3, 1, 2]), "[3,1,2]");

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

signingTest()
  .then(identityTest)
  .then(() => console.log("widgets: all helper + JCS + signing + identity tests passed"))
  .catch((e) => {
    console.error(e);
    process.exit(1);
  });

/*
 * Freedback drop-in widgets (component 3) — vanilla Web Components, no build step.
 *
 *   <freedback-stars
 *      data-target="https://example.com/item/1"
 *      data-read="http://localhost:8100/index"      <!-- collection /index or feedback /annotations/ -->
 *      data-publish="http://localhost:8080/annotations/"
 *      data-sign                                     <!-- self-signed P-256 (WebCrypto); no token needed -->
 *      data-token="optional-oauth-bearer"></freedback-stars>
 *
 * A widget with only `data-read` renders a read-only aggregate (no auth).
 * `data-publish` enables submitting. Two auth paths:
 *   - `data-sign`  → the page holds an extractable P-256 key (WebCrypto,
 *     persisted in IndexedDB); each annotation carries a detached ES256
 *     signature over its RFC 8785 canonical bytes — the *federating* identity
 *     (INVARIANT 4a). The key is extractable only so the user can explicitly
 *     password-encrypt and back it up / move devices (export/import/rotate,
 *     issue #27); it otherwise never leaves the page unencrypted.
 *   - `data-token` → an OAuth bearer (the siloed, non-federating identity).
 * `data-sign` wins when both are present.
 *
 * Every widget kind also accepts an optional `data-license` attribute: a
 * license IRI (e.g. https://creativecommons.org/licenses/by/4.0/) set as the
 * annotation's W3C `rights` property on both the signed and bearer publish
 * paths (data licensing, ADR 0022). It is part of the signed content. Without
 * it, feedback falls under the server's default license (well-known).
 *
 * The native wire format is a W3C Web Annotation; these widgets emit exactly the
 * same shape `freedback-protocol` does in Rust, and the signature is computed
 * over the same JCS bytes the Rust server reconstructs and verifies (ADR 0013).
 */
(() => {
  "use strict";

  const ANNO_CTX = ["http://www.w3.org/ns/anno.jsonld", "https://freedback.net/ns/context.jsonld"];
  const PROFILE = "https://freedback.net/profile/1";

  /** Build the read URL with a target query param.
   *
   * `page_size=0` asks the server for the whole (unbounded) collection rather
   * than its default first page: a widget renders one target's full history,
   * so a target past the server's default page size (e.g. a popular target
   * with 50+ prior items, oldest-first) would otherwise never see its own
   * most recent — and thus the viewer's own — items, breaking both the
   * aggregate and the own-item delete affordance (`isOwn()` can only match
   * annotations actually present in `this.annotations`).
   */
  function readUrl(base, target) {
    const sep = base.includes("?") ? "&" : "?";
    return `${base}${sep}target=${encodeURIComponent(target)}&page_size=0`;
  }

  /** Pull annotations from a `data-read` endpoint (AnnotationPage or array). */
  async function fetchAnnotations(base, target) {
    // `cache: "no-store"` bypasses the browser HTTP cache: the collection
    // response carries `Cache-Control: max-age=...`, and a `refresh()` right
    // after `publish()`/`erase()` reuses the identical read URL, so a
    // caching fetch would silently replay the pre-publish snapshot.
    const resp = await fetch(readUrl(base, target), {
      headers: { accept: "application/ld+json" },
      cache: "no-store",
    });
    if (!resp.ok) throw new Error(`read failed: ${resp.status}`);
    const doc = await resp.json();
    if (Array.isArray(doc)) return doc;
    if (Array.isArray(doc.items)) return doc.items;
    return [];
  }

  /** POST an annotation to a `data-publish` endpoint. */
  async function publish(url, annotation, token) {
    const headers = { "content-type": "application/ld+json" };
    if (token) headers.authorization = `Bearer ${token}`;
    const resp = await fetch(url, { method: "POST", headers, body: JSON.stringify(annotation) });
    if (!resp.ok) throw new Error(`publish failed: ${resp.status} ${await resp.text()}`);
    return resp.json();
  }

  function baseAnnotation(motivation, target, body, rights) {
    const ann = {
      "@context": ANNO_CTX,
      type: "Annotation",
      motivation,
      created: new Date().toISOString(),
      target,
      body: [body],
      conformsTo: PROFILE,
    };
    // Optional data license (ADR 0022): a license IRI on the W3C `rights` term.
    if (rights) ann.rights = rights;
    return ann;
  }

  // --- RFC 8785 JCS canonicalization --------------------------------------
  // Must byte-match `serde_json_canonicalizer` (Rust) for the same logical
  // value, so a signature made here verifies there. Keys are sorted by UTF-16
  // code unit; numbers use the ECMAScript Number→String form RFC 8785 mandates
  // (which `JSON.stringify` implements); strings reuse `JSON.stringify`'s
  // (RFC-8785-compatible) escaping. Cross-checked against the Rust canonical
  // bytes in `widgets/test.cjs` (ADR 0013).
  function jcs(value) {
    if (value === null) return "null";
    const t = typeof value;
    if (t === "boolean") return value ? "true" : "false";
    if (t === "number") {
      if (!Number.isFinite(value)) throw new Error("JCS: non-finite number");
      return JSON.stringify(value);
    }
    if (t === "string") return JSON.stringify(value);
    if (Array.isArray(value)) return "[" + value.map(jcs).join(",") + "]";
    if (t === "object") {
      const keys = Object.keys(value).filter((k) => value[k] !== undefined);
      keys.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
      return "{" + keys.map((k) => JSON.stringify(k) + ":" + jcs(value[k])).join(",") + "}";
    }
    throw new Error(`JCS: unserializable ${t}`);
  }

  /** The signed content: the model shape minus `id`/`signature` (what Rust
   *  canonicalizes on verify). Bodies must already be in canonical wire form.
   *  `rights` (a license IRI, ADR 0022) is content: when present it is part of
   *  the canonical bytes the signature covers. */
  function canonicalContent(motivation, target, body, creatorId, created, rights) {
    const content = {
      "@context": ANNO_CTX,
      type: "Annotation",
      motivation,
      creator: { id: creatorId },
      created,
      target,
      body: [body],
      conformsTo: PROFILE,
    };
    if (rights) content.rights = rights;
    return content;
  }

  // --- WebCrypto P-256 self-signed identity --------------------------------
  const subtle = () => (typeof crypto !== "undefined" && crypto.subtle) || null;

  function hex(bytes) {
    let s = "";
    for (const b of bytes) s += b.toString(16).padStart(2, "0");
    return s;
  }
  function b64(bytes) {
    let bin = "";
    for (const b of bytes) bin += String.fromCharCode(b);
    return btoa(bin);
  }
  function b64url(buf) {
    return b64(new Uint8Array(buf)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }
  function pemFromDer(der) {
    const lines = b64(new Uint8Array(der)).match(/.{1,64}/g).join("\n");
    return `-----BEGIN PUBLIC KEY-----\n${lines}\n-----END PUBLIC KEY-----\n`;
  }

  // Persist the keypair in IndexedDB: an *extractable* private CryptoKey plus
  // the public SPKI DER bytes (for the kid / issuer id). Structured clone stores
  // CryptoKeys without exposing private material to page JS, but extractable:true
  // is required so the user can explicitly password-wrap and export the identity
  // for backup / moving across devices (issue #27, ADR 0013 follow-up). The raw
  // JWK only ever leaves the page encrypted under a user-supplied password.
  const DB_NAME = "freedback";
  const STORE = "identity";
  function openDb() {
    return new Promise((res, rej) => {
      const r = indexedDB.open(DB_NAME, 1);
      r.onupgradeneeded = () => r.result.createObjectStore(STORE);
      r.onsuccess = () => res(r.result);
      r.onerror = () => rej(r.error);
    });
  }
  function idbGet(db, key) {
    return new Promise((res, rej) => {
      const req = db.transaction(STORE, "readonly").objectStore(STORE).get(key);
      req.onsuccess = () => res(req.result || null);
      req.onerror = () => rej(req.error);
    });
  }
  function idbPut(db, key, val) {
    return new Promise((res, rej) => {
      const tx = db.transaction(STORE, "readwrite");
      tx.objectStore(STORE).put(val, key);
      tx.oncomplete = () => res();
      tx.onerror = () => rej(tx.error);
    });
  }

  let identityPromise = null;
  /** Load (or first-run generate) the page's self-signed identity. */
  function getIdentity() {
    if (!identityPromise) identityPromise = loadIdentity();
    return identityPromise;
  }

  /** The local issuer id, computed once per page from the existing identity
   *  machinery — used to recognise the visitor's OWN annotations in fetched
   *  lists (ownership detection for the delete affordance, ADR 0021). Resolves
   *  to null where the identity is unavailable (no secure context). */
  let ownIssuerPromise = null;
  function getOwnIssuerId() {
    if (!ownIssuerPromise) {
      ownIssuerPromise = getIdentity().then(
        (i) => i.issuerId,
        () => null
      );
    }
    return ownIssuerPromise;
  }
  async function loadIdentity() {
    const sc = subtle();
    if (!sc) throw new Error("WebCrypto unavailable (needs a secure context)");
    const db = await openDb();
    let rec = await idbGet(db, "kp");
    if (!rec) {
      rec = await generateKeyRecord(sc);
      await idbPut(db, "kp", rec);
    }
    return identityFromRecord(sc, rec);
  }

  /** Generate a fresh extractable P-256 signing key record (private CryptoKey +
   *  public SPKI DER). Extractable so the user can back it up (issue #27). */
  async function generateKeyRecord(sc) {
    const kp = await sc.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
    const spki = await sc.exportKey("spki", kp.publicKey);
    return { priv: kp.privateKey, spki };
  }

  /** Derive the public identity (issuer id + kid PEM) from a stored record. */
  async function identityFromRecord(sc, rec) {
    const digest = await sc.digest("SHA-256", rec.spki);
    return {
      priv: rec.priv,
      issuerId: "urn:freedback:key:" + hex(new Uint8Array(digest)),
      kid: pemFromDer(rec.spki),
    };
  }

  // --- password-encrypted export / import (issue #27) ----------------------
  // The portable, federating issuer id IS the public key (INVARIANT 4a). To back
  // it up or carry it to another browser/device we wrap the private JWK with a
  // key derived from a user password (PBKDF2-SHA-256 → AES-GCM, the standard
  // WebCrypto recipe). The exported blob is plain JSON (base64url fields) so it
  // survives copy/paste and file download; it carries no plaintext key material.
  const KDF_ITERS = 210000; // OWASP 2023 PBKDF2-SHA256 floor
  const WRAP_VERSION = 1;

  function rand(n) {
    const a = new Uint8Array(n);
    crypto.getRandomValues(a);
    return a;
  }
  function fromB64url(s) {
    const pad = s.length % 4 === 0 ? "" : "=".repeat(4 - (s.length % 4));
    const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/") + pad);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  /** Derive an AES-GCM key from a password + salt via PBKDF2-SHA-256. */
  async function deriveWrapKey(sc, password, salt, iterations) {
    const base = await sc.importKey("raw", new TextEncoder().encode(password), "PBKDF2", false, ["deriveKey"]);
    return sc.deriveKey(
      { name: "PBKDF2", salt, iterations, hash: "SHA-256" },
      base,
      { name: "AES-GCM", length: 256 },
      false,
      ["encrypt", "decrypt"]
    );
  }

  /** Encrypt a private CryptoKey (+ its public SPKI DER) under `password`.
   *  Returns a self-describing JSON blob (no plaintext key material). */
  async function wrapIdentity(sc, rec, password) {
    if (!password) throw new Error("a password is required to export the identity");
    const jwk = await sc.exportKey("jwk", rec.priv);
    const salt = rand(16);
    const iv = rand(12);
    const key = await deriveWrapKey(sc, password, salt, KDF_ITERS);
    const plaintext = new TextEncoder().encode(JSON.stringify(jwk));
    const ct = await sc.encrypt({ name: "AES-GCM", iv }, key, plaintext);
    return {
      v: WRAP_VERSION,
      type: "freedback-identity",
      alg: "ES256",
      kdf: { name: "PBKDF2", hash: "SHA-256", iterations: KDF_ITERS, salt: b64url(salt) },
      enc: { name: "AES-GCM", iv: b64url(iv) },
      spki: b64url(rec.spki),
      ciphertext: b64url(ct),
    };
  }

  /** Decrypt a blob produced by `wrapIdentity` back into a key record
   *  (`{priv, spki}`). Throws on a wrong password (AES-GCM tag mismatch). */
  async function unwrapIdentity(sc, blob, password) {
    if (!blob || blob.type !== "freedback-identity") throw new Error("not a Freedback identity backup");
    if (blob.v !== WRAP_VERSION) throw new Error(`unsupported backup version ${blob.v}`);
    const salt = fromB64url(blob.kdf.salt);
    const iv = fromB64url(blob.enc.iv);
    const iterations = blob.kdf.iterations || KDF_ITERS;
    const key = await deriveWrapKey(sc, password, salt, iterations);
    let plaintext;
    try {
      plaintext = await sc.decrypt({ name: "AES-GCM", iv }, key, fromB64url(blob.ciphertext));
    } catch (_e) {
      throw new Error("wrong password or corrupt backup");
    }
    const jwk = JSON.parse(new TextDecoder().decode(plaintext));
    const priv = await sc.importKey("jwk", jwk, { name: "ECDSA", namedCurve: "P-256" }, true, ["sign"]);
    return { priv, spki: fromB64url(blob.spki).buffer };
  }

  /** Export the current IndexedDB identity as a password-encrypted backup blob. */
  async function exportIdentity(password) {
    const sc = subtle();
    if (!sc) throw new Error("WebCrypto unavailable (needs a secure context)");
    const db = await openDb();
    const rec = await idbGet(db, "kp");
    if (!rec) throw new Error("no identity to export yet");
    return wrapIdentity(sc, rec, password);
  }

  /** Import a password-encrypted backup, replacing the IndexedDB identity and
   *  restoring the same issuer id. Resets the cached identity promise. */
  async function importIdentity(blob, password) {
    const sc = subtle();
    if (!sc) throw new Error("WebCrypto unavailable (needs a secure context)");
    const rec = await unwrapIdentity(sc, blob, password);
    const db = await openDb();
    await idbPut(db, "kp", rec);
    identityPromise = null;
    ownIssuerPromise = null;
    return identityFromRecord(sc, rec);
  }

  // --- rotation (issue #27) ------------------------------------------------
  // Generate a fresh signing key and replace the active identity, while keeping
  // history attributable: the *new* key signs a small statement carrying the old
  // issuer id + old public PEM. Past self-signed annotations under the old key
  // stay valid — signatures are detached and verify independently of which key
  // is currently active. The returned link statement can be published as an
  // `oa:linking` annotation (or kept) to vouch that both keys are the same actor.
  async function buildRotationLink(sc, oldIdent, newRec) {
    const newIdent = await identityFromRecord(sc, newRec);
    const statement = {
      type: "freedback-key-rotation",
      newIssuer: newIdent.issuerId,
      oldIssuer: oldIdent.issuerId,
      oldKid: oldIdent.kid,
      created: new Date().toISOString(),
    };
    // The new key signs the canonical statement, vouching for the old issuer id.
    const bytes = new TextEncoder().encode(jcs(statement));
    const raw = await sc.sign({ name: "ECDSA", hash: "SHA-256" }, newRec.priv, bytes);
    return {
      statement,
      signature: { alg: "ES256", kid: newIdent.kid, sig: b64url(raw) },
    };
  }

  /** Rotate to a new signing key. Persists the new key as the active identity
   *  and returns `{ identity, previous, link }` where `link` is a statement
   *  signed by the new key that cross-references the old issuer id so history
   *  stays attributable. Past annotations under `previous` remain valid. */
  async function rotateIdentity() {
    const sc = subtle();
    if (!sc) throw new Error("WebCrypto unavailable (needs a secure context)");
    const db = await openDb();
    const oldRec = await idbGet(db, "kp");
    const oldIdent = oldRec ? await identityFromRecord(sc, oldRec) : null;
    const newRec = await generateKeyRecord(sc);
    const link = oldIdent ? await buildRotationLink(sc, oldIdent, newRec) : null;
    await idbPut(db, "kp", newRec);
    identityPromise = null;
    ownIssuerPromise = null;
    const newIdent = await identityFromRecord(sc, newRec);
    return { identity: newIdent, previous: oldIdent, link };
  }

  /** Build a self-signed annotation (detached ES256 over the JCS bytes).
   *  `created` is overridable so tests can pin a deterministic content;
   *  `rights` optionally licenses the feedback (ADR 0022). */
  async function buildSignedAnnotation(motivation, target, body, ident, created, rights) {
    const content = canonicalContent(motivation, target, body, ident.issuerId, created || new Date().toISOString(), rights);
    const bytes = new TextEncoder().encode(jcs(content));
    const raw = await subtle().sign({ name: "ECDSA", hash: "SHA-256" }, ident.priv, bytes);
    return { ...content, signature: { alg: "ES256", kid: ident.kid, sig: b64url(raw) } };
  }

  // --- right to erasure (ADR 0021) ------------------------------------------
  // An author deletes their own annotation with a *delete document* signed by
  // the SAME key that signed the annotation. The canonical shape mirrors
  // crates/protocol-lib/src/erasure.rs exactly:
  //   {"type":"Delete","annotation":"<dedup_id>","created":"<RFC3339>"}
  // and the ES256 signature is computed over the JCS bytes of the document
  // WITHOUT the `signature` field — same canonicalization, same scheme, same
  // kid (SPKI PEM) / sig (base64url, no pad) encoding as annotations.

  /** The unsigned delete document (the exact bytes-to-sign, once JCS'd). */
  function deleteDocument(dedupId, created) {
    return {
      type: "Delete",
      annotation: dedupId,
      created: created || new Date().toISOString(),
    };
  }

  /** Sign a delete document with the page identity (detached ES256 over the
   *  JCS bytes). `created` is overridable so tests can pin the content. */
  async function buildSignedDelete(dedupId, ident, created) {
    const doc = deleteDocument(dedupId, created);
    const bytes = new TextEncoder().encode(jcs(doc));
    const raw = await subtle().sign({ name: "ECDSA", hash: "SHA-256" }, ident.priv, bytes);
    return { ...doc, signature: { alg: "ES256", kid: ident.kid, sig: b64url(raw) } };
  }

  /** The dedup id is the basename of the server-minted annotation `id`
   *  (`{base}/annotations/{dedup}`). Never fetch that URL — just take the last
   *  path segment (an IRI with no `/` path, e.g. a urn, passes through whole). */
  function dedupFromId(id) {
    if (!id) return null;
    const path = String(id).split(/[?#]/)[0];
    return path.split("/").filter(Boolean).pop() || null;
  }

  /** A short, deterministic client-side fingerprint for an issuer id — a
   *  "same author?" glance, NOT a cryptographic key fingerprint (it's a
   *  32-bit FNV-1a hash of the id string, not the key material itself, so it
   *  works identically for self-signed `urn:freedback:key:…` issuers and
   *  OAuth `urn:freedback:oauth:…` ones without format-specific parsing). */
  function fingerprint(id) {
    if (!id) return "";
    let h = 0x811c9dc5;
    for (let i = 0; i < id.length; i++) {
      h ^= id.charCodeAt(i);
      h = Math.imul(h, 0x01000193);
    }
    return (h >>> 0).toString(16).padStart(8, "0");
  }

  // --- canonical body builders (match the Rust BodyWire serialization) -----
  function starBody(value) {
    return {
      type: ["freedback:StarRating", "schema:Rating"],
      "schema:ratingValue": Number(value),
      "schema:worstRating": 1,
      "schema:bestRating": 5,
    };
  }
  function thumbBody(up) {
    return {
      type: ["freedback:ThumbRating", "schema:Rating"],
      "schema:ratingValue": up ? 1 : 0,
      "schema:worstRating": 0,
      "schema:bestRating": 1,
    };
  }
  function scalarBody(value, worst, best) {
    return {
      type: ["freedback:ScalarRating", "schema:Rating"],
      "schema:ratingValue": Number(value),
      "schema:worstRating": Number(worst),
      "schema:bestRating": Number(best),
    };
  }
  function textBody(value, purpose) {
    return { type: "TextualBody", value, format: "text/plain", purpose };
  }

  function ratingValue(ann) {
    const bodies = Array.isArray(ann.body) ? ann.body : [ann.body];
    for (const b of bodies) {
      const t = b && b.type;
      const isRating = Array.isArray(t) ? t.some((x) => /Rating$/.test(x)) : /Rating$/.test(t || "");
      if (isRating && b["schema:ratingValue"] != null) return Number(b["schema:ratingValue"]);
    }
    return null;
  }

  function textBodies(ann, purpose) {
    const bodies = Array.isArray(ann.body) ? ann.body : [ann.body];
    return bodies
      .filter((b) => b && (b.type === "TextualBody" || b.type === "oa:TextualBody") && (!purpose || b.purpose === purpose))
      .map((b) => b.value)
      .filter(Boolean);
  }

  // The custom-element classes are only defined in a DOM environment, so this
  // file can also be `require`d in Node to unit-test the pure helpers above.
  const hasDom = typeof HTMLElement !== "undefined" && typeof customElements !== "undefined";
  if (hasDom) defineElements();

  function defineElements() {
  /** Shared base class: reads target/read/publish/token from data-attributes. */
  class FreedbackEl extends HTMLElement {
    get target() { return this.getAttribute("data-target"); }
    get readBase() { return this.getAttribute("data-read"); }
    get publishUrl() { return this.getAttribute("data-publish"); }
    get token() { return this.getAttribute("data-token") || undefined; }
    get signing() { return this.hasAttribute("data-sign") && !!subtle(); }
    /** Optional license IRI set as the annotation's `rights` (ADR 0022). */
    get license() { return this.getAttribute("data-license") || undefined; }

    connectedCallback() {
      this.render();
      this.refresh();
    }

    async refresh() {
      if (!this.readBase || !this.target) return;
      try {
        this.annotations = await fetchAnnotations(this.readBase, this.target);
        // Ownership detection (ADR 0021): with data-sign in a secure context,
        // items whose creator.id equals this browser identity's issuer id are
        // the visitor's OWN feedback and get a delete affordance.
        this.ownerId = this.signing ? await getOwnIssuerId() : null;
        this.renderAggregate();
      } catch (e) {
        this.setStatus(String(e.message || e));
      }
    }

    setStatus(msg) {
      const s = this.querySelector(".fb-status");
      if (s) s.textContent = msg;
    }

    async submit(motivation, body) {
      if (!this.publishUrl) return;
      try {
        let ann;
        let stored;
        if (this.signing) {
          // data-license rides in the signed content (6th arg; created defaults).
          ann = await buildSignedAnnotation(motivation, this.target, body, await getIdentity(), undefined, this.license);
          stored = await publish(this.publishUrl, ann);
        } else {
          ann = baseAnnotation(motivation, this.target, body, this.license);
          stored = await publish(this.publishUrl, ann, this.token);
        }
        // Remember the just-published record's dedup id (the basename of the
        // server-minted `id`) so rating widgets can offer an "undo" (erase)
        // affordance for this session's submission (ADR 0021).
        const dedup = stored ? dedupFromId(stored.id) : null;
        if (dedup) this.lastDedup = dedup;
        // Additive outcome event: a host app (e.g. React via a ref) can observe a
        // successful publish without scraping the widget's own DOM. `detail`
        // carries the stored annotation / server `response` plus the `annotation`
        // payload we sent. The `.fb-agg`/`.fb-status` DOM behavior is unchanged.
        this.emit("freedback:published", { response: stored, annotation: ann });
        await this.refresh();
      } catch (e) {
        this.setStatus(String(e.message || e));
        // Additive failure event mirroring `freedback:published`.
        this.emit("freedback:error", { error: e });
      }
    }

    /** Whether this widget can issue a delete at all: it needs somewhere to
     *  send it plus an identity — the signing key, or the same bearer that
     *  authorized the publish (the server checks ownership either way). */
    canErase() {
      return !!this.publishUrl && (this.signing || !!this.token);
    }

    /** Whether a fetched annotation is the visitor's own (signed) feedback. */
    isOwn(ann) {
      return !!(this.ownerId && ann && ann.creator && ann.creator.id === this.ownerId);
    }

    /** A discreet fingerprint badge for `ann`'s author (issue: authors
     *  deserve an identity IRI too — they at least have their public key,
     *  namespaced under `urn:freedback:key:…`). With `data-author-href` set,
     *  the badge links to a view of that identity used AS A TARGET: the same
     *  read/publish mechanism as any other feedback, just pointed at the
     *  author's own IRI instead of a product or page, so reviews "on" an
     *  author are ordinary annotations, nothing new to build server-side.
     *  Without the attribute the badge is inert text — still useful to spot
     *  a repeat author in a list without offering a link nothing backs. */
    authorBadge(ann) {
      const id = ann && ann.creator && ann.creator.id;
      if (!id) return null;
      const base = this.getAttribute("data-author-href");
      const el = document.createElement(base ? "a" : "span");
      el.className = "fb-fp";
      el.textContent = `#${fingerprint(id)}`;
      el.title = id;
      if (base) {
        const params = new URLSearchParams({ id });
        if (this.readBase) params.set("read", this.readBase);
        if (this.publishUrl) params.set("publish", this.publishUrl);
        const sep = base.includes("?") ? "&" : "?";
        el.href = `${base}${sep}${params.toString()}`;
      }
      return el;
    }

    /** Erase one annotation by dedup id — the author's right to erasure
     *  (ADR 0021). Builds the delete document, signs it with the SAME stored
     *  key that signs annotations (or carries the same bearer on the OAuth
     *  path), then `DELETE {data-publish}/{dedup}`. On 204 the aggregate/list
     *  refreshes and `freedback:deleted` fires; failures surface in
     *  `.fb-status` and fire `freedback:error`. */
    async erase(dedupId) {
      if (!this.publishUrl || !dedupId) return;
      try {
        const headers = { "content-type": "application/json" };
        let doc;
        if (this.signing) {
          doc = await buildSignedDelete(dedupId, await getIdentity());
        } else if (this.token) {
          doc = deleteDocument(dedupId);
          headers.authorization = `Bearer ${this.token}`;
        } else {
          throw new Error("delete needs data-sign or data-token");
        }
        const sep = this.publishUrl.endsWith("/") ? "" : "/";
        const resp = await fetch(`${this.publishUrl}${sep}${dedupId}`, {
          method: "DELETE",
          headers,
          body: JSON.stringify(doc),
        });
        if (resp.status !== 204 && !resp.ok) {
          throw new Error(`delete failed: ${resp.status} ${await resp.text()}`);
        }
        if (this.lastDedup === dedupId) this.lastDedup = null;
        this.emit("freedback:deleted", { annotation: dedupId, response: resp });
        await this.refresh();
      } catch (e) {
        this.setStatus(String(e.message || e));
        this.emit("freedback:error", { error: e });
      }
    }

    /** A small `×` control that erases `dedupId` when clicked. */
    deleteControl(dedupId) {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "fb-del";
      btn.setAttribute("aria-label", "Delete my feedback");
      btn.title = "Delete my feedback";
      btn.textContent = "×";
      btn.addEventListener("click", (e) => {
        e.preventDefault();
        this.erase(dedupId);
      });
      return btn;
    }

    /** Rating widgets have no per-item list, so after a successful publish in
     *  this session they show an "undo" delete control next to `.fb-agg` that
     *  erases the just-published annotation. Call at the end of
     *  `renderAggregate()`. */
    renderUndo() {
      const agg = this.querySelector(".fb-agg");
      if (!agg) return;
      const existing = this.querySelector(".fb-del");
      if (existing) existing.remove();
      if (!this.lastDedup || !this.canErase()) return;
      agg.after(this.deleteControl(this.lastDedup));
    }

    /** Dispatch a bubbling, composed CustomEvent on this host element so a host
     *  app (React ref, plain DOM listener, …) can observe publish outcomes. */
    emit(type, detail) {
      this.dispatchEvent(new CustomEvent(type, { detail, bubbles: true, composed: true }));
    }
  }

  // --- <freedback-stars> ---------------------------------------------------
  class FreedbackStars extends FreedbackEl {
    render() {
      this.innerHTML = `<div class="fb fb-stars">
        <span class="fb-row">${[1, 2, 3, 4, 5].map((n) => `<button data-v="${n}" aria-label="${n} stars">★</button>`).join("")}</span>
        <span class="fb-agg"></span><span class="fb-status"></span></div>`;
      if (this.publishUrl) {
        this.querySelectorAll("button").forEach((btn) =>
          btn.addEventListener("click", () => this.submit("assessing", starBody(btn.dataset.v)))
        );
      } else {
        this.querySelectorAll("button").forEach((b) => (b.disabled = true));
      }
    }
    renderAggregate() {
      const vals = this.annotations.map(ratingValue).filter((v) => v != null);
      const avg = vals.length ? (vals.reduce((a, b) => a + b, 0) / vals.length).toFixed(1) : "–";
      this.querySelector(".fb-agg").textContent = ` ${avg} (${vals.length})`;
      this.renderUndo();
    }
  }

  // --- <freedback-thumb> ---------------------------------------------------
  class FreedbackThumb extends FreedbackEl {
    render() {
      this.innerHTML = `<div class="fb fb-thumb">
        <button data-up="1">👍</button><button data-up="0">👎</button>
        <span class="fb-agg"></span><span class="fb-status"></span></div>`;
      if (this.publishUrl) {
        this.querySelectorAll("button").forEach((btn) =>
          btn.addEventListener("click", () => this.submit("assessing", thumbBody(btn.dataset.up === "1")))
        );
      } else {
        this.querySelectorAll("button").forEach((b) => (b.disabled = true));
      }
    }
    renderAggregate() {
      const vals = this.annotations.map(ratingValue).filter((v) => v != null);
      const up = vals.filter((v) => v >= 0.5).length;
      this.querySelector(".fb-agg").textContent = ` 👍 ${up} · 👎 ${vals.length - up}`;
      this.renderUndo();
    }
  }

  // --- <freedback-scalar> --------------------------------------------------
  // A continuous bounded rating. `data-worst`/`data-best`/`data-step` configure
  // the scale (default 0..1, step 0.1); the body carries the scale so SHACL can
  // validate against it (freedback:ScalarRating, ADR 0009).
  class FreedbackScalar extends FreedbackEl {
    get worst() { return Number(this.getAttribute("data-worst") ?? 0); }
    get best() { return Number(this.getAttribute("data-best") ?? 1); }
    get step() { return Number(this.getAttribute("data-step") ?? 0.1); }
    render() {
      const mid = (this.worst + this.best) / 2;
      this.innerHTML = `<div class="fb fb-scalar">
        <input class="fb-range" type="range" min="${this.worst}" max="${this.best}" step="${this.step}" value="${mid}" ${this.publishUrl ? "" : "disabled"} />
        <output class="fb-out">${mid}</output>
        ${this.publishUrl ? `<button class="fb-send">Rate</button>` : ""}
        <span class="fb-agg"></span><span class="fb-status"></span></div>`;
      const range = this.querySelector(".fb-range");
      const out = this.querySelector(".fb-out");
      range.addEventListener("input", () => (out.textContent = range.value));
      const send = this.querySelector(".fb-send");
      if (send) {
        send.addEventListener("click", () =>
          this.submit("assessing", scalarBody(range.value, this.worst, this.best))
        );
      }
    }
    renderAggregate() {
      const vals = this.annotations.map(ratingValue).filter((v) => v != null);
      const avg = vals.length ? (vals.reduce((a, b) => a + b, 0) / vals.length).toFixed(2) : "–";
      this.querySelector(".fb-agg").textContent = ` avg ${avg} (${vals.length})`;
      this.renderUndo();
    }
  }

  // --- <freedback-comment> -------------------------------------------------
  class FreedbackComment extends FreedbackEl {
    render() {
      this.innerHTML = `<div class="fb fb-comment">
        ${this.publishUrl ? `<form class="fb-row"><input class="fb-in" placeholder="Leave feedback…" /><button>Post</button></form>` : ""}
        <ul class="fb-list"></ul><span class="fb-status"></span></div>`;
      const form = this.querySelector("form");
      if (form) {
        form.addEventListener("submit", (e) => {
          e.preventDefault();
          const input = this.querySelector(".fb-in");
          const value = input.value.trim();
          if (!value) return;
          input.value = "";
          this.submit("commenting", textBody(value, "commenting"));
        });
      }
    }
    renderAggregate() {
      const ul = this.querySelector(".fb-list");
      ul.innerHTML = "";
      for (const ann of this.annotations) {
        const own = this.isOwn(ann) && this.canErase();
        for (const text of textBodies(ann, "commenting")) {
          const li = document.createElement("li");
          li.append(text);
          const badge = this.authorBadge(ann);
          if (badge) li.append(" ", badge);
          // The visitor's OWN comments get a delete control (ADR 0021).
          if (own) li.appendChild(this.deleteControl(dedupFromId(ann.id)));
          ul.appendChild(li);
        }
      }
    }
  }

  // --- <freedback-issue> ---------------------------------------------------
  // An issue / problem report (ADR 0023): free text published as an
  // oa:TextualBody under the STANDARD oa:editing motivation ("request a change
  // or edit to the Target resource") — zero new vocabulary. Full parity with
  // <freedback-comment>: signed + bearer publish, data-license, outcome events,
  // and the delete-own `×` (ADR 0021). Items get the `.fb-issue` styling (a ⚠
  // marker via CSS).
  class FreedbackIssue extends FreedbackEl {
    render() {
      this.innerHTML = `<div class="fb fb-issue">
        ${this.publishUrl ? `<form class="fb-row"><textarea class="fb-in" rows="2" placeholder="Describe the problem…"></textarea><button>Report</button></form>` : ""}
        <ul class="fb-list"></ul><span class="fb-status"></span></div>`;
      const form = this.querySelector("form");
      if (form) {
        form.addEventListener("submit", (e) => {
          e.preventDefault();
          const input = this.querySelector(".fb-in");
          const value = input.value.trim();
          if (!value) return;
          input.value = "";
          this.submit("editing", textBody(value, "editing"));
        });
      }
    }
    renderAggregate() {
      const ul = this.querySelector(".fb-list");
      ul.innerHTML = "";
      for (const ann of this.annotations) {
        const own = this.isOwn(ann) && this.canErase();
        for (const text of textBodies(ann, "editing")) {
          const li = document.createElement("li");
          li.className = "fb-issue-item";
          li.append(text);
          const badge = this.authorBadge(ann);
          if (badge) li.append(" ", badge);
          // The visitor's OWN issue reports get a delete control (ADR 0021).
          if (own) li.appendChild(this.deleteControl(dedupFromId(ann.id)));
          ul.appendChild(li);
        }
      }
    }
  }

  // --- <freedback-tag> -----------------------------------------------------
  // A single tag per submission (oa:tagging). Renders the distinct tags seen.
  class FreedbackTag extends FreedbackEl {
    render() {
      this.innerHTML = `<div class="fb fb-tag">
        ${this.publishUrl ? `<form class="fb-row"><input class="fb-in" placeholder="Add a tag…" /><button>Tag</button></form>` : ""}
        <span class="fb-tags"></span><span class="fb-status"></span></div>`;
      const form = this.querySelector("form");
      if (form) {
        form.addEventListener("submit", (e) => {
          e.preventDefault();
          const input = this.querySelector(".fb-in");
          const value = input.value.trim();
          if (!value) return;
          input.value = "";
          this.submit("tagging", textBody(value, "tagging"));
        });
      }
    }
    renderAggregate() {
      // tag -> { n, ownDedup }: chips aggregate counts across annotations, and
      // remember the visitor's OWN submission of that tag (if any) so the chip
      // can offer to delete just their annotation (ADR 0021).
      const counts = new Map();
      for (const ann of this.annotations) {
        for (const t of textBodies(ann, "tagging")) {
          const entry = counts.get(t) || { n: 0, ownDedup: null };
          entry.n += 1;
          if (!entry.ownDedup && this.isOwn(ann)) entry.ownDedup = dedupFromId(ann.id);
          counts.set(t, entry);
        }
      }
      const span = this.querySelector(".fb-tags");
      span.innerHTML = "";
      for (const [t, { n, ownDedup }] of counts) {
        const chip = document.createElement("span");
        chip.className = "fb-chip";
        chip.textContent = n > 1 ? `${t} ×${n}` : t;
        if (ownDedup && this.canErase()) chip.appendChild(this.deleteControl(ownDedup));
        span.appendChild(chip);
      }
    }
  }

  customElements.define("freedback-stars", FreedbackStars);
  customElements.define("freedback-thumb", FreedbackThumb);
  customElements.define("freedback-scalar", FreedbackScalar);
  customElements.define("freedback-comment", FreedbackComment);
  customElements.define("freedback-issue", FreedbackIssue);
  customElements.define("freedback-tag", FreedbackTag);
  } // defineElements

  // Expose builders for testing in non-DOM environments (Node).
  if (typeof module !== "undefined" && module.exports) {
    module.exports = {
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
      // right to erasure (ADR 0021)
      deleteDocument,
      buildSignedDelete,
      dedupFromId,
      fingerprint,
      getIdentity,
      // identity management (issue #27). The *Record helpers take an explicit
      // SubtleCrypto so they unit-test in Node without IndexedDB; the IndexedDB-
      // backed export/import/rotate wrappers run in the browser.
      generateKeyRecord,
      identityFromRecord,
      wrapIdentity,
      unwrapIdentity,
      buildRotationLink,
      exportIdentity,
      importIdentity,
      rotateIdentity,
    };
  }

  // Expose the browser-facing identity-management API on a global so pages
  // (e.g. demo.html) can offer export/import/rotate without a build step.
  if (typeof window !== "undefined") {
    window.Freedback = Object.assign(window.Freedback || {}, {
      getIdentity,
      exportIdentity,
      importIdentity,
      rotateIdentity,
    });
  }
})();

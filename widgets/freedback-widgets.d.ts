// Type definitions for @freedback/widgets.
//
// Importing "@freedback/widgets" (the ESM entry) for its side effect registers
// the six custom elements; these types make the tags type-check in JSX (React
// 19 and React <= 18) and in framework-neutral DOM code, with ZERO consumer
// setup, plus types for the helper/identity exports and the outcome events.

// --- shared attribute surface -------------------------------------------------

/** The `data-*` attributes every Freedback widget accepts. */
export interface FreedbackDataAttributes {
  /** The URI the feedback is *about* (your page/product/item). Required. */
  "data-target": string;
  /**
   * Read endpoint that returns aggregates — a collection server's `/index` or a
   * feedback server's `/annotations/`. Omit for a write-only widget.
   */
  "data-read"?: string;
  /** A feedback server's `/annotations/`. Omit for a read-only widget. */
  "data-publish"?: string;
  /**
   * Presence enables self-signed publishing (a per-browser P-256 key in
   * IndexedDB, via WebCrypto). Write it as `data-sign=""` in JSX.
   */
  "data-sign"?: "" | boolean;
  /**
   * An OAuth bearer for the app-managed identity instead of `data-sign`.
   * `data-sign` wins if both are set.
   */
  "data-token"?: string;
  /**
   * Optional license IRI (e.g. `https://creativecommons.org/licenses/by/4.0/`)
   * set as the published annotation's W3C `rights` property, on both the
   * signed and bearer paths (data licensing, ADR 0022). Omit to fall under
   * the server's default license (`/.well-known/freedback`).
   */
  "data-license"?: string;
  /** `<freedback-scalar>` scale: worst value (default 0). */
  "data-worst"?: string | number;
  /** `<freedback-scalar>` scale: best value (default 1). */
  "data-best"?: string | number;
  /** `<freedback-scalar>` scale: step (default 0.1). */
  "data-step"?: string | number;
  /**
   * `<freedback-comment>` / `<freedback-issue>` only: a base URL for a view
   * of an item's author, used AS A FEEDBACK TARGET (an author's identity is
   * an IRI too — at minimum their public key). When set, each item's short
   * fingerprint badge becomes a link to `${base}?id=<creator.id>&read=...
   * &publish=...`; without it the badge is inert text. No server support is
   * needed either way — an author's IRI is just an ordinary target.
   */
  "data-author-href"?: string;
}

// --- outcome events (CustomEvent detail shapes) -------------------------------

/** `detail` of the `freedback:published` CustomEvent. */
export interface FreedbackPublishedDetail {
  /** The stored annotation / server response (parsed JSON). */
  response: unknown;
  /** The annotation payload that was sent. */
  annotation: Annotation;
}

/** `detail` of the `freedback:error` CustomEvent. */
export interface FreedbackErrorDetail {
  /** The error thrown while publishing or deleting. */
  error: Error;
}

/** `detail` of the `freedback:deleted` CustomEvent (right to erasure, ADR 0021). */
export interface FreedbackDeletedDetail {
  /** The dedup id of the annotation that was erased. */
  annotation: string;
  /** The raw `fetch` Response of the `DELETE` (204 No Content on success). */
  response: Response;
}

/** The `freedback:published` CustomEvent dispatched on a widget after success. */
export type FreedbackPublishedEvent = CustomEvent<FreedbackPublishedDetail>;
/** The `freedback:error` CustomEvent dispatched on a widget after a failure. */
export type FreedbackErrorEvent = CustomEvent<FreedbackErrorDetail>;
/** The `freedback:deleted` CustomEvent dispatched after a successful erasure. */
export type FreedbackDeletedEvent = CustomEvent<FreedbackDeletedDetail>;

/** The custom event map a Freedback widget element fires. */
export interface FreedbackEventMap {
  "freedback:published": FreedbackPublishedEvent;
  "freedback:error": FreedbackErrorEvent;
  "freedback:deleted": FreedbackDeletedEvent;
}

// --- element interfaces -------------------------------------------------------

/** Base element type for every Freedback widget custom element. */
export interface FreedbackElement extends HTMLElement {
  addEventListener<K extends keyof FreedbackEventMap>(
    type: K,
    listener: (this: FreedbackElement, ev: FreedbackEventMap[K]) => unknown,
    options?: boolean | AddEventListenerOptions
  ): void;
  addEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: boolean | AddEventListenerOptions
  ): void;
  removeEventListener<K extends keyof FreedbackEventMap>(
    type: K,
    listener: (this: FreedbackElement, ev: FreedbackEventMap[K]) => unknown,
    options?: boolean | EventListenerOptions
  ): void;
  removeEventListener(
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: boolean | EventListenerOptions
  ): void;
}

export interface FreedbackStarsElement extends FreedbackElement {}
export interface FreedbackThumbElement extends FreedbackElement {}
export interface FreedbackScalarElement extends FreedbackElement {}
export interface FreedbackCommentElement extends FreedbackElement {}
export interface FreedbackIssueElement extends FreedbackElement {}
export interface FreedbackTagElement extends FreedbackElement {}

// --- annotation / body shapes (mirror the canonical wire form) ----------------

export interface RatingBody {
  type: string[];
  "schema:ratingValue": number;
  "schema:worstRating": number;
  "schema:bestRating": number;
}

export interface TextualBody {
  type: "TextualBody";
  value: string;
  format: "text/plain";
  purpose: string;
}

export interface Annotation {
  "@context": string[];
  type: "Annotation";
  motivation: string;
  created?: string;
  creator?: { id: string };
  target: string;
  body: Array<RatingBody | TextualBody>;
  conformsTo: string;
  /** License IRI the author distributes this feedback under (ADR 0022). */
  rights?: string;
  signature?: { alg: string; kid: string; sig: string };
}

/** A right-to-erasure delete document (ADR 0021) — mirrors the Rust
 *  `DeleteRequest` in `crates/protocol-lib/src/erasure.rs`. The ES256
 *  signature is computed over the JCS bytes WITHOUT the `signature` field. */
export interface DeleteDocument {
  type: "Delete";
  /** The dedup id (content address) of the annotation to erase. */
  annotation: string;
  /** When the delete was issued (RFC 3339). */
  created: string;
  signature?: { alg: string; kid: string; sig: string };
}

/** A self-signed P-256 identity derived from the per-browser key. */
export interface FreedbackIdentity {
  /** The signing private key (a WebCrypto `CryptoKey`). */
  priv: CryptoKey;
  /** The portable, federating issuer id (`urn:freedback:key:<sha256-hex>`). */
  issuerId: string;
  /** The SPKI PEM public key used as the signature `kid`. */
  kid: string;
}

/** A stored key record: extractable private key + public SPKI DER bytes. */
export interface FreedbackKeyRecord {
  priv: CryptoKey;
  spki: ArrayBuffer;
}

/** A password-encrypted identity backup blob (no plaintext key material). */
export interface FreedbackIdentityBackup {
  v: number;
  type: "freedback-identity";
  alg: "ES256";
  kdf: { name: "PBKDF2"; hash: "SHA-256"; iterations: number; salt: string };
  enc: { name: "AES-GCM"; iv: string };
  spki: string;
  ciphertext: string;
}

/** The link a key rotation emits, signed by the new key over the old issuer id. */
export interface FreedbackRotationLink {
  statement: {
    type: "freedback-key-rotation";
    newIssuer: string;
    oldIssuer: string;
    oldKid: string;
    created: string;
  };
  signature: { alg: "ES256"; kid: string; sig: string };
}

// --- helper / identity API (mirrors the IIFE `module.exports`) ----------------

export function baseAnnotation(
  motivation: string,
  target: string,
  body: RatingBody | TextualBody,
  rights?: string
): Annotation;
export function canonicalContent(
  motivation: string,
  target: string,
  body: RatingBody | TextualBody,
  creatorId: string,
  created: string,
  rights?: string
): Annotation;
export function jcs(value: unknown): string;
export function ratingValue(annotation: { body: unknown }): number | null;
export function textBodies(annotation: { body: unknown }, purpose?: string): string[];
export function readUrl(base: string, target: string): string;
export function starBody(value: number | string): RatingBody;
export function thumbBody(up: boolean): RatingBody;
export function scalarBody(
  value: number | string,
  worst: number | string,
  best: number | string
): RatingBody;
export function textBody(value: string, purpose: string): TextualBody;
export function buildSignedAnnotation(
  motivation: string,
  target: string,
  body: RatingBody | TextualBody,
  identity: FreedbackIdentity,
  created?: string,
  rights?: string
): Promise<Annotation>;
export function deleteDocument(dedupId: string, created?: string): DeleteDocument;
export function buildSignedDelete(
  dedupId: string,
  identity: FreedbackIdentity,
  created?: string
): Promise<DeleteDocument>;
export function dedupFromId(id: string | null | undefined): string | null;
/**
 * A short, deterministic, non-cryptographic hash of an issuer id (a 32-bit
 * FNV-1a over the id string, as 8 lowercase hex chars) — a "same author?"
 * glance, NOT the key's real fingerprint. Works uniformly for self-signed
 * (`urn:freedback:key:…`) and OAuth (`urn:freedback:oauth:…`) issuer ids.
 */
export function fingerprint(id: string | null | undefined): string;
export function getIdentity(): Promise<FreedbackIdentity>;
export function generateKeyRecord(subtle: SubtleCrypto): Promise<FreedbackKeyRecord>;
export function identityFromRecord(
  subtle: SubtleCrypto,
  record: FreedbackKeyRecord
): Promise<FreedbackIdentity>;
export function wrapIdentity(
  subtle: SubtleCrypto,
  record: FreedbackKeyRecord,
  password: string
): Promise<FreedbackIdentityBackup>;
export function unwrapIdentity(
  subtle: SubtleCrypto,
  blob: FreedbackIdentityBackup,
  password: string
): Promise<FreedbackKeyRecord>;
export function buildRotationLink(
  subtle: SubtleCrypto,
  oldIdentity: FreedbackIdentity,
  newRecord: FreedbackKeyRecord
): Promise<FreedbackRotationLink>;
export function exportIdentity(password: string): Promise<FreedbackIdentityBackup>;
export function importIdentity(
  blob: FreedbackIdentityBackup,
  password: string
): Promise<FreedbackIdentity>;
export function rotateIdentity(): Promise<{
  identity: FreedbackIdentity;
  previous: FreedbackIdentity | null;
  link: FreedbackRotationLink | null;
}>;

declare const _default: {
  baseAnnotation: typeof baseAnnotation;
  canonicalContent: typeof canonicalContent;
  jcs: typeof jcs;
  ratingValue: typeof ratingValue;
  textBodies: typeof textBodies;
  readUrl: typeof readUrl;
  starBody: typeof starBody;
  thumbBody: typeof thumbBody;
  scalarBody: typeof scalarBody;
  textBody: typeof textBody;
  buildSignedAnnotation: typeof buildSignedAnnotation;
  deleteDocument: typeof deleteDocument;
  buildSignedDelete: typeof buildSignedDelete;
  dedupFromId: typeof dedupFromId;
  fingerprint: typeof fingerprint;
  getIdentity: typeof getIdentity;
  generateKeyRecord: typeof generateKeyRecord;
  identityFromRecord: typeof identityFromRecord;
  wrapIdentity: typeof wrapIdentity;
  unwrapIdentity: typeof unwrapIdentity;
  buildRotationLink: typeof buildRotationLink;
  exportIdentity: typeof exportIdentity;
  importIdentity: typeof importIdentity;
  rotateIdentity: typeof rotateIdentity;
};
export default _default;

// --- framework-neutral DOM registration --------------------------------------

declare global {
  interface HTMLElementTagNameMap {
    "freedback-stars": FreedbackStarsElement;
    "freedback-thumb": FreedbackThumbElement;
    "freedback-scalar": FreedbackScalarElement;
    "freedback-comment": FreedbackCommentElement;
    "freedback-issue": FreedbackIssueElement;
    "freedback-tag": FreedbackTagElement;
  }

  /** The browser-facing identity-management API exposed on `window.Freedback`. */
  interface Window {
    Freedback?: {
      getIdentity: typeof getIdentity;
      exportIdentity: typeof exportIdentity;
      importIdentity: typeof importIdentity;
      rotateIdentity: typeof rotateIdentity;
    };
  }
}

// --- JSX augmentation ---------------------------------------------------------
//
// React forwards `data-*`/`aria-*` to the DOM, so the props surface is the
// `data-*` attrs plus standard HTML attributes. We augment BOTH the React 19
// module-scoped `React.JSX.IntrinsicElements` and, guarded behind it being
// declared, the React <= 18 global `JSX.IntrinsicElements`.

type FreedbackBaseProps = {
  /** `freedback:published` fires after a successful publish (custom event). */
  onPublished?: (event: FreedbackPublishedEvent) => void;
  /** `freedback:error` fires after a failed publish or delete (custom event). */
  onError?: (event: FreedbackErrorEvent) => void;
  /** `freedback:deleted` fires after a successful erasure (custom event). */
  onDeleted?: (event: FreedbackDeletedEvent) => void;
};

// React 19: JSX lives on the `react` module's `JSX` namespace.
declare module "react" {
  namespace JSX {
    interface IntrinsicElements {
      "freedback-stars": React.DetailedHTMLProps<React.HTMLAttributes<HTMLElement>, HTMLElement> &
        FreedbackDataAttributes &
        FreedbackBaseProps;
      "freedback-thumb": React.DetailedHTMLProps<React.HTMLAttributes<HTMLElement>, HTMLElement> &
        FreedbackDataAttributes &
        FreedbackBaseProps;
      "freedback-scalar": React.DetailedHTMLProps<React.HTMLAttributes<HTMLElement>, HTMLElement> &
        FreedbackDataAttributes &
        FreedbackBaseProps;
      "freedback-comment": React.DetailedHTMLProps<React.HTMLAttributes<HTMLElement>, HTMLElement> &
        FreedbackDataAttributes &
        FreedbackBaseProps;
      "freedback-issue": React.DetailedHTMLProps<React.HTMLAttributes<HTMLElement>, HTMLElement> &
        FreedbackDataAttributes &
        FreedbackBaseProps;
      "freedback-tag": React.DetailedHTMLProps<React.HTMLAttributes<HTMLElement>, HTMLElement> &
        FreedbackDataAttributes &
        FreedbackBaseProps;
    }
  }
}

// React <= 18: JSX lives on the GLOBAL `JSX` namespace. This block augments it
// when present; if the consumer never references a global `JSX` (e.g. a pure
// React 19 setup), the declaration is simply unused and harmless.
declare global {
  namespace JSX {
    interface IntrinsicElements {
      "freedback-stars": GlobalFreedbackProps;
      "freedback-thumb": GlobalFreedbackProps;
      "freedback-scalar": GlobalFreedbackProps;
      "freedback-comment": GlobalFreedbackProps;
      "freedback-issue": GlobalFreedbackProps;
      "freedback-tag": GlobalFreedbackProps;
    }
  }
}

// The global-JSX prop shape. Kept structurally identical to the React-19 form
// but without depending on the `react` module's types resolving in this scope.
type GlobalFreedbackProps = FreedbackDataAttributes &
  FreedbackBaseProps & {
    [key: `data-${string}`]: string | number | boolean | undefined;
    children?: unknown;
    id?: string;
    className?: string;
    class?: string;
    slot?: string;
    style?: unknown;
  };

export {};

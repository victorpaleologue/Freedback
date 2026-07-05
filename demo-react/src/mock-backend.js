// In-browser mock backend for the Freedback widget showcase.
//
// There is no live Freedback server behind the GitHub Pages deploy, so this
// module monkey-patches window.fetch to emulate the two endpoints the shipped
// widgets call (widgets/freedback-widgets.js), backed by an in-memory Map keyed
// by `target`. It fakes success for BOTH auth paths (data-sign and data-token)
// — it never verifies signatures or bearer tokens, it just stores and succeeds.
//
// The contract this MUST match (read widgets/freedback-widgets.js):
//
//   READ  — fetchAnnotations(base, target):
//     GET `${base}?target=${encodeURIComponent(target)}` with
//     `accept: application/ld+json`. The widget does:
//        if (Array.isArray(doc)) return doc;
//        if (Array.isArray(doc.items)) return doc.items;
//        return [];
//     We mirror the collection server's `/index` shape and return a W3C
//     AnnotationPage: `{ "@context", type: "AnnotationPage", items: [...] }`.
//
//   PUBLISH — publish(url, annotation, token):
//     POST to data-publish with `content-type: application/ld+json` and an
//     optional `authorization: Bearer <token>`. The widget requires resp.ok
//     and then `resp.json()`. We accept the body, assign a fake id, store it
//     under its `target`, and return 201 + the stored annotation (the shape the
//     feedback server returns: the annotation echoed back with a server `id`).
//
//   DELETE — erase(dedupId) (right to erasure, ADR 0021):
//     DELETE to `${data-publish}/<id>` (pathname `/annotations/<id>`) with a
//     JSON delete document body. We ignore the body (fake success — no
//     signature verification), remove the matching stored annotation → 204;
//     unknown id → 404.
//
// Aggregates render purely client-side in the widget's renderAggregate from the
// stored bodies (ratingValue / textBodies), so storing the posted annotation
// verbatim is enough to make stars-avg / 👍-count / scalar-avg / comment-list /
// tag-chips update live on the next refresh() the widget runs after publish().

const ANNO_CTX = ["http://www.w3.org/ns/anno.jsonld", "https://freedback.net/ns/context.jsonld"];

// target (string) -> array of stored annotation objects.
const store = new Map();

let counter = 0;
function nextId(target) {
  counter += 1;
  // A stable, fake server-assigned id. Real servers mint an absolute URL under
  // their base; the widgets never parse it, so any unique IRI-ish string works.
  return `urn:freedback:mock:${counter}`;
}

function add(target, annotation) {
  const list = store.get(target) || [];
  // Echo the posted annotation with a server-assigned id (what a real feedback
  // server returns). We deep-clone so later mutation of the caller's object
  // cannot retroactively change stored state (determinism).
  const stored = { ...structuredCloneSafe(annotation), id: nextId(target) };
  list.push(stored);
  store.set(target, list);
  return stored;
}

function structuredCloneSafe(v) {
  try {
    return structuredClone(v);
  } catch {
    return JSON.parse(JSON.stringify(v));
  }
}

// ---- body builders (match the Rust BodyWire / widget body serialization) ----
function starBody(value) {
  return {
    type: ["freedback:StarRating", "schema:Rating"],
    "schema:ratingValue": value,
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
    "schema:ratingValue": value,
    "schema:worstRating": worst,
    "schema:bestRating": best,
  };
}
function textBody(value, purpose) {
  return { type: "TextualBody", value, format: "text/plain", purpose };
}

function seedAnnotation(target, motivation, body, created) {
  add(target, {
    "@context": ANNO_CTX,
    type: "Annotation",
    motivation,
    creator: { id: "urn:freedback:mock:seed" },
    created,
    target,
    body: [body],
    conformsTo: "https://freedback.net/profile/1",
  });
}

// Seed a little deterministic data per showcase target so the widgets render
// non-empty aggregates on first load (no flake — fixed values + timestamps).
// Each demo target is dedicated to a single widget kind (see App.jsx) to avoid
// cross-counting, exactly as e2e.html isolates kinds with `#fragment` targets.
export function seedDemoData(targets) {
  // Stars: a few ratings → a visible average like "4.0 (3)".
  for (const v of [5, 4, 3]) seedAnnotation(targets.stars, "assessing", starBody(v), "2026-01-01T00:00:00.000Z");
  // Thumb: two up, one down → "👍 2 · 👎 1".
  seedAnnotation(targets.thumb, "assessing", thumbBody(true), "2026-01-01T00:00:01.000Z");
  seedAnnotation(targets.thumb, "assessing", thumbBody(true), "2026-01-01T00:00:02.000Z");
  seedAnnotation(targets.thumb, "assessing", thumbBody(false), "2026-01-01T00:00:03.000Z");
  // Scalar (0..10): two ratings → "avg 6.50 (2)".
  seedAnnotation(targets.scalar, "assessing", scalarBody(8, 0, 10), "2026-01-01T00:00:04.000Z");
  seedAnnotation(targets.scalar, "assessing", scalarBody(5, 0, 10), "2026-01-01T00:00:05.000Z");
  // Comments: two existing comments.
  seedAnnotation(targets.comment, "commenting", textBody("Clean, federated, and no lock-in. Love it.", "commenting"), "2026-01-01T00:00:06.000Z");
  seedAnnotation(targets.comment, "commenting", textBody("The self-signed identity path is brilliant.", "commenting"), "2026-01-01T00:00:07.000Z");
  // Tags: a couple of tags, one repeated → chips "rust ×2", "open-protocol".
  seedAnnotation(targets.tag, "tagging", textBody("rust", "tagging"), "2026-01-01T00:00:08.000Z");
  seedAnnotation(targets.tag, "tagging", textBody("rust", "tagging"), "2026-01-01T00:00:09.000Z");
  seedAnnotation(targets.tag, "tagging", textBody("open-protocol", "tagging"), "2026-01-01T00:00:10.000Z");

  // The signed / bearer showcase targets (each a single stars widget) get one
  // seed rating so they render an aggregate before the visitor interacts.
  if (targets.signStars) seedAnnotation(targets.signStars, "assessing", starBody(5), "2026-01-01T00:00:11.000Z");
  if (targets.tokenStars) seedAnnotation(targets.tokenStars, "assessing", starBody(4), "2026-01-01T00:00:12.000Z");
}

// ---- the fetch interceptor --------------------------------------------------

// We recognise our mock endpoints by path so the showcase can use realistic,
// human-readable URLs (e.g. https://feedback.demo.freedback.net/annotations/
// and https://collection.demo.freedback.net/index). Anything we don't recognise
// falls through to the real fetch (so Vite HMR, source maps, etc. still work).
const PUBLISH_PATH = "/annotations/";
const READ_PATH = "/index";

let realFetch = null;

function isPublish(url) {
  try {
    return new URL(url, location.href).pathname === PUBLISH_PATH;
  } catch {
    return false;
  }
}
function isRead(url) {
  try {
    return new URL(url, location.href).pathname === READ_PATH;
  } catch {
    return false;
  }
}
function isDeleteItem(url) {
  try {
    const p = new URL(url, location.href).pathname;
    return p.startsWith(PUBLISH_PATH) && p.length > PUBLISH_PATH.length;
  } catch {
    return false;
  }
}

function jsonResponse(status, body) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/ld+json" },
  });
}

async function handlePublish(input, init) {
  // Accept BOTH auth paths without verifying. data-sign produces a body with a
  // detached `signature`; data-token sends `authorization: Bearer ...`. Either
  // way we just store and succeed (this is a demo, fake-success backend).
  let annotation;
  try {
    annotation = JSON.parse((init && init.body) || "{}");
  } catch {
    return jsonResponse(400, { error: "invalid JSON" });
  }
  const target = annotation && annotation.target;
  if (!target || typeof target !== "string") {
    return jsonResponse(422, { error: "missing target" });
  }
  const stored = add(target, annotation);
  // Real feedback server answers 201 Created with the stored annotation
  // (echoed body + server id). The widget only needs resp.ok + resp.json().
  return jsonResponse(201, stored);
}

// DELETE /annotations/<id> — the widgets' "delete my feedback" affordance
// (ADR 0021). The <id> is the basename of the stored annotation's `id` (real
// servers mint `{base}/annotations/{dedup}`; this mock mints urns, whose
// basename is the whole urn). Fake-success like the rest of this mock: no
// signature/bearer verification — remove from the store → 204; unknown → 404.
function handleDelete(input) {
  const url = new URL(input, location.href);
  const suffix = decodeURIComponent(url.pathname.slice(PUBLISH_PATH.length));
  if (!suffix) return jsonResponse(400, { error: "missing annotation id" });
  for (const [target, list] of store) {
    const idx = list.findIndex((a) => {
      const id = String(a.id || "");
      return id === suffix || id.split("/").filter(Boolean).pop() === suffix;
    });
    if (idx !== -1) {
      list.splice(idx, 1);
      store.set(target, list);
      return new Response(null, { status: 204 });
    }
  }
  return jsonResponse(404, { error: "annotation not found" });
}

function handleRead(input) {
  const url = new URL(input, location.href);
  const target = url.searchParams.get("target") || "";
  const items = store.get(target) || [];
  // The collection /index shape the widgets read: a W3C AnnotationPage whose
  // `items` array the widget extracts (doc.items). Return a deep copy so the
  // widget can't mutate our store.
  return jsonResponse(200, {
    "@context": "http://www.w3.org/ns/anno.jsonld",
    type: "AnnotationPage",
    id: url.toString(),
    items: items.map(structuredCloneSafe),
  });
}

let installed = false;

/** Install the fetch interceptor. Idempotent. */
export function installMockBackend() {
  if (installed) return;
  installed = true;
  realFetch = window.fetch ? window.fetch.bind(window) : null;

  window.fetch = async function mockFetch(input, init) {
    // `input` may be a Request, URL, or string. Normalise to a URL string and
    // an init we can read the method/body from.
    let url;
    let method = (init && init.method) || "GET";
    let effectiveInit = init;
    if (input instanceof Request) {
      url = input.url;
      method = input.method || method;
      // The widgets always pass a string body via init for POSTs, so reading
      // init.body is sufficient; if a Request carried the body we read it too.
      if (!effectiveInit) effectiveInit = {};
      if (effectiveInit.body == null && (method || "").toUpperCase() === "POST") {
        try {
          effectiveInit = { ...effectiveInit, body: await input.clone().text() };
        } catch {
          /* fall through */
        }
      }
    } else {
      url = String(input);
    }

    const upper = (method || "GET").toUpperCase();
    if (upper === "POST" && isPublish(url)) return handlePublish(url, effectiveInit);
    if (upper === "GET" && isRead(url)) return handleRead(url);
    if (upper === "DELETE" && isDeleteItem(url)) return handleDelete(url);

    // Not a mock endpoint — defer to the real fetch.
    if (realFetch) return realFetch(input, init);
    throw new Error(`mock backend: no real fetch available for ${url}`);
  };
}

// Exposed for debugging / potential reset between demos.
export function _resetStore() {
  store.clear();
  counter = 0;
}

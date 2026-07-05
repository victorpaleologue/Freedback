// Annotate-anything demo — in-browser mock backend (classic script, NOT a module).
//
// There is no live Freedback server behind the GitHub Pages deploy, so this
// file monkey-patches window.fetch to emulate the two endpoints the shipped
// widgets (/widgets/freedback-widgets.js) call, backed by an in-memory Map keyed
// by `target`. It never verifies signatures or bearer tokens — it just stores
// and succeeds (demo, fake-success backend).
//
// It is a vanilla IIFE mirror of demo-react/src/mock-backend.js (no imports /
// exports). Load it FIRST, before the widgets, so the interceptor is installed
// before any widget calls fetch().
//
// Contract (matches widgets/freedback-widgets.js):
//
//   READ  — GET `${base}?target=<uri>` with `accept: application/ld+json`.
//     The widget does: Array.isArray(doc) ? doc : Array.isArray(doc.items) ?
//     doc.items : []. We return a W3C AnnotationPage: `{ "@context", type:
//     "AnnotationPage", id, items: [...] }`. 200, application/ld+json.
//
//   PUBLISH — POST to data-publish (pathname `/annotations/`) with
//     `content-type: application/ld+json`. The body is JSON of the annotation
//     (it has a string `target`). We store it under its target and return 201
//     with the stored annotation + a fake `id`, application/ld+json.
//
// Endpoints are recognised by pathname so the cosmetic hostnames in the page
// snippets (feedback.demo.freedback.net / collection.demo.freedback.net) work.
// Anything else falls through to the real fetch.
(() => {
  "use strict";

  // Idempotent: never install twice.
  if (window.__freedbackAnywhereMockInstalled) return;
  window.__freedbackAnywhereMockInstalled = true;

  const ANNO_CTX = ["http://www.w3.org/ns/anno.jsonld", "https://freedback.net/ns/context.jsonld"];

  // target (string) -> array of stored annotation objects.
  const store = new Map();

  let counter = 0;
  function nextId() {
    counter += 1;
    // A stable, fake server-assigned id. Real servers mint an absolute URL under
    // their base; the widgets never parse it, so any unique IRI-ish string works.
    return `urn:freedback:mock:${counter}`;
  }

  function cloneSafe(v) {
    try {
      return structuredClone(v);
    } catch {
      return JSON.parse(JSON.stringify(v));
    }
  }

  function add(target, annotation) {
    const list = store.get(target) || [];
    const stored = { ...cloneSafe(annotation), id: nextId() };
    list.push(stored);
    store.set(target, list);
    return stored;
  }

  // ---- body builders (match widgets/freedback-widgets.js + mock-backend.js) ----
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

  // ---- targets: the external resources we attach feedback to ----------------
  // Each widget kind gets its OWN target string. Where two kinds share a real
  // resource we suffix a `#fragment` so rating aggregates never cross-count.
  const TARGETS = {
    // GDPR (Regulation (EU) 2016/679) via its ELI URI.
    gdprStars: "http://data.europa.eu/eli/reg/2016/679/oj",
    gdprComment: "http://data.europa.eu/eli/reg/2016/679/oj#comments",
    // Wikipedia: Web annotation.
    wikiStars: "https://en.wikipedia.org/wiki/Web_annotation",
    wikiTag: "https://en.wikipedia.org/wiki/Web_annotation#tags",
    // W3C Web Annotation Data Model spec.
    w3cThumb: "https://www.w3.org/TR/annotation-model/",
    w3cComment: "https://www.w3.org/TR/annotation-model/#comments",
  };
  window.__freedbackAnywhereTargets = TARGETS;

  // Seed deterministic data per target (fixed values + fixed ISO timestamps) so
  // aggregates render non-empty on first load.
  function seed() {
    // GDPR — stars "how clear is this regulation?" → avg 3.0 (3).
    for (const v of [4, 3, 2]) seedAnnotation(TARGETS.gdprStars, "assessing", starBody(v), "2026-01-01T00:00:00.000Z");
    // GDPR — comments.
    seedAnnotation(TARGETS.gdprComment, "commenting", textBody("Article 17 (right to erasure) is clearer than I expected.", "commenting"), "2026-01-01T00:00:01.000Z");
    seedAnnotation(TARGETS.gdprComment, "commenting", textBody("Recitals help, but the legalese is dense.", "commenting"), "2026-01-01T00:00:02.000Z");

    // Wikipedia — stars → avg 4.5 (2).
    for (const v of [5, 4]) seedAnnotation(TARGETS.wikiStars, "assessing", starBody(v), "2026-01-01T00:00:03.000Z");
    // Wikipedia — tags (one repeated → "w3c ×2", "standards").
    seedAnnotation(TARGETS.wikiTag, "tagging", textBody("w3c", "tagging"), "2026-01-01T00:00:04.000Z");
    seedAnnotation(TARGETS.wikiTag, "tagging", textBody("w3c", "tagging"), "2026-01-01T00:00:05.000Z");
    seedAnnotation(TARGETS.wikiTag, "tagging", textBody("standards", "tagging"), "2026-01-01T00:00:06.000Z");

    // W3C spec — thumbs (two up, one down → "👍 2 · 👎 1").
    seedAnnotation(TARGETS.w3cThumb, "assessing", thumbBody(true), "2026-01-01T00:00:07.000Z");
    seedAnnotation(TARGETS.w3cThumb, "assessing", thumbBody(true), "2026-01-01T00:00:08.000Z");
    seedAnnotation(TARGETS.w3cThumb, "assessing", thumbBody(false), "2026-01-01T00:00:09.000Z");
    // W3C spec — comment.
    seedAnnotation(TARGETS.w3cComment, "commenting", textBody("The selector model is the part everyone reimplements.", "commenting"), "2026-01-01T00:00:10.000Z");
  }
  seed();

  // ---- the fetch interceptor ------------------------------------------------
  const PUBLISH_PATH = "/annotations/";
  const READ_PATH = "/index";

  function pathOf(url) {
    try {
      return new URL(url, location.href).pathname;
    } catch {
      return null;
    }
  }

  function jsonResponse(status, body) {
    return new Response(JSON.stringify(body), {
      status,
      headers: { "content-type": "application/ld+json" },
    });
  }

  function handlePublish(init) {
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

  function handleRead(url) {
    const u = new URL(url, location.href);
    const target = u.searchParams.get("target") || "";
    const items = store.get(target) || [];
    return jsonResponse(200, {
      "@context": "http://www.w3.org/ns/anno.jsonld",
      type: "AnnotationPage",
      id: u.toString(),
      items: items.map(cloneSafe),
    });
  }

  // DELETE /annotations/<id> — the widgets' "delete my feedback" affordance
  // (ADR 0021). The <id> is the basename of the stored annotation's `id` (this
  // mock mints urns, whose basename is the whole urn). Fake-success like the
  // rest of this mock: no signature/bearer verification — remove from the
  // store → 204; unknown → 404.
  function handleDelete(url) {
    const u = new URL(url, location.href);
    const suffix = decodeURIComponent(u.pathname.slice(PUBLISH_PATH.length));
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

  const realFetch = window.fetch ? window.fetch.bind(window) : null;

  window.fetch = async function mockFetch(input, init) {
    // `input` may be a Request, URL, or string. Normalise to a URL string and an
    // init we can read the method/body from.
    let url;
    let method = (init && init.method) || "GET";
    let effectiveInit = init;
    if (typeof Request !== "undefined" && input instanceof Request) {
      url = input.url;
      method = input.method || method;
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
    const path = pathOf(url);
    if (upper === "POST" && path === PUBLISH_PATH) return handlePublish(effectiveInit);
    if (upper === "GET" && path === READ_PATH) return handleRead(url);
    if (upper === "DELETE" && path && path.startsWith(PUBLISH_PATH) && path.length > PUBLISH_PATH.length) {
      return handleDelete(url);
    }

    // Not a mock endpoint — defer to the real fetch.
    if (realFetch) return realFetch(input, init);
    throw new Error(`mock backend: no real fetch available for ${url}`);
  };
})();

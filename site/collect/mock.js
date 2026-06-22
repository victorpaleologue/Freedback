// In-browser mock backend for the Freedback "data collection" demo page
// (site/collect/index.html).
//
// There is no live Freedback server behind the GitHub Pages deploy, so this
// classic (non-module) script monkey-patches window.fetch to emulate the two
// endpoints the shipped widgets call (widgets/freedback-widgets.js), backed by
// an in-memory Map keyed by `target`. It fakes success for BOTH auth paths
// (data-sign and data-token) — it never verifies signatures or bearer tokens,
// it just stores and succeeds.
//
// It mirrors demo-react/src/mock-backend.js exactly:
//
//   READ  — fetchAnnotations(base, target):
//     GET `${base}?target=${encodeURIComponent(target)}` with
//     `accept: application/ld+json`. We return a W3C AnnotationPage
//     `{ "@context", type: "AnnotationPage", id, items: [...] }`; the widget
//     extracts `doc.items`.
//
//   PUBLISH — publish(url, annotation, token):
//     POST to data-publish (pathname `/annotations/`) with body = annotation
//     JSON (string `target`). We accept the body, assign a fake id, store it
//     under its `target`, and return 201 + the stored annotation.
//
// In addition to the React mock, this build exposes `window.FreedbackMock` with
// `dump()` (a flat, deep-cloned array of every stored annotation across all
// targets) and `reset()` so the page's CSV / JSON-LD export buttons can read
// the store directly.
(function () {
  "use strict";

  // Idempotent install guard.
  if (window.FreedbackMock && window.FreedbackMock.__installed) return;

  var ANNO_CTX = ["http://www.w3.org/ns/anno.jsonld", "https://freedback.net/ns/context.jsonld"];

  // target (string) -> array of stored annotation objects.
  var store = new Map();

  var counter = 0;
  function nextId() {
    counter += 1;
    // A stable, fake server-assigned id. Real servers mint an absolute URL under
    // their base; the widgets never parse it, so any unique IRI-ish string works.
    return "urn:freedback:mock:" + counter;
  }

  function cloneSafe(v) {
    try {
      return structuredClone(v);
    } catch (e) {
      return JSON.parse(JSON.stringify(v));
    }
  }

  function add(target, annotation) {
    var list = store.get(target) || [];
    // Echo the posted annotation with a server-assigned id (what a real feedback
    // server returns). Deep-clone so later mutation of the caller's object cannot
    // retroactively change stored state (determinism).
    var stored = Object.assign(cloneSafe(annotation), { id: nextId() });
    list.push(stored);
    store.set(target, list);
    return stored;
  }

  // ---- body builders (match the widget body serialization) ------------------
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
    return { type: "TextualBody", value: value, format: "text/plain", purpose: purpose };
  }

  function seed(target, motivation, body, created) {
    add(target, {
      "@context": ANNO_CTX,
      type: "Annotation",
      motivation: motivation,
      creator: { id: "urn:freedback:mock:seed" },
      created: created,
      target: target,
      body: [body],
      conformsTo: "https://freedback.net/profile/1",
    });
  }

  // The five question targets of the talk-feedback form (one widget kind each,
  // so no cross-counting). Kept in sync with site/collect/index.html.
  var BASE = "https://talks.example/2026/rdf-in-the-browser";
  var T = {
    overall: BASE + "#q-overall", // <freedback-scalar> 0..10
    clarity: BASE + "#q-clarity", // <freedback-stars>
    recommend: BASE + "#q-recommend", // <freedback-thumb>
    comments: BASE + "#q-comments", // <freedback-comment>
    tags: BASE + "#q-tags", // <freedback-tag>
  };

  // Seed deterministic data per question target (fixed values + fixed ISO
  // timestamps) so the form shows non-empty aggregates and the export has rows
  // immediately.
  function seedAll() {
    // Overall (scalar 0..10): three ratings.
    seed(T.overall, "assessing", scalarBody(9, 0, 10), "2026-06-01T09:00:00.000Z");
    seed(T.overall, "assessing", scalarBody(7, 0, 10), "2026-06-01T09:01:00.000Z");
    seed(T.overall, "assessing", scalarBody(8, 0, 10), "2026-06-01T09:02:00.000Z");
    // Clarity (stars 1..5): three ratings.
    seed(T.clarity, "assessing", starBody(5), "2026-06-01T09:03:00.000Z");
    seed(T.clarity, "assessing", starBody(4), "2026-06-01T09:04:00.000Z");
    seed(T.clarity, "assessing", starBody(4), "2026-06-01T09:05:00.000Z");
    // Recommend (thumb 0/1): two up, one down.
    seed(T.recommend, "assessing", thumbBody(true), "2026-06-01T09:06:00.000Z");
    seed(T.recommend, "assessing", thumbBody(true), "2026-06-01T09:07:00.000Z");
    seed(T.recommend, "assessing", thumbBody(false), "2026-06-01T09:08:00.000Z");
    // Comments: two existing comments.
    seed(T.comments, "commenting", textBody("More live coding, fewer slides.", "commenting"), "2026-06-01T09:09:00.000Z");
    seed(T.comments, "commenting", textBody("Loved the JSON-LD walkthrough — maybe slow down on SHACL.", "commenting"), "2026-06-01T09:10:00.000Z");
    // Tags: a couple of tags, one repeated.
    seed(T.tags, "tagging", textBody("sparql", "tagging"), "2026-06-01T09:11:00.000Z");
    seed(T.tags, "tagging", textBody("sparql", "tagging"), "2026-06-01T09:12:00.000Z");
    seed(T.tags, "tagging", textBody("federation", "tagging"), "2026-06-01T09:13:00.000Z");
  }

  // ---- the fetch interceptor ------------------------------------------------
  var PUBLISH_PATH = "/annotations/";
  var READ_PATH = "/index";

  function isPublish(url) {
    try {
      return new URL(url, location.href).pathname === PUBLISH_PATH;
    } catch (e) {
      return false;
    }
  }
  function isRead(url) {
    try {
      return new URL(url, location.href).pathname === READ_PATH;
    } catch (e) {
      return false;
    }
  }

  function jsonResponse(status, body) {
    return new Response(JSON.stringify(body), {
      status: status,
      headers: { "content-type": "application/ld+json" },
    });
  }

  function handlePublish(init) {
    var annotation;
    try {
      annotation = JSON.parse((init && init.body) || "{}");
    } catch (e) {
      return jsonResponse(400, { error: "invalid JSON" });
    }
    var target = annotation && annotation.target;
    if (!target || typeof target !== "string") {
      return jsonResponse(422, { error: "missing target" });
    }
    var stored = add(target, annotation);
    // Real feedback server answers 201 Created with the stored annotation.
    return jsonResponse(201, stored);
  }

  function handleRead(input) {
    var url = new URL(input, location.href);
    var target = url.searchParams.get("target") || "";
    var items = store.get(target) || [];
    // The collection /index shape the widgets read: a W3C AnnotationPage whose
    // `items` array the widget extracts. Return a deep copy.
    return jsonResponse(200, {
      "@context": "http://www.w3.org/ns/anno.jsonld",
      type: "AnnotationPage",
      id: url.toString(),
      items: items.map(cloneSafe),
    });
  }

  var realFetch = window.fetch ? window.fetch.bind(window) : null;

  window.fetch = async function mockFetch(input, init) {
    var url;
    var method = (init && init.method) || "GET";
    var effectiveInit = init;
    if (typeof Request !== "undefined" && input instanceof Request) {
      url = input.url;
      method = input.method || method;
      if (!effectiveInit) effectiveInit = {};
      if (effectiveInit.body == null && (method || "").toUpperCase() === "POST") {
        try {
          effectiveInit = Object.assign({}, effectiveInit, { body: await input.clone().text() });
        } catch (e) {
          /* fall through */
        }
      }
    } else {
      url = String(input);
    }

    var upper = (method || "GET").toUpperCase();
    if (upper === "POST" && isPublish(url)) return handlePublish(effectiveInit);
    if (upper === "GET" && isRead(url)) return handleRead(url);

    if (realFetch) return realFetch(input, init);
    throw new Error("mock backend: no real fetch available for " + url);
  };

  seedAll();

  // Exposed so the export buttons (and debugging) can read/reset the store.
  window.FreedbackMock = {
    __installed: true,
    targets: T,
    // A flat, deep-cloned array of every stored annotation across all targets.
    dump: function () {
      var out = [];
      store.forEach(function (list) {
        list.forEach(function (a) {
          out.push(cloneSafe(a));
        });
      });
      return out;
    },
    reset: function () {
      store.clear();
      counter = 0;
      seedAll();
    },
  };
})();

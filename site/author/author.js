// Author view — an author's identity IS a feedback target.
//
// Freedback issuer ids are already IRIs: a self-signed identity's id is
// `urn:freedback:key:<sha256-of-its-public-key>` (an OAuth identity's is
// `urn:freedback:oauth:<app>:<user>`) — no new server work is needed to make
// an author reviewable, since the protocol already lets ANY IRI be a target.
// This page just points the shipped widgets at that IRI instead of a URL or
// a barcode.
//
// Intentionally text-only (the comment widget, not a star rating): rating
// PEOPLE with a number is a different, more fraught thing than rating a
// product or a page, and this project would rather not build that if it can
// help it. A free-text note stays closer to "here's some feedback for you"
// than to a leaderboard.
(function () {
  "use strict";

  var LIVE_ANNOTATIONS = "https://freedback-demo.fly.dev/annotations/";
  var LICENSE = "https://creativecommons.org/licenses/by/4.0/";

  function init() {
    var params = new URLSearchParams(location.search);
    var id = params.get("id");
    var readBase = params.get("read") || LIVE_ANNOTATIONS;
    var publishBase = params.get("publish") || LIVE_ANNOTATIONS;

    var idEl = document.getElementById("author-id");
    var missing = document.getElementById("author-missing");
    var card = document.getElementById("author-card");

    if (!id) {
      missing.hidden = false;
      card.hidden = true;
      return;
    }

    idEl.textContent = id;
    missing.hidden = true;
    card.hidden = false;

    var host = document.getElementById("author-widgets");
    host.replaceChildren();
    var comment = document.createElement("freedback-comment");
    comment.setAttribute("data-target", id);
    comment.setAttribute("data-read", readBase);
    comment.setAttribute("data-publish", publishBase);
    comment.setAttribute("data-sign", "");
    comment.setAttribute("data-license", LICENSE);
    // Reviews left here are feedback too — their own authors get the same
    // fingerprint-and-link treatment, recursively.
    comment.setAttribute("data-author-href", "/author/");
    host.appendChild(comment);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();

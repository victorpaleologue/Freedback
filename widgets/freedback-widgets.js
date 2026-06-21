/*
 * Freedback drop-in widgets (component 3) — vanilla Web Components, no build step.
 *
 *   <freedback-stars
 *      data-target="https://example.com/item/1"
 *      data-read="http://localhost:8100/index"      <!-- collection /index or feedback /annotations/ -->
 *      data-publish="http://localhost:8080/annotations/"
 *      data-token="optional-oauth-bearer"></freedback-stars>
 *
 * A widget with only `data-read` renders a read-only aggregate (no auth).
 * `data-publish` enables submitting; auth is an OAuth bearer (`data-token`) in
 * this minimal build. WebCrypto P-256 signing (so browser keys never leave the
 * page) is the documented next step — see docs/adr/0003.
 *
 * The native wire format is a W3C Web Annotation; these widgets emit exactly the
 * same shape `freedback-protocol` does in Rust.
 */
(() => {
  "use strict";

  const ANNO_CTX = ["http://www.w3.org/ns/anno.jsonld", "https://freedback.org/ns/context.jsonld"];
  const PROFILE = "https://freedback.org/profile/1";

  /** Build the read URL with a target query param. */
  function readUrl(base, target) {
    const sep = base.includes("?") ? "&" : "?";
    return `${base}${sep}target=${encodeURIComponent(target)}`;
  }

  /** Pull annotations from a `data-read` endpoint (AnnotationPage or array). */
  async function fetchAnnotations(base, target) {
    const resp = await fetch(readUrl(base, target), { headers: { accept: "application/ld+json" } });
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

  function baseAnnotation(motivation, target, body) {
    return {
      "@context": ANNO_CTX,
      type: "Annotation",
      motivation,
      created: new Date().toISOString(),
      target,
      body: [body],
      conformsTo: PROFILE,
    };
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

    connectedCallback() {
      this.render();
      this.refresh();
    }

    async refresh() {
      if (!this.readBase || !this.target) return;
      try {
        this.annotations = await fetchAnnotations(this.readBase, this.target);
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
        const ann = baseAnnotation(motivation, this.target, body);
        await publish(this.publishUrl, ann, this.token);
        await this.refresh();
      } catch (e) {
        this.setStatus(String(e.message || e));
      }
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
          btn.addEventListener("click", () =>
            this.submit("assessing", {
              type: ["freedback:StarRating", "schema:Rating"],
              "schema:ratingValue": Number(btn.dataset.v),
              "schema:worstRating": 1,
              "schema:bestRating": 5,
            })
          )
        );
      } else {
        this.querySelectorAll("button").forEach((b) => (b.disabled = true));
      }
    }
    renderAggregate() {
      const vals = this.annotations.map(ratingValue).filter((v) => v != null);
      const avg = vals.length ? (vals.reduce((a, b) => a + b, 0) / vals.length).toFixed(1) : "–";
      this.querySelector(".fb-agg").textContent = ` ${avg} (${vals.length})`;
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
          btn.addEventListener("click", () =>
            this.submit("assessing", {
              type: ["freedback:ThumbRating", "schema:Rating"],
              "schema:ratingValue": Number(btn.dataset.up),
              "schema:worstRating": 0,
              "schema:bestRating": 1,
            })
          )
        );
      } else {
        this.querySelectorAll("button").forEach((b) => (b.disabled = true));
      }
    }
    renderAggregate() {
      const vals = this.annotations.map(ratingValue).filter((v) => v != null);
      const up = vals.filter((v) => v >= 0.5).length;
      this.querySelector(".fb-agg").textContent = ` 👍 ${up} · 👎 ${vals.length - up}`;
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
          this.submit("commenting", { type: "TextualBody", value, format: "text/plain", purpose: "commenting" });
        });
      }
    }
    renderAggregate() {
      const comments = this.annotations.flatMap((a) => textBodies(a, "commenting"));
      this.querySelector(".fb-list").innerHTML = comments.map((c) => `<li></li>`).join("");
      this.querySelectorAll(".fb-list li").forEach((li, i) => (li.textContent = comments[i]));
    }
  }

  customElements.define("freedback-stars", FreedbackStars);
  customElements.define("freedback-thumb", FreedbackThumb);
  customElements.define("freedback-comment", FreedbackComment);
  } // defineElements

  // Expose builders for testing in non-DOM environments (Node).
  if (typeof module !== "undefined" && module.exports) {
    module.exports = { baseAnnotation, ratingValue, textBodies, readUrl };
  }
})();

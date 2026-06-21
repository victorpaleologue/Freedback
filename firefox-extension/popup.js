/* Freedback Firefox popup: list feedback for the active tab's URL. */
(() => {
  "use strict";
  const api = typeof browser !== "undefined" ? browser : chrome;
  const out = document.getElementById("out");
  const urlEl = document.getElementById("url");
  const serverEl = document.getElementById("server");

  let pageUrl = "";

  async function activeTabUrl() {
    const tabs = await api.tabs.query({ active: true, currentWindow: true });
    return (tabs && tabs[0] && tabs[0].url) || "";
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
  function text(ann) {
    const bodies = Array.isArray(ann.body) ? ann.body : [ann.body];
    const tb = bodies.find((b) => b && (b.type === "TextualBody" || b.type === "oa:TextualBody"));
    return tb ? tb.value : null;
  }

  function render(items) {
    if (!items.length) {
      out.className = "muted";
      out.textContent = "No feedback found for this page.";
      return;
    }
    out.className = "";
    out.textContent = "";
    for (const ann of items) {
      const div = document.createElement("div");
      div.className = "item";
      const rv = ratingValue(ann);
      const tx = text(ann);
      if (rv != null) {
        const span = document.createElement("span");
        span.className = "star";
        span.textContent = `★ ${rv}`;
        div.appendChild(span);
      } else if (tx) {
        div.textContent = tx;
      } else {
        div.className = "item muted";
        div.textContent = "(typed body)";
      }
      out.appendChild(div);
    }
  }

  async function load() {
    const server = (serverEl.value || "").trim().replace(/\/$/, "");
    if (!server || !pageUrl) return;
    api.storage.local.set({ server });
    out.className = "muted";
    out.textContent = "Loading…";
    try {
      const url = `${server}/index?target=${encodeURIComponent(pageUrl)}`;
      const resp = await fetch(url, { headers: { accept: "application/ld+json" } });
      if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
      const doc = await resp.json();
      render(Array.isArray(doc) ? doc : doc.items || []);
    } catch (e) {
      out.className = "muted";
      out.textContent = `Error: ${e.message || e}`;
    }
  }

  async function init() {
    pageUrl = await activeTabUrl();
    urlEl.textContent = pageUrl || "(no active tab)";
    const saved = await api.storage.local.get("server");
    if (saved && saved.server) serverEl.value = saved.server;
    document.getElementById("go").addEventListener("click", load);
  }

  init();
})();

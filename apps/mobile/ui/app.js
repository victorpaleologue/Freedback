// Freedback mobile UI — vanilla JS over the Tauri command layer.
// `withGlobalTauri` exposes window.__TAURI__; every command is a thin wrapper
// over freedback-app-core (see ../src-tauri/src/lib.rs).
"use strict";

const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
const listen = (event, handler) => window.__TAURI__.event.listen(event, handler);

const $ = (id) => document.getElementById(id);

// ---------------------------------------------------------------- routing

const VIEWS = ["home", "feedback", "author", "journal", "key", "settings"];
const TITLES = {
  home: "Freedback",
  feedback: "Feedback",
  author: "Author",
  journal: "My feedback",
  key: "My key",
  settings: "Settings",
};

let currentView = "home";
let currentTarget = null;
let currentAuthor = null;
// Where to return when leaving the author view — it's only ever reached by
// tapping a fingerprint badge from within another view.
let returnTo = null;

function show(view) {
  currentView = view;
  for (const v of VIEWS) $(`view-${v}`).classList.toggle("hidden", v !== view);
  $("title").textContent = TITLES[view];
  $("nav-back").classList.toggle("hidden", view === "home");
}

// ------------------------------------------------------------- utilities

function flash(id, message) {
  const el = $(id);
  el.textContent = message;
  el.classList.remove("hidden");
  if (id.endsWith("-ok") || el.classList.contains("ok")) {
    setTimeout(() => el.classList.add("hidden"), 6000);
  }
}

function clearNotices(...ids) {
  for (const id of ids) $(id).classList.add("hidden");
}

function itemLi(text, meta) {
  const li = document.createElement("li");
  li.textContent = text;
  if (meta) {
    const span = document.createElement("span");
    span.className = "meta";
    span.textContent = meta;
    li.appendChild(span);
  }
  return li;
}

function shortIssuer(creator) {
  if (!creator) return "anonymous";
  const tail = creator.split(":").pop() || creator;
  return `key …${tail.slice(-8)}`;
}

// Deterministic, non-cryptographic 32-bit FNV-1a over the issuer id — kept
// byte-identical to widgets/freedback-widgets.js so the same author gets the
// same badge everywhere (web widgets, author view, mobile app).
function fingerprint(id) {
  if (!id) return "";
  let h = 0x811c9dc5;
  for (let i = 0; i < id.length; i++) {
    h ^= id.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

// A discreet, tappable "#a1b2c3d4" badge next to a piece of feedback — a
// "same author?" glance that opens a view of that identity used as an
// ordinary feedback target (an author's own IRI is a target like any other).
function authorBadge(creator) {
  if (!creator) return null;
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "fb-fp";
  btn.textContent = `#${fingerprint(creator)}`;
  btn.title = creator;
  btn.addEventListener("click", () => openAuthor(creator));
  return btn;
}

function commentLi(item) {
  const li = document.createElement("li");
  li.textContent = item.text;
  const meta = document.createElement("span");
  meta.className = "meta";
  meta.append(authorBadge(item.creator) || "anonymous");
  if (item.created) meta.append(" · " + item.created);
  li.appendChild(meta);
  return li;
}

// ----------------------------------------------------------------- home

async function checkBackupNudge() {
  try {
    const shouldNudge = await invoke("should_nudge_key_backup");
    $("backup-nudge").classList.toggle("hidden", !shouldNudge);
  } catch (e) {
    console.error("backup nudge check", e);
  }
}

async function lookUp(raw) {
  clearNotices("resolve-error");
  try {
    const resolved = await invoke("resolve_input", { input: raw });
    await openFeedback(resolved.uri);
  } catch (e) {
    flash("resolve-error", String(e));
  }
}

// tauri-plugin-barcode-scanner is mobile-only — its commands only exist when
// `scanning_supported` (a Rust `cfg!(mobile)` check) says so; on desktop this
// leaves the button disabled rather than calling a command that can't exist.
const SCAN_FORMATS = ["QR_CODE", "EAN_13", "EAN_8", "UPC_A", "UPC_E", "CODE_128"];

async function initScan() {
  try {
    const supported = await invoke("scanning_supported");
    if (!supported) return;
    const btn = $("scan-btn");
    btn.disabled = false;
    btn.title = "";
    btn.textContent = "Scan";
  } catch (e) {
    console.error("scan support check", e);
  }
}

async function scanBarcode() {
  clearNotices("resolve-error");
  try {
    let perm = await invoke("plugin:barcode-scanner|check_permissions");
    if (perm.camera !== "granted") {
      perm = await invoke("plugin:barcode-scanner|request_permissions");
    }
    if (perm.camera !== "granted") {
      return flash("resolve-error", "Camera permission is needed to scan.");
    }
    const result = await invoke("plugin:barcode-scanner|scan", { formats: SCAN_FORMATS });
    if (result && result.content) await lookUp(result.content);
  } catch (e) {
    flash("resolve-error", String(e));
  }
}

async function initKeyScan() {
  try {
    const supported = await invoke("scanning_supported");
    if (!supported) return;
    const btn = $("key-scan-btn");
    btn.disabled = false;
    btn.title = "";
    btn.textContent = "Scan QR to import";
  } catch (e) {
    console.error("scan support check", e);
  }
}

// Scanning only fills the textarea — importing still goes through the same
// explicit two-step confirm as pasting one in by hand.
async function scanKeyQr() {
  clearNotices("key-error", "key-ok");
  try {
    let perm = await invoke("plugin:barcode-scanner|check_permissions");
    if (perm.camera !== "granted") {
      perm = await invoke("plugin:barcode-scanner|request_permissions");
    }
    if (perm.camera !== "granted") {
      return flash("key-error", "Camera permission is needed to scan.");
    }
    const result = await invoke("plugin:barcode-scanner|scan", { formats: ["QR_CODE"] });
    if (result && result.content) $("key-import").value = result.content;
  } catch (e) {
    flash("key-error", String(e));
  }
}

// -------------------------------------------------------------- feedback

async function openFeedback(target) {
  currentTarget = target;
  $("fb-target").textContent = target;
  show("feedback");
  await refreshFeedback();
}

async function refreshFeedback() {
  clearNotices("fb-error");
  try {
    const view = await invoke("get_feedback", { target: currentTarget });
    $("fb-star-avg").textContent =
      view.star_avg == null ? "–" : `★ ${view.star_avg.toFixed(1)}`;
    $("fb-star-count").textContent =
      view.star_count > 0 ? `(${view.star_count} rating${view.star_count > 1 ? "s" : ""})` : "(no ratings yet)";
    $("fb-thumbs-up").textContent = `👍 ${view.thumbs_up}`;
    $("fb-thumbs-down").textContent = `👎 ${view.thumbs_down}`;

    const issues = $("fb-issues");
    issues.replaceChildren();
    for (const i of view.issues) issues.appendChild(commentLi(i));
    if (view.issues.length === 0) issues.appendChild(itemLi("No issues reported."));

    const comments = $("fb-comments");
    comments.replaceChildren();
    for (const c of view.comments) comments.appendChild(commentLi(c));
    if (view.comments.length === 0) comments.appendChild(itemLi("No comments yet."));

    const tags = $("fb-tags");
    tags.replaceChildren();
    for (const t of view.tags) tags.appendChild(itemLi(t.text));
    if (view.tags.length === 0) tags.appendChild(itemLi("No tags yet."));
  } catch (e) {
    flash("fb-error", String(e));
  }
}

async function publishContribution(contribution) {
  clearNotices("fb-error", "fb-ok");
  try {
    await invoke("publish", { target: currentTarget, contribution, license: null });
    await refreshFeedback();
    flash("fb-ok", "Published ✓");
  } catch (e) {
    flash("fb-error", String(e));
  }
}

// ----------------------------------------------------------------- author

// An author's identity IRI is a feedback target like any other — no server
// work needed to make an author reviewable. Intentionally text-only (a
// comment, not a star rating): rating PEOPLE with a number is a different,
// more fraught thing than rating a product, and this app would rather not
// build that if it can help it.
async function openAuthor(id) {
  if (!id) return;
  returnTo = { view: currentView, target: currentTarget };
  currentAuthor = id;
  $("author-id").textContent = id;
  show("author");
  await refreshAuthor();
}

async function refreshAuthor() {
  clearNotices("author-error");
  try {
    const view = await invoke("get_feedback", { target: currentAuthor });
    const comments = $("author-comments");
    comments.replaceChildren();
    for (const c of view.comments) comments.appendChild(commentLi(c));
    if (view.comments.length === 0) comments.appendChild(itemLi("No notes yet."));
  } catch (e) {
    flash("author-error", String(e));
  }
}

async function publishAuthorNote(text) {
  clearNotices("author-error", "author-ok");
  try {
    await invoke("publish", { target: currentAuthor, contribution: { kind: "comment", text }, license: null });
    await refreshAuthor();
    flash("author-ok", "Published ✓");
  } catch (e) {
    flash("author-error", String(e));
  }
}

// --------------------------------------------------------------- journal

async function refreshJournal() {
  clearNotices("journal-error");
  try {
    const entries = await invoke("my_feedback");
    const list = $("journal-list");
    list.replaceChildren();
    $("journal-empty").classList.toggle("hidden", entries.length > 0);

    for (const entry of entries) {
      list.appendChild(journalLi(entry));
    }
  } catch (e) {
    flash("journal-error", String(e));
  }
}

// One journal row. No native dialogs (prompt/confirm are untestable and feel
// alien): Update reveals an inline editor for the same kind; Delete asks for
// a second, explicit click.
function journalLi(entry) {
  const li = document.createElement("li");
  li.dataset.dedupId = entry.dedup_id;
  li.dataset.kind = entry.kind;

  const head = document.createElement("div");
  head.className = "journal-head";
  head.textContent = `${entry.kind}: ${entry.summary}`;
  li.appendChild(head);

  const meta = document.createElement("span");
  meta.className = "meta";
  meta.textContent = `${entry.target} · ${entry.created}`;
  li.appendChild(meta);

  const state = entry.status.state;
  const status = document.createElement("span");
  status.className = `journal-status ${state}`;
  status.textContent = state === "active" ? "live" : state;
  li.appendChild(status);

  if (state !== "active") return li;

  const actions = document.createElement("div");
  actions.className = "journal-actions";
  li.appendChild(actions);

  const open = document.createElement("button");
  open.className = "secondary journal-open";
  open.textContent = "Open";
  open.addEventListener("click", () => openFeedback(entry.target));
  actions.appendChild(open);

  const update = document.createElement("button");
  update.className = "secondary journal-update";
  update.textContent = "Update";
  update.addEventListener("click", () => {
    editor.classList.toggle("hidden");
  });
  actions.appendChild(update);

  const del = document.createElement("button");
  del.className = "danger journal-delete";
  del.textContent = "Delete";
  del.addEventListener("click", async () => {
    if (del.dataset.armed !== "true") {
      del.dataset.armed = "true";
      del.textContent = "Really delete?";
      return;
    }
    try {
      await invoke("erase_entry", { dedupId: entry.dedup_id });
      await refreshJournal();
    } catch (e) {
      flash("journal-error", String(e));
    }
  });
  actions.appendChild(del);

  // The inline editor, hidden until Update is pressed.
  const editor = document.createElement("div");
  editor.className = "row journal-editor hidden";
  li.appendChild(editor);

  let getContribution;
  if (entry.kind === "stars") {
    const select = starSelect();
    editor.appendChild(select);
    getContribution = () => {
      const value = Number(select.value);
      return value >= 1 && value <= 5 ? { kind: "stars", value } : null;
    };
  } else if (entry.kind === "thumb") {
    const select = document.createElement("select");
    for (const opt of ["👍 up", "👎 down"]) {
      const o = document.createElement("option");
      o.value = opt.endsWith("up") ? "up" : "down";
      o.textContent = opt;
      select.appendChild(o);
    }
    editor.appendChild(select);
    getContribution = () => ({ kind: "thumb", up: select.value === "up" });
  } else {
    const input = document.createElement("input");
    input.type = "text";
    input.placeholder = `New ${entry.kind}…`;
    editor.appendChild(input);
    getContribution = () => {
      const text = input.value.trim();
      return text ? { kind: entry.kind, text } : null;
    };
  }

  const save = document.createElement("button");
  save.className = "primary journal-save";
  save.textContent = "Save";
  save.addEventListener("click", async () => {
    const contribution = getContribution();
    if (!contribution) return flash("journal-error", "Enter a new value first.");
    try {
      await invoke("update_entry", { dedupId: entry.dedup_id, contribution, license: null });
      await refreshJournal();
    } catch (e) {
      flash("journal-error", String(e));
    }
  });
  editor.appendChild(save);

  return li;
}

function starSelect() {
  const select = document.createElement("select");
  for (let i = 1; i <= 5; i++) {
    const o = document.createElement("option");
    o.value = String(i);
    o.textContent = `${i} ★`;
    select.appendChild(o);
  }
  select.value = "5";
  return select;
}

// ------------------------------------------------------------------ key

async function exportKey() {
  clearNotices("key-error", "key-ok");
  try {
    $("key-export").value = await invoke("export_identity");
    const qr = $("key-qr");
    qr.innerHTML = await invoke("export_identity_qr");
    qr.classList.remove("hidden");
  } catch (e) {
    flash("key-error", String(e));
  }
}

async function copyKey() {
  const pem = $("key-export").value;
  if (!pem) return flash("key-error", "Export first.");
  try {
    await navigator.clipboard.writeText(pem);
    flash("key-ok", "Copied — keep it safe!");
  } catch (e) {
    flash("key-error", `Copy failed: ${e}`);
  }
}

async function importKey() {
  clearNotices("key-error", "key-ok");
  const pem = $("key-import").value.trim();
  if (!pem) return flash("key-error", "Paste a PKCS#8 PEM private key first.");
  // Two-step confirm, inline (no native dialog).
  const btn = $("key-import-btn");
  if (btn.dataset.armed !== "true") {
    btn.dataset.armed = "true";
    btn.textContent = "Really replace this device's key?";
    return;
  }
  btn.dataset.armed = "";
  btn.textContent = "Import";
  try {
    const issuer = await invoke("import_identity", { pem });
    $("key-import").value = "";
    flash("key-ok", `Imported ✓ — now publishing as ${shortIssuer(issuer)}`);
  } catch (e) {
    flash("key-error", String(e));
  }
}

// -------------------------------------------------------------- settings

async function loadSettings() {
  try {
    const settings = await invoke("get_settings");
    $("settings-server").value = settings.server_url;
  } catch (e) {
    flash("settings-error", String(e));
  }
}

async function saveSettings() {
  clearNotices("settings-error", "settings-ok");
  try {
    const settings = await invoke("set_settings", { serverUrl: $("settings-server").value });
    $("settings-server").value = settings.server_url;
    flash("settings-ok", "Saved ✓");
  } catch (e) {
    flash("settings-error", String(e));
  }
}

// ------------------------------------------------- share (deep-link bridge)

// ShareActivity → freedback://share?text=… → deep-link plugin → Rust stores
// the pending share + emits `share`. We drain on startup (the link may have
// arrived before this script ran) AND on every event.
async function drainPendingShare() {
  try {
    const text = await invoke("take_pending_share");
    if (text) await lookUp(text);
  } catch (e) {
    console.error("pending share", e);
  }
}

// ----------------------------------------------------------------- wiring

window.addEventListener("DOMContentLoaded", () => {
  $("nav-back").addEventListener("click", async () => {
    if (currentView === "author" && returnTo) {
      const back = returnTo;
      returnTo = null;
      if (back.view === "feedback") await openFeedback(back.target);
      else show(back.view);
      return;
    }
    if (currentView === "feedback" || currentView === "journal") refreshLater();
    show("home");
    checkBackupNudge();
  });

  // Home
  $("resolve-form").addEventListener("submit", (e) => {
    e.preventDefault();
    const raw = $("resolve-input").value.trim();
    if (raw) lookUp(raw);
  });
  $("scan-btn").addEventListener("click", scanBarcode);
  initScan();
  for (const chip of document.querySelectorAll("#example-chips .chip")) {
    chip.addEventListener("click", () => {
      $("resolve-input").value = chip.dataset.example;
      lookUp(chip.dataset.example);
    });
  }
  $("nav-journal").addEventListener("click", async () => {
    show("journal");
    await refreshJournal();
  });
  $("nav-key").addEventListener("click", () => {
    $("key-export").value = "";
    $("key-qr").classList.add("hidden");
    $("key-qr").innerHTML = "";
    show("key");
  });
  $("nav-settings").addEventListener("click", async () => {
    show("settings");
    await loadSettings();
  });

  // Feedback composers
  $("c-stars-send").addEventListener("click", () => {
    const v = Number($("c-stars").value);
    if (v >= 1 && v <= 5) publishContribution({ kind: "stars", value: v });
    else flash("fb-error", "Pick 1–5 stars first.");
  });
  $("c-thumb-up").addEventListener("click", () => publishContribution({ kind: "thumb", up: true }));
  $("c-thumb-down").addEventListener("click", () => publishContribution({ kind: "thumb", up: false }));
  $("c-comment-send").addEventListener("click", () => {
    const text = $("c-comment").value.trim();
    if (!text) return flash("fb-error", "Write a comment first.");
    $("c-comment").value = "";
    publishContribution({ kind: "comment", text });
  });
  $("c-tag-send").addEventListener("click", () => {
    const text = $("c-tag").value.trim();
    if (!text) return flash("fb-error", "Type a tag first.");
    $("c-tag").value = "";
    publishContribution({ kind: "tag", text });
  });
  $("c-issue-send").addEventListener("click", () => {
    const text = $("c-issue").value.trim();
    if (!text) return flash("fb-error", "Describe the issue first.");
    $("c-issue").value = "";
    publishContribution({ kind: "issue", text });
  });

  // Author
  $("a-comment-send").addEventListener("click", () => {
    const text = $("a-comment").value.trim();
    if (!text) return flash("author-error", "Write a note first.");
    $("a-comment").value = "";
    publishAuthorNote(text);
  });

  // Key
  $("key-export-btn").addEventListener("click", exportKey);
  $("key-copy-btn").addEventListener("click", copyKey);
  $("key-import-btn").addEventListener("click", importKey);
  $("key-scan-btn").addEventListener("click", scanKeyQr);
  initKeyScan();

  // Settings
  $("settings-save").addEventListener("click", saveSettings);

  // Share bridge
  listen("share", drainPendingShare);
  drainPendingShare();

  // Backup nudge
  $("backup-nudge-btn").addEventListener("click", () => {
    $("key-export").value = "";
    show("key");
  });

  show("home");
  checkBackupNudge();
});

// No-op placeholder so back-navigation stays cheap; kept for symmetry.
function refreshLater() {}

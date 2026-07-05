"use strict";
// Headless-browser E2E for the Freedback widgets (issue #29).
//
// Renders the custom elements in a real DOM (Chromium) against an in-process
// feedback + collection stack and drives publish + read-back through the actual
// UI for stars / scalar / thumb / comment / issue / tag, over BOTH auth paths:
//   - data-sign  : self-signed P-256 via WebCrypto (the federating identity)
//   - data-token : an OAuth bearer (the siloed identity)
//
// The launcher (run-e2e.cjs) starts the stack and passes its URLs + the test
// bearer token through env. Waits are on explicit DOM conditions (the aggregate
// reflecting the new submission), never fixed timeouts, so the test is not flaky.
const { test, expect } = require("@playwright/test");

const FEEDBACK = process.env.FB_E2E_FEEDBACK; // http://127.0.0.1:8080/annotations/
const STATIC = process.env.FB_E2E_STATIC; // http://127.0.0.1:8099
const READ = process.env.FB_E2E_READ; // http://127.0.0.1:8100/index
const TOKEN = process.env.FB_E2E_TOKEN; // the configured OAuth bearer

test.beforeEach(() => {
  expect(FEEDBACK, "FB_E2E_FEEDBACK must be set by the launcher").toBeTruthy();
  expect(STATIC, "FB_E2E_STATIC must be set by the launcher").toBeTruthy();
  expect(READ, "FB_E2E_READ must be set by the launcher").toBeTruthy();
});

/** URL of the parametrized harness page for `auth` ('sign'|'token') + target. */
function harnessUrl(auth, target) {
  const u = new URL(STATIC + "/e2e/e2e.html");
  u.searchParams.set("feedback", FEEDBACK);
  u.searchParams.set("read", READ);
  u.searchParams.set("target", target);
  u.searchParams.set("auth", auth);
  if (auth === "token") u.searchParams.set("token", TOKEN);
  return u.toString();
}

// Surface page console errors to the test output for easier triage.
test.beforeEach(async ({ page }) => {
  page.on("pageerror", (e) => console.log("PAGE ERROR:", e.message));
  page.on("console", (m) => {
    if (m.type() === "error") console.log("PAGE CONSOLE ERROR:", m.text());
  });
});

// 1) The real demo.html renders and the self-signed publish→read-back works.
test("demo.html: self-signed stars publish and read back the aggregate", async ({ page }) => {
  // demo.html targets the default ports (8080/8100); the launcher pins those.
  await page.goto(STATIC + "/demo.html");
  const stars = page.locator("freedback-stars[data-publish]").first();
  await expect(stars.locator("button").first()).toBeVisible();

  // Publish a 5-star rating by clicking the 5th star, then wait for the
  // aggregate to reflect at least one rating (read back through the collection).
  await stars.locator('button[data-v="5"]').click();
  await expect(stars.locator(".fb-agg")).toContainText("(", { timeout: 15000 });
  await expect(stars.locator(".fb-agg")).not.toHaveText("", { timeout: 15000 });
  // The aggregate text is like " 5.0 (1)"; assert it counted our submission.
  await expect(stars.locator(".fb-agg")).toContainText(/\(\d+\)/, { timeout: 15000 });
});

// 2) Self-signed path across every widget kind on isolated targets.
test("e2e harness: self-signed publish + read back for all widget kinds", async ({ page }) => {
  const target = "https://example.com/item/sign-" + Date.now();
  await page.goto(harnessUrl("sign", target));
  await page.waitForFunction(() => window.__fbReady === true);

  // stars
  await page.locator("#stars button[data-v='4']").click();
  await expect(page.locator("#stars .fb-agg")).toContainText("4.0", { timeout: 15000 });
  await expect(page.locator("#stars .fb-agg")).toContainText("(1)", { timeout: 15000 });

  // thumb (up)
  await page.locator("#thumb button[data-up='1']").click();
  await expect(page.locator("#thumb .fb-agg")).toContainText("👍 1", { timeout: 15000 });

  // scalar (0..10 scale; set to 7 and submit)
  await page.locator("#scalar .fb-range").fill("7");
  await page.locator("#scalar .fb-send").click();
  await expect(page.locator("#scalar .fb-agg")).toContainText("avg 7", { timeout: 15000 });

  // comment (the li also carries the own-item `×` delete control, so match on
  // containment rather than exact text)
  await page.locator("#comment .fb-in").fill("great work");
  await page.locator("#comment form button").click();
  await expect(page.locator("#comment .fb-list li")).toHaveCount(1, { timeout: 15000 });
  await expect(page.locator("#comment .fb-list li")).toContainText("great work", { timeout: 15000 });

  // issue (ADR 0023): report a problem through the textarea; the signed
  // annotation (oa:TextualBody + the standard oa:editing motivation) is
  // accepted by the REAL feedback server and read back into the list.
  await page.locator("#issue .fb-in").fill("the checkout button does nothing");
  await page.locator("#issue form button").click();
  await expect(page.locator("#issue .fb-list li")).toHaveCount(1, { timeout: 15000 });
  await expect(page.locator("#issue .fb-list li")).toContainText("the checkout button does nothing", { timeout: 15000 });
  await expect(page.locator("#issue .fb-list li")).toHaveClass(/fb-issue-item/);
  // Being the visitor's OWN report, it carries the delete affordance too.
  await expect(page.locator("#issue .fb-list li .fb-del")).toBeVisible({ timeout: 15000 });

  // tag (own chip also carries the `×` delete control)
  await page.locator("#tag .fb-in").fill("rust");
  await page.locator("#tag form button").click();
  await expect(page.locator("#tag .fb-chip")).toHaveCount(1, { timeout: 15000 });
  await expect(page.locator("#tag .fb-chip")).toContainText("rust", { timeout: 15000 });

  // delete my feedback (right to erasure, ADR 0021): the just-published
  // comment is OWN (its creator.id matches this browser identity), so it
  // renders an `.fb-del` control. Clicking it signs a delete document with the
  // same WebCrypto key and DELETEs the annotation on the REAL feedback server;
  // on 204 the widget fires `freedback:deleted` and refreshes the list.
  const ownDel = page.locator("#comment .fb-list li .fb-del");
  await expect(ownDel).toBeVisible({ timeout: 15000 });
  await expect(ownDel).toHaveAttribute("aria-label", "Delete my feedback");
  // Observe the bubbling+composed freedback:deleted event at the document.
  await page.evaluate(() => {
    window.__fbDeleted = [];
    document.addEventListener("freedback:deleted", (e) =>
      window.__fbDeleted.push({ annotation: e.detail.annotation, status: e.detail.response && e.detail.response.status })
    );
  });
  await ownDel.click();
  await page.waitForFunction(() => window.__fbDeleted && window.__fbDeleted.length === 1, null, {
    timeout: 15000,
  });
  const deleted = await page.evaluate(() => window.__fbDeleted[0]);
  expect(deleted.status).toBe(204); // the server really erased it
  expect(deleted.annotation).toMatch(/^[0-9a-f]{64}$/); // the dedup id (content address)
  // After the post-delete refresh the comment is gone from the read-back list.
  await expect(page.locator("#comment .fb-list li")).toHaveCount(0, { timeout: 15000 });
  // And no error surfaced in the widget status line.
  await expect(page.locator("#comment .fb-status")).toHaveText("");
});

/** Minimal JCS (RFC 8785): sorted keys, no whitespace — matches the widget's
 * own jcs() for these plain string/object fixtures. */
function jcs(value) {
  return JSON.stringify(sortKeys(value));
}
function sortKeys(v) {
  if (Array.isArray(v)) return v.map(sortKeys);
  if (v && typeof v === "object") {
    const out = {};
    for (const k of Object.keys(v).sort()) out[k] = sortKeys(v[k]);
    return out;
  }
  return v;
}

/** Publish one throwaway signed comment directly via HTTP (bypassing the
 * browser) with a fresh identity and the given `created` timestamp. */
async function seedComment(feedbackUrl, target, created, value) {
  const { webcrypto } = require("node:crypto");
  const kp = await webcrypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const spki = Buffer.from(await webcrypto.subtle.exportKey("spki", kp.publicKey));
  const digest = Buffer.from(await webcrypto.subtle.digest("SHA-256", spki));
  const issuerId = "urn:freedback:key:" + digest.toString("hex");
  const kid = `-----BEGIN PUBLIC KEY-----\n${spki.toString("base64").match(/.{1,64}/g).join("\n")}\n-----END PUBLIC KEY-----\n`;
  const content = {
    "@context": ["http://www.w3.org/ns/anno.jsonld", "https://freedback.net/ns/context.jsonld"],
    type: "Annotation",
    motivation: "commenting",
    creator: { id: issuerId },
    created,
    target,
    body: [{ type: "TextualBody", value, format: "text/plain", purpose: "commenting" }],
    conformsTo: "https://freedback.net/profile/1",
  };
  const bytes = Buffer.from(jcs(content));
  const sigRaw = Buffer.from(await webcrypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, kp.privateKey, bytes));
  const sig = sigRaw.toString("base64").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  const resp = await fetch(feedbackUrl, {
    method: "POST",
    headers: { "content-type": "application/ld+json" },
    body: JSON.stringify({ ...content, signature: { alg: "ES256", kid, sig } }),
  });
  if (!resp.ok) throw new Error(`seed failed: ${resp.status} ${await resp.text()}`);
}

// 2b) Own-item delete affordance survives a target with more history than the
// server's default page size (issue: dogfood page's comment × control stopped
// appearing once the target accumulated 50+ older comments). Both `readUrl()`
// (widget → feedback/collection server) and the collection server's own
// upstream fetch must ask for the unbounded collection, not just page 0.
test("e2e harness: own-delete affordance survives a target past the default page size", async ({ page }) => {
  const target = "https://example.com/item/paginated-" + Date.now();
  const commentTarget = target + "#comment";

  // Seed 55 strictly-older comments directly (oldest-first server ordering
  // means these fill exactly the pages before ours).
  for (let i = 0; i < 55; i++) {
    const created = new Date(Date.now() - (55 - i) * 1000).toISOString();
    await seedComment(FEEDBACK, commentTarget, created, `seed-${i}`);
  }

  await page.goto(harnessUrl("sign", target));
  await page.waitForFunction(() => window.__fbReady === true);

  const marker = "our own comment " + Date.now();
  await page.locator("#comment .fb-in").fill(marker);
  await page.locator("#comment form button").click();
  await expect(page.locator(`#comment .fb-list li:has-text("${marker}")`)).toBeVisible({ timeout: 15000 });
  // The own-item delete control must render even though 55 older items exist.
  await expect(page.locator(`#comment .fb-list li:has-text("${marker}") .fb-del`)).toBeVisible({
    timeout: 15000,
  });
});

// 3) OAuth bearer (data-token) path: publish + read back the aggregate.
test("e2e harness: data-token (OAuth bearer) publish + read back", async ({ page }) => {
  expect(TOKEN, "FB_E2E_TOKEN must be set for the bearer path").toBeTruthy();
  const target = "https://example.com/item/token-" + Date.now();
  await page.goto(harnessUrl("token", target));
  await page.waitForFunction(() => window.__fbReady === true);

  // A bearer-authorized star rating must be accepted and read back.
  await page.locator("#stars button[data-v='3']").click();
  await expect(page.locator("#stars .fb-agg")).toContainText("3.0", { timeout: 15000 });
  await expect(page.locator("#stars .fb-agg")).toContainText("(1)", { timeout: 15000 });

  // A comment over the bearer path too.
  await page.locator("#comment .fb-in").fill("via bearer");
  await page.locator("#comment form button").click();
  await expect(page.locator("#comment .fb-list li")).toHaveText(["via bearer"], { timeout: 15000 });
});

// 4) Negative control: with neither data-sign nor data-token, a publish is
// rejected (401) and the aggregate stays empty — proving auth is enforced.
test("e2e harness: unauthenticated publish is rejected", async ({ page }) => {
  const target = "https://example.com/item/noauth-" + Date.now();
  const u = new URL(STATIC + "/e2e/e2e.html");
  u.searchParams.set("feedback", FEEDBACK);
  u.searchParams.set("read", READ);
  u.searchParams.set("target", target);
  u.searchParams.set("auth", "none");
  await page.goto(u.toString());
  await page.waitForFunction(() => window.__fbReady === true);

  await page.locator("#stars button[data-v='5']").click();
  // The widget surfaces the publish error in .fb-status; the aggregate stays "–".
  await expect(page.locator("#stars .fb-status")).toContainText(/publish failed: 401/, {
    timeout: 15000,
  });
  await expect(page.locator("#stars .fb-agg")).toContainText("–");
});

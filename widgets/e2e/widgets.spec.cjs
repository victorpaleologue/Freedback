"use strict";
// Headless-browser E2E for the Freedback widgets (issue #29).
//
// Renders the custom elements in a real DOM (Chromium) against an in-process
// feedback + collection stack and drives publish + read-back through the actual
// UI for stars / scalar / thumb / comment / tag, over BOTH auth paths:
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

  // comment
  await page.locator("#comment .fb-in").fill("great work");
  await page.locator("#comment form button").click();
  await expect(page.locator("#comment .fb-list li")).toHaveText(["great work"], { timeout: 15000 });

  // tag
  await page.locator("#tag .fb-in").fill("rust");
  await page.locator("#tag form button").click();
  await expect(page.locator("#tag .fb-chip")).toHaveText(["rust"], { timeout: 15000 });
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

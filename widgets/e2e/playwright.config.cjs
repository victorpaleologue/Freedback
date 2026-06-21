"use strict";
// Playwright config for the widgets E2E. The stack is started by run-e2e.cjs
// (not Playwright's webServer), which passes the live URLs through env. Chromium
// only; deterministic single worker; no retries so flakiness surfaces loudly.
const { defineConfig, devices } = require("@playwright/test");

module.exports = defineConfig({
  testDir: __dirname,
  testMatch: /.*\.spec\.cjs$/,
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 30000,
  expect: { timeout: 15000 },
  reporter: [["list"]],
  use: {
    headless: true,
    // localhost / 127.0.0.1 is a secure context, so WebCrypto (data-sign) works.
    ignoreHTTPSErrors: true,
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});

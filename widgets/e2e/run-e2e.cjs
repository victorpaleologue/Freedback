"use strict";
// Launcher for the widgets headless-browser E2E (issue #29).
//
// Brings up an in-process feedback + collection stack with an in-memory store on
// fixed local ports, serves the widgets statically, then runs the Playwright
// spec against it and tears everything down. Deterministic and CI-ready.
//
//   node widgets/e2e/run-e2e.cjs
//
// Env overrides: FB_FEEDBACK_PORT (8080), FB_COLLECTION_PORT (8100),
// FB_STATIC_PORT (8099), FB_BIN_DIR (target/debug). Set FB_KEEP=1 to leave the
// stack up (no Playwright) for manual poking.
const { spawn } = require("node:child_process");
const path = require("node:path");
const fs = require("node:fs");
const http = require("node:http");
const { startStatic } = require("./serve.cjs");

const REPO_ROOT = path.resolve(__dirname, "..", "..");
const WIDGETS_DIR = path.resolve(__dirname, "..");

const FEEDBACK_PORT = Number(process.env.FB_FEEDBACK_PORT || 8080);
const COLLECTION_PORT = Number(process.env.FB_COLLECTION_PORT || 8100);
const STATIC_PORT = Number(process.env.FB_STATIC_PORT || 8099);

// A fixed, deterministic demo OAuth bearer for the data-token path. The
// feedback server accepts exactly this token (mapped to an app-scoped identity).
const OAUTH_TOKEN = "e2e-test-bearer-token";
const OAUTH_APP = "e2e-app";
const OAUTH_USER = "e2e-user";

const BIN_DIR = process.env.FB_BIN_DIR || path.join(REPO_ROOT, "target", "debug");
const exe = (name) => path.join(BIN_DIR, process.platform === "win32" ? `${name}.exe` : name);
const FEEDBACK_BIN = exe("freedback-feedback-server");
const COLLECTION_BIN = exe("freedback-collection-server");

const children = [];
let staticServer = null;

/** Spawn a server binary with env, piping its output (prefixed) to ours. */
function spawnServer(label, bin, env) {
  if (!fs.existsSync(bin)) {
    throw new Error(
      `missing binary ${bin}\n` +
        `build it first: cargo build -p freedback-feedback-server -p freedback-collection-server`
    );
  }
  const child = spawn(bin, [], { env: { ...process.env, ...env }, stdio: ["ignore", "pipe", "pipe"] });
  const tag = (line) => process.stdout.write(`[${label}] ${line}`);
  child.stdout.on("data", (d) => tag(d.toString()));
  child.stderr.on("data", (d) => tag(d.toString()));
  child.on("exit", (code) => {
    if (code !== 0 && code !== null) console.log(`[${label}] exited with code ${code}`);
  });
  children.push(child);
  return child;
}

/** Resolve once `url` answers any HTTP status, or reject after `timeoutMs`. */
function waitForHttp(url, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const attempt = () => {
      const req = http.get(url, (res) => {
        res.resume();
        resolve();
      });
      req.on("error", () => {
        if (Date.now() > deadline) reject(new Error(`timed out waiting for ${url}`));
        else setTimeout(attempt, 250);
      });
    };
    attempt();
  });
}

function shutdown() {
  for (const c of children) {
    try {
      c.kill("SIGTERM");
    } catch (_e) {
      /* ignore */
    }
  }
  if (staticServer) {
    try {
      staticServer.close();
    } catch (_e) {
      /* ignore */
    }
  }
}

async function main() {
  const feedbackBase = `http://127.0.0.1:${FEEDBACK_PORT}`;
  const collectionBase = `http://127.0.0.1:${COLLECTION_PORT}`;
  const staticBase = `http://127.0.0.1:${STATIC_PORT}`;

  // 1) feedback-server: in-memory store, permissive CORS, max-age=0 (so the
  //    aggregator always revalidates → deterministic read-back), and a single
  //    fixed demo OAuth bearer for the data-token path.
  spawnServer("feedback", FEEDBACK_BIN, {
    FREEDBACK_BIND: `127.0.0.1:${FEEDBACK_PORT}`,
    FREEDBACK_BASE_URL: feedbackBase,
    FREEDBACK_CORS_PERMISSIVE: "1",
    FREEDBACK_CACHE_MAX_AGE: "0",
    FREEDBACK_OAUTH_TOKEN: OAUTH_TOKEN,
    FREEDBACK_OAUTH_APP: OAUTH_APP,
    FREEDBACK_OAUTH_USER: OAUTH_USER,
    RUST_LOG: process.env.RUST_LOG || "warn",
  });

  // 2) collection-server: aggregates from the feedback server, permissive CORS.
  spawnServer("collection", COLLECTION_BIN, {
    FREEDBACK_BIND: `127.0.0.1:${COLLECTION_PORT}`,
    FREEDBACK_BASE_URL: collectionBase,
    FREEDBACK_SERVERS: feedbackBase,
    FREEDBACK_CORS_PERMISSIVE: "1",
    RUST_LOG: process.env.RUST_LOG || "warn",
  });

  // 3) static server for demo.html + the parametrized harness page.
  staticServer = await startStatic(WIDGETS_DIR, STATIC_PORT);

  // Wait for both servers to answer before driving the browser.
  await waitForHttp(`${feedbackBase}/annotations/?target=https://example.com/probe`);
  await waitForHttp(`${collectionBase}/index?target=https://example.com/probe`);
  await waitForHttp(`${staticBase}/demo.html`);
  console.log("stack ready:", { feedbackBase, collectionBase, staticBase });

  if (process.env.FB_KEEP) {
    console.log("FB_KEEP set — leaving stack up. Ctrl-C to stop.");
    return new Promise(() => {});
  }

  // Run Playwright against the live stack.
  const env = {
    ...process.env,
    FB_E2E_FEEDBACK: `${feedbackBase}/annotations/`,
    FB_E2E_READ: `${collectionBase}/index`,
    FB_E2E_STATIC: staticBase,
    FB_E2E_TOKEN: OAUTH_TOKEN,
  };
  const code = await new Promise((resolve) => {
    const pw = spawn(
      process.platform === "win32" ? "npx.cmd" : "npx",
      ["playwright", "test", "--config", path.join(__dirname, "playwright.config.cjs")],
      { cwd: WIDGETS_DIR, env, stdio: "inherit" }
    );
    pw.on("exit", (c) => resolve(c == null ? 1 : c));
  });
  return code;
}

let exitCode = 1;
main()
  .then((code) => {
    exitCode = typeof code === "number" ? code : 0;
  })
  .catch((e) => {
    console.error(e);
    exitCode = 1;
  })
  .finally(() => {
    shutdown();
    // Give children a moment to die before exiting.
    setTimeout(() => process.exit(exitCode), 300);
  });

process.on("SIGINT", () => {
  shutdown();
  process.exit(130);
});
process.on("SIGTERM", () => {
  shutdown();
  process.exit(143);
});

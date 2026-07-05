// Test harness for the native (tauri-driver) suite: builds/locates the app
// and server binaries, runs a REAL freedback-feedback-server (in-memory
// store) on a local port, and prepares an isolated XDG profile so the app
// under test starts from a clean first-run state pointed at the test server.
"use strict";

const { spawn, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const http = require("node:http");

const REPO_ROOT = path.resolve(__dirname, "..", "..", "..");
const APP_WORKSPACE = path.resolve(__dirname, "..");

const APP_BINARY = path.join(APP_WORKSPACE, "target", "debug", "freedback-app");
const SERVER_BINARY = path.join(REPO_ROOT, "target", "debug", "freedback-feedback-server");

/** The port the local feedback server listens on during the suite. */
const SERVER_PORT = 43125;
const SERVER_URL = `http://127.0.0.1:${SERVER_PORT}`;

function buildIfMissing(binary, args, cwd) {
  if (fs.existsSync(binary)) return;
  console.log(`[harness] building ${path.basename(binary)}…`);
  const res = spawnSync("cargo", args, { cwd, stdio: "inherit" });
  if (res.status !== 0) throw new Error(`cargo build failed for ${binary}`);
}

/** Ensure the app + server binaries exist (CI pre-builds; local runs build). */
function ensureBinaries() {
  buildIfMissing(APP_BINARY, ["build", "-p", "freedback-app"], APP_WORKSPACE);
  buildIfMissing(SERVER_BINARY, ["build", "-p", "freedback-feedback-server"], REPO_ROOT);
}

/** Start the real feedback server; resolves once it answers HTTP. */
async function startServer() {
  const child = spawn(SERVER_BINARY, [], {
    env: { ...process.env, FREEDBACK_BIND: `127.0.0.1:${SERVER_PORT}` },
    stdio: ["ignore", "inherit", "inherit"],
  });
  const deadline = Date.now() + 15_000;
  for (;;) {
    if (Date.now() > deadline) {
      child.kill();
      throw new Error("feedback server did not come up in 15s");
    }
    const ok = await new Promise((resolve) => {
      const req = http.get(`${SERVER_URL}/.well-known/freedback`, (res) => {
        res.resume();
        resolve(res.statusCode === 200);
      });
      req.on("error", () => resolve(false));
      req.setTimeout(1000, () => {
        req.destroy();
        resolve(false);
      });
    });
    if (ok) return child;
    await new Promise((r) => setTimeout(r, 200));
  }
}

/**
 * A fresh, isolated profile: temp XDG dirs so the app's
 * `app_data_dir()` (`$XDG_DATA_HOME/net.freedback.app`) starts empty except
 * for a seeded settings.json pointing at the local test server.
 */
function makeProfile() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "freedback-e2e-"));
  const dataHome = path.join(root, "data");
  const appData = path.join(dataHome, "net.freedback.app");
  fs.mkdirSync(appData, { recursive: true });
  fs.writeFileSync(
    path.join(appData, "settings.json"),
    JSON.stringify({ server_url: SERVER_URL }, null, 2)
  );
  return {
    root,
    env: {
      XDG_DATA_HOME: dataHome,
      XDG_CONFIG_HOME: path.join(root, "config"),
      XDG_CACHE_HOME: path.join(root, "cache"),
    },
  };
}

module.exports = {
  APP_BINARY,
  SERVER_BINARY,
  SERVER_URL,
  ensureBinaries,
  startServer,
  makeProfile,
};

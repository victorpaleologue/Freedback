// WebdriverIO configuration for the native (tauri-driver) UX suite.
//
// The chain: wdio → tauri-driver (:4444) → WebKitWebDriver → the REAL
// freedback-app debug binary, with an isolated XDG profile pointed at a
// local freedback-feedback-server. Runs headless under `xvfb-run`.
// See https://v2.tauri.app/develop/tests/webdriver/ for the pattern.
"use strict";

const { spawn } = require("node:child_process");
const os = require("node:os");
const path = require("node:path");

const harness = require("./harness.cjs");

let tauriDriver = null;
let server = null;
let profile = null;

exports.config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  specs: ["./specs/**/*.cjs"],
  maxInstances: 1,
  capabilities: [
    {
      maxInstances: 1,
      "tauri:options": {
        application: harness.APP_BINARY,
      },
    },
  ],
  logLevel: "warn",
  waitforTimeout: 15000,
  connectionRetryTimeout: 120000,
  connectionRetryCount: 2,
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    // First invoke compiles nothing but does hit the network-less local
    // server; keep a generous ceiling for slow CI runners.
    timeout: 120000,
  },

  onPrepare: async () => {
    harness.ensureBinaries();
    server = await harness.startServer();
  },

  beforeSession: () => {
    profile = harness.makeProfile();
    // tauri-driver spawns the app; the app inherits this environment, so the
    // temp XDG dirs give it a clean first-run data dir.
    tauriDriver = spawn(
      path.resolve(os.homedir(), ".cargo", "bin", "tauri-driver"),
      [],
      {
        stdio: [null, process.stdout, process.stderr],
        env: { ...process.env, ...profile.env },
      }
    );
    tauriDriver.on("error", (error) => {
      console.error("tauri-driver error:", error);
      process.exit(1);
    });
  },

  afterSession: () => {
    if (tauriDriver) tauriDriver.kill();
  },

  onComplete: () => {
    if (server) server.kill();
  },
};

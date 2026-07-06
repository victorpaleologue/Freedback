# Freedback mobile app (Tauri 2, Android-first)

Scan or enter a **barcode (GTIN)**, an **ISBN**, or a **URL** → resolve it to a
canonical target URI → see the feedback (star average, thumbs, comments, tags)
→ contribute your own (published under **CC BY 4.0** by default) → manage it in
**My feedback** (update-by-supersede / erase, ADR 0021) → move your account
with **My key** (the P-256 key IS the account — no signup).

Server: configurable in Settings, default `https://freedback-demo.fly.dev`.

## Architecture

```
apps/mobile/                # its OWN Cargo workspace (root excludes it)
  core/                     # freedback-app-core: ALL logic, host-testable, ZERO tauri deps
    src/input.rs            #   GTIN/ISBN/URL/free-text → canonical target URI
    src/share.rs            #   freedback:// + freedback.net/l deep links → resolution
    src/identity.rs         #   mint-on-first-use P-256 key, PEM export/import
    src/journal.rs          #   redb "My feedback" journal (supersede/delete status)
    src/feedback.rs         #   Contribution kinds + aggregation (FeedbackView)
    src/lib.rs              #   AppCore facade (settings, publish, update, erase, pending share)
  src-tauri/                # thin Tauri 2 shell: every command delegates to core
    src/lib.rs              #   commands + the deep-link → pending-share bridge
    gen/android/            #   the Android project (COMMITTED — see below)
  ui/                       # vanilla static frontend (withGlobalTauri, no bundler)
  e2e/                      # native UX suite: tauri-driver + WebdriverIO
```

The protocol work is entirely in the shared crates (path deps):
`freedback-protocol` (model, signing, JCS dedup ids, delete documents),
`freedback-cli-client` (read/write/delete/sync over reqwest + rustls),
patterns from `freedback-advanced-client` (redb store) for the journal.

### The Android share bridge

`ShareActivity` (a dedicated `<activity>` labeled *"Look up feedback…"*,
handling `ACTION_SEND text/plain` and `ACTION_PROCESS_TEXT`) reads the shared
text, rewrites it as a `freedback://share?text=<urlencoded>` VIEW intent aimed
at `MainActivity`, and finishes. The official `tauri-plugin-deep-link` (v2)
delivers the URI to Rust, which stores the decoded text as the *pending share*
and emits a `share` event; the webview drains it with the `take_pending_share`
command (on startup AND on each event, so nothing is lost if the link beats
the JS listeners). `https://freedback.net/l` lookup App Links ride the same
path via the plugin config in `tauri.conf.json`.

## Test story (three tiers)

1. **Core logic tests** (`core/` — `cargo test -p freedback-app-core`):
   narrow, fast Rust tests of the pure logic — GS1 check digits, ISBN-10/13
   conversion, free-text extraction, share URI parsing, journal persistence &
   ordering, PEM lifecycle, aggregation math — plus orchestration tests
   against an in-process feedback server (the `cli-client/tests/e2e.rs`
   pattern) for the error paths that need a server (403 foreign-key delete,
   410 tombstoned re-publish, missing-key erase).
2. **Native UX suite** (`e2e/` — the PRIMARY coverage): `tauri-driver`
   (WebDriver) drives the **real desktop binary** — real webview, real
   `invoke()` IPC, real Rust core, and a **real local
   `freedback-feedback-server`** (in-memory store) — through the actual UI:
   first-run key mint, every resolve path, publishing each kind, journal
   update/erase, key export/import, settings. Fully offline + deterministic.
3. **Android instrumentation tests**
   (`src-tauri/gen/android/app/src/androidTest/`): espresso-intents tests for
   what only a device proves — `ShareActivity` really forwards `ACTION_SEND` /
   `ACTION_PROCESS_TEXT` as the `freedback://share` intent and finishes.

CI: `.github/workflows/app-ci.yml` (jobs `core`, `shell`, `desktop-e2e`,
`android-build`, `android-test`); releases: `mobile-release.yml` (tag
`mobile-v*`, signing guarded on `ANDROID_*` secrets).

## Dev setup

### Desktop (fastest loop)

```sh
# Linux deps (Debian/Ubuntu):
sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
npm i -g @tauri-apps/cli
cd apps/mobile/src-tauri
tauri dev            # or: cargo build -p freedback-app && ../target/debug/freedback-app
```

### Native UX suite

```sh
sudo apt-get install -y webkit2gtk-driver xvfb
cargo install tauri-driver --locked
cargo build -p freedback-app --manifest-path apps/mobile/Cargo.toml
cargo build -p freedback-feedback-server            # repo root
cd apps/mobile/e2e && npm ci && xvfb-run -a npm test
```

### Android

Requirements: JDK 17, the Android SDK (platform 36, build-tools 36) and NDK
(r27), `ANDROID_HOME` + `NDK_HOME` exported, rust targets
(`rustup target add aarch64-linux-android x86_64-linux-android …`), and the
npm Tauri CLI (`npm i -g @tauri-apps/cli` — the Gradle rust plugin calls the
`tauri` bin from `PATH`).

```sh
cd apps/mobile/src-tauri
tauri android build --debug --apk --target aarch64   # debug APK
tauri android dev                                     # device/emulator loop
# ShareActivity intent tests (device/emulator attached):
(cd gen/android && ./gradlew connectedX86_64DebugAndroidTest)
```

### `gen/android` is committed — regeneration policy

`src-tauri/gen/android` is the real Android project and **carries manual
customizations** (all marked `CUSTOMIZED` in-file). If you ever regenerate it
with `tauri android init`, you MUST preserve:

- `app/src/main/AndroidManifest.xml` — the `freedback://` intent-filter on
  `MainActivity` (which must stay the FIRST `<activity>`: the deep-link
  plugin injects its filters before the first `</activity>`) and the whole
  `ShareActivity` declaration;
- `app/src/main/java/net/freedback/app/ShareActivity.kt`;
- `app/src/androidTest/**` (the intent tests) plus the `androidTest`
  dependencies and `testInstrumentationRunner` in `app/build.gradle.kts`;
- `app/src/main/res/values/strings.xml` — `share_activity_title`;
- `buildSrc/.../BuildTask.kt` — invokes the globally installed `tauri` bin
  (the template's `node tauri …` only resolves inside an npm project).

## Roadmap notes

- **Camera scanning**: the Home screen's "Scan (soon)" button is the landing
  spot for the barcode-scanner plugin (M3).
- **Issue reports**: `Body::Issue` (ADR 0023) has landed in `freedback-protocol`;
  `FeedbackView::issues` now aggregates them. The remaining `// TODO(issue-type)`
  seam is submission: `Contribution` and the composer UI don't yet expose an
  "Issue" action, so issues can only be read, not created, from the app.
- The Feedback screen aggregates the **read view** (every annotation). The
  protocol's edit-supersession collapses per `(issuer, target)` in the
  `/sync` latest-edits view (`AppCore::get_feedback_latest`); merging both
  into one "smart" view is the collection server's job (component 7).

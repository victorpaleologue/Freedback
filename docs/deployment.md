# Deployment

## One-command demo (Docker Compose)

```bash
docker compose up --build
```

Brings up the full stack:

| Service | Port | Role |
|---|---|---|
| `feedback`   | 8080 | publish + serve + `/sync` |
| `discovery`  | 8090 | announce + resolve |
| `collection` | 8100 | index + equivalence + polite cache |

The collection server is pre-pointed at the feedback server
(`FREEDBACK_SERVERS=http://feedback:8080`). Try it:

```bash
curl localhost:8080/.well-known/freedback
curl 'localhost:8100/index?target=https://example.com/item/1'
```

## Single server

```bash
docker build -t freedback .
docker run -p 8080:8080 -e FREEDBACK_BASE_URL=https://feedback.example.org \
  freedback freedback-feedback-server
```

### Environment

| Var | Default | Applies to |
|---|---|---|
| `FREEDBACK_BIND` | `0.0.0.0:8080` (image) | all servers |
| `FREEDBACK_BASE_URL` | `http://<bind>` | all servers (mints ids / links) |
| `FREEDBACK_SERVERS` | — | collection (comma-separated upstreams) |
| `FREEDBACK_STORE_PATH` | — | feedback (JSON-Lines snapshot file; in-memory backend) |
| `FREEDBACK_ROCKSDB_PATH` | — | feedback (durable RocksDB dir; needs the `rocksdb` build) |
| `FREEDBACK_OAUTH_TOKEN` / `_APP` / `_USER` | — | feedback (one demo bearer token) |
| `FREEDBACK_DEFAULT_LICENSE` | — | feedback (license IRI advertised as `"license"` in `/.well-known/freedback`; annotations without an explicit `rights` fall under it — ADR 0022) |

## Storage

This image uses **Oxigraph's in-memory backend** — the workspace pins
`oxigraph` with `default-features = false`, so RocksDB and Clang are not needed
and the build is a clean single binary.

**Durable demo storage (snapshots).** Set `FREEDBACK_STORE_PATH` to a file on a
mounted volume and the feedback server loads it on boot, re-snapshots every 60 s,
and snapshots on graceful shutdown (Ctrl-C / SIGTERM). The compose stack does
this on a named volume, so feedback **survives restarts**. The format is
JSON-Lines (one annotation per line), backend-agnostic and `put`-idempotent.
This is snapshot — **not transactional** — persistence (ADR 0008): a crash
between snapshots can lose up to ~60 s of writes. Leave `FREEDBACK_STORE_PATH`
unset for the old ephemeral behavior.

**Durable RocksDB backend (`rocksdb` feature).** For real transactional
persistence, build the feedback server with the on-disk Oxigraph/RocksDB backend
and point it at a directory:

```bash
# Native:
cargo run -p freedback-feedback-server --features rocksdb
#   with FREEDBACK_ROCKSDB_PATH=/var/lib/freedback/db

# Docker (durable image — the rust image has the C/C++ toolchain RocksDB needs):
docker build --build-arg FEEDBACK_FEATURES=rocksdb -t freedback:durable .
docker run -p 8080:8080 -v freedback-db:/data \
  -e FREEDBACK_ROCKSDB_PATH=/data/db freedback:durable freedback-feedback-server
```

When `FREEDBACK_ROCKSDB_PATH` is set on a `rocksdb` build, every write is
persisted directly (no snapshot loop) and survives a hard restart. On a build
without the feature the variable is ignored (with a warning) and the in-memory +
snapshot path is used. RocksDB needs Clang/LLVM (or g++) at build time, which is
why it is opt-in rather than the default demo image.

## Releases

### Per-package releases (the primary model)

Every releasable unit is versioned, tagged, and released **independently**. The
set of packages is declared once in `.github/packages.json`:

| Package | Path | Tag |
|---|---|---|
| `protocol-lib`, `storage`, `feedback-server`, `cli-client`, `discovery-server`, `collection-server`, `advanced-client` | `crates/*` | `<name>-v<version>` |
| `widgets` (`@freedback/widgets`) | `widgets/` | `widgets-v<version>` |
| `mobile` (the Tauri app) | `apps/mobile/` | `mobile-v<version>` |
| `firefox-extension` | `firefox-extension/` | `firefox-extension-v<version>` |

Two rules make this run without anyone cutting tags by hand:

1. **Bump-on-touch (enforced at PR time).** `.github/workflows/versions.yml`
   fails a pull request that changes a package's code without bumping that
   package's version (docs-only changes are exempt). Each crate carries its own
   `version` in its `Cargo.toml` (no longer inherited from the workspace);
   `widgets` uses its `package.json` version; the mobile app uses its workspace
   version; the extension uses its `manifest.json` version.
2. **Tag + release on merge.** `.github/workflows/tag-and-release.yml` runs on
   every push to `main`: for each package whose current version has no tag yet,
   it creates `<name>-v<version>` and a GitHub Release. It's idempotent — a
   merge that bumped only `feedback-server` releases only `feedback-server`.
   - **binary crates** (the servers, `freedback`, `freedback-sync`) attach a
     static `x86_64-unknown-linux-musl` binary (the build also gates the
     release — broken code can't publish);
   - **library crates** (`protocol-lib`, `storage`) and the extension get a
     notes-only Release;
   - **`widgets`** additionally runs `npm publish` (guarded on `NPM_TOKEN`);
   - **`mobile`** hands off to `mobile-release.yml`, which builds the signed
     APK/AAB and publishes behind its own `app-ci.yml` gate.

The `changes` job in `ci.yml` also uses the package layout to run **only the
suites affected by a change** (a docs- or ontology-only PR skips the Rust and
browser matrices).

The binary crates' Releases each carry a fully static
`x86_64-unknown-linux-musl` binary (built with **`cargo-zigbuild`** — zig as the
C cross-compiler for `ring`; no glibc, no OpenSSL, no RocksDB, runs on any
x86-64 Linux) plus its `.sha256`. Library crates and the extension get a
notes-only Release. (There is no aggregate all-in-one bundle any more — each
package ships on its own tag.)

### Publishing `@freedback/widgets` to npm (guarded)

`tag-and-release.yml` publishes the drop-in widgets as
**[`@freedback/widgets`](https://www.npmjs.com/package/@freedback/widgets)** (the
scope reserved in `docs/naming.md`) as part of the `widgets` package's release,
so React/any apps can `npm add @freedback/widgets`. It builds the ESM + UMD
bundles and the bundled `.d.ts` from the canonical `widgets/freedback-widgets.js`
via the package's `prepublishOnly` hook, then `npm publish --access public`.

It is **guarded**: it runs only when an `NPM_TOKEN` repo secret is present.
Until the owner enables it, the step is **skipped cleanly** — the tag + GitHub
Release still go out, just no npm publish. To turn it on (one-time, **outside
the repo**):

1. **Create the `@freedback` npm org/scope** at <https://www.npmjs.com/org/create>
   (or reserve the scope under your user). The package name `@freedback/widgets`
   needs the `@freedback` scope to exist and be owned by the publishing account.
   (The unscoped name `freedback` is taken by an unrelated dormant package —
   `docs/naming.md`.)
2. **Mint an automation token** (npm → Access Tokens → *Granular*/*Automation*,
   with publish rights to `@freedback/*`).
3. **Add it as a repo secret** named **`NPM_TOKEN`** (GitHub → Settings →
   Secrets and variables → Actions → New repository secret).

Then bumping `widgets`'s version and merging publishes it at that version. npm
rejects re-publishing an already-published version, so the version must be new
(the bump-on-touch gate enforces that). The `<script>`/CDN path
(`https://freedback.net/widgets/freedback-widgets.js`, served by Pages) keeps
working independently of npm.

## TLS

The servers speak HTTP/1.1. Terminate TLS at a reverse proxy (Caddy/Traefik) in
front of them. Browser/WASM clients have TLS managed by the browser.

## Static artifacts (GitHub Pages)

`.github/workflows/pages.yml` publishes the protocol artifacts at the stable
`freedback.net` URLs that `protocol-lib::context` pins:

- `https://freedback.net/ns/context.jsonld` — the JSON-LD `@context`
- `https://freedback.net/ns/freedback.ttl` — the vocabulary (also at `/ns`)
- `https://freedback.net/ns/shapes.ttl` — the SHACL shapes
- `https://freedback.net/profile/1` — the validation profile (`dcterms:conformsTo`)
- `https://freedback.net/widgets/freedback-widgets.js` — the drop-in widget script

Pages is **static-only** and never runs a server. The workflow writes a `CNAME`
of `freedback.net` into the artifact, so once the apex DNS points at GitHub Pages
the site serves on the custom domain with auto-provisioned HTTPS.

### Attaching `freedback.net` (one-time, registrar + GitHub UI)

These steps are **outside the repo** and must be done by the domain owner:

1. **DNS at your registrar** — point the apex `freedback.net` at GitHub Pages:
   - `A` → `185.199.108.153`, `185.199.109.153`, `185.199.110.153`, `185.199.111.153`
   - `AAAA` → `2606:50c0:8000::153`, `2606:50c0:8001::153`, `2606:50c0:8002::153`, `2606:50c0:8003::153`
   - (optional) `CNAME` `www` → `<owner>.github.io`
2. **Repo → Settings → Pages**: set **Source = GitHub Actions** (the `pages.yml`
   workflow), set **Custom domain = `freedback.net`**, then enable **Enforce
   HTTPS** once the certificate is issued.
3. **(Recommended) Verify the domain** under GitHub **Settings → Pages → "Verified
   domains"** (add the `TXT _github-pages-challenge-…` record it shows) to prevent
   takeover.

`freedback.dev` is **not** available (taken by an unrelated, dormant npm homonym
— `docs/naming.md`); `freedback.net` is the owned, canonical base.

## CI validation

- `ci.yml` — fmt, clippy, native tests, wasm32 builds, the
  `x86_64-unknown-linux-musl` static build, widgets unit + headless E2E, and
  ontology parse checks.
- `container.yml` — builds this image (on deploy-config / lockfile changes or on
  demand), so the Dockerfile can't silently rot.
- `pages.yml` — publishes the static artifacts from `main`.
- `versions.yml` — PR gate: a package touched without a version bump fails.
- `tag-and-release.yml` — on merge to `main`, tags + releases each package whose
  version is new (per-package musl binary, notes, or npm publish), and dispatches
  the mobile release. See "Per-package releases" above.
- `app-ci.yml` / `mobile-release.yml` — the mobile app's own CI and release
  pipeline (`apps/mobile/README.md`); `mobile-release.yml`'s Release publish is
  gated on `app-ci.yml` having succeeded for the released commit.

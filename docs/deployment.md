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

`.github/workflows/release.yml` cuts artifacts on a semver tag:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

It attaches to the GitHub Release for the tag:

- **`freedback-<tag>-x86_64-linux-musl.tar.gz`** — fully static
  `x86_64-unknown-linux-musl` binaries (all five executables: the three servers,
  the `freedback` CLI, and `freedback-sync`), built with **`cargo-zigbuild`**
  (zig as the C cross-compiler for `ring`). No glibc, no OpenSSL (rustls), no
  RocksDB — runs on any x86-64 Linux.
- **`freedback-wasm-<tag>.tar.gz`** — the `wasm32-unknown-unknown` build of the
  protocol core + cli-client, bundled with the pinned ontology.

Each archive ships a `.sha256`. `workflow_dispatch` builds the artifacts without
publishing a Release (smoke test).

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
- `release.yml` — on a `v*` tag, publishes the musl binaries + wasm package to
  the GitHub Release.

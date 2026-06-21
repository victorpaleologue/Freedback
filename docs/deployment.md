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
| `FREEDBACK_OAUTH_TOKEN` / `_APP` / `_USER` | — | feedback (one demo bearer token) |

## Storage caveat (important)

This image uses **Oxigraph's in-memory backend** — the workspace pins
`oxigraph` with `default-features = false`, so RocksDB and Clang are not needed
and the build is a clean single binary. **Data is therefore ephemeral**: it does
not survive a restart. This is the "one-command demo" posture.

A durable build (RocksDB backend, or a `dump/load` on shutdown) is tracked as
future work. The static-binary `x86_64-unknown-linux-musl` target (via
`cargo-zigbuild`) is also future work; the current image is glibc/`debian-slim`.

## TLS

The servers speak HTTP/1.1. Terminate TLS at a reverse proxy (Caddy/Traefik) in
front of them. Browser/WASM clients have TLS managed by the browser.

## Static artifacts (GitHub Pages)

`.github/workflows/pages.yml` publishes the protocol artifacts at stable URLs:

- `/ns/context.jsonld` — the JSON-LD `@context`
- `/ns/freedback.ttl` — the vocabulary
- `/ns/shapes.ttl` — the SHACL shapes

Pages is **static-only** and never runs a server. The pinned IRIs use
`freedback.org`; the custom domain (CNAME) is pending registrar confirmation —
see [`naming.md`](./naming.md). `freedback.dev` is **not** available (taken by an
unrelated, dormant npm homonym).

## CI validation

- `ci.yml` — fmt, clippy, native tests, wasm32 builds, ontology parse checks.
- `container.yml` — builds this image (on deploy-config / lockfile changes or on
  demand), so the Dockerfile can't silently rot.
- `pages.yml` — publishes the static artifacts from `main`.

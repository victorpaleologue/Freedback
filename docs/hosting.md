# Hosting a Freedback demo (discovery + feedback server)

The servers are long-running HTTP/1.1 binaries with a `Dockerfile`, so any
container host works. This guide uses **Fly.io** (always-on-ish, free allowance,
deploys straight from the Dockerfile, free volumes for durable storage) and
notes a **Hugging Face Docker Space** alternative. GitHub Pages can't host them —
it's static-only (it serves the `@context`/ontology/shapes + the widget demo).

## Fly.io (recommended)

Configs live in [`deploy/fly/`](../deploy/fly/): `feedback.toml` (the demo
feedback server) and `discovery.toml` (the registry). They run the prebuilt
image's binaries via `[processes]`, scale to zero when idle, and force HTTPS.

### One-time

```bash
# 1. Install + log in
curl -L https://fly.io/install.sh | sh
fly auth login

# 2. Create the two apps (run from the repo root so the build context is the
#    workspace and ./Dockerfile is found). --copy-config keeps our toml.
fly launch --no-deploy --copy-config --config deploy/fly/feedback.toml   --name freedback-demo
fly launch --no-deploy --copy-config --config deploy/fly/discovery.toml  --name freedback-discovery
```

> Pick app names you own; if you change them, update the `app =` line in each
> toml and the `FREEDBACK_BASE_URL` once you know the public host.

### Deploy

```bash
fly deploy --config deploy/fly/feedback.toml
fly deploy --config deploy/fly/discovery.toml
```

Each app gets a `https://<app>.fly.dev` URL immediately. Check them:

```bash
curl https://freedback-demo.fly.dev/.well-known/freedback
curl https://freedback-discovery.fly.dev/.well-known/freedback
```

### Branded subdomains (optional)

Point subdomains of `freedback.net` at the apps and set certs:

```bash
fly certs add demo.freedback.net      --config deploy/fly/feedback.toml
fly certs add discovery.freedback.net --config deploy/fly/discovery.toml
```

`fly certs add` prints the DNS records to create at your registrar (an `A`/`AAAA`
to Fly's anycast IPs, or a `CNAME` to `<app>.fly.dev`). Then set
`FREEDBACK_BASE_URL` in each toml to the `https://…freedback.net` host and
re-`fly deploy` (the base URL is what the servers mint ids/links with).

### Durable feedback storage (optional)

The default image is **in-memory** (ephemeral — fine for a demo; data resets on
restart). For persistence, switch the feedback app to the on-disk RocksDB build:

1. In `deploy/fly/feedback.toml`, uncomment the `[build.args] FEEDBACK_FEATURES =
   "rocksdb"`, the `FREEDBACK_ROCKSDB_PATH = "/data/db"` env, and the `[[mounts]]`
   block.
2. Create the volume once, then deploy:
   ```bash
   fly volumes create freedback_data --size 1 --region cdg --config deploy/fly/feedback.toml
   fly deploy --config deploy/fly/feedback.toml
   ```

(The discovery registry stays in-memory — re-announces are cheap.)

## Hugging Face Docker Space (zero-cost alternative)

Create a **Docker** Space and push this repo's image. Add a Space `README.md`
header:

```yaml
---
title: Freedback demo
sdk: docker
app_port: 8080
---
```

with a `Dockerfile` whose `CMD` is `freedback-feedback-server`. Free CPU; the
Space **sleeps when idle** and storage is **ephemeral**, so use the in-memory
build. Good for a click-to-deploy demo; less suited to durability than Fly.

## Wiring the demo together

Once both are up:

1. The discovery server verifies + lists announced feedback servers
   (`POST /announce`, `GET /servers`, `GET /resolve?target=`).
2. Point the **Pages widget showcase** at the live server instead of its
   in-browser mock by setting the widgets' `data-read` / `data-publish` to the
   feedback/collection URLs (e.g. `https://demo.freedback.net/annotations/`).
   (A small follow-up in `demo-react/` swaps the mock for these URLs.)

## Why not serverless (Workers / Deno Deploy)?

Those run WASM/edge functions; Freedback's wasm is **browser-only**
(`wasm32-unknown-unknown`, not WASI), and the servers are native axum binaries —
so a container host (Fly/HF/Render/Koyeb/Oracle Always-Free) is the fit, not an
edge-function platform.

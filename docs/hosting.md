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

### Automated deployment (GitHub Actions CD)

`.github/workflows/fly-deploy.yml` redeploys **both** apps to Fly on every push
to `main` that touches the server code / build / Fly config (`crates/**`,
`Cargo.*`, `Dockerfile`, `deploy/fly/**`), and on manual **Run workflow**. It
builds on Fly's **remote builders** (no Docker in CI) and **creates the apps on
first run** (idempotent), so you don't need `flyctl` locally at all — the CLI
steps above are only for a one-off manual deploy or local testing.

The job is **guarded**: it runs only when a `FLY_API_TOKEN` repo secret is
present, and is skipped cleanly otherwise (it never fails a push). Turning it on
is **all web UI** — nothing to install:

1. **Fly.io — billing.** Add a payment method (Fly **Dashboard → your account →
   Billing**). The configs scale to zero, so a demo costs ~nothing, but Fly
   requires a card on file.
2. **Fly.io — token.** Create an **account-level** access token at
   **<https://fly.io/user/personal_access_tokens>** ("Create access token") and
   copy it. Use this one, not a per-app *deploy token* — a deploy token is
   scoped to an app that must already exist, which is exactly backwards here
   (the workflow creates the apps itself on first run). Don't use **Apps →
   Launch an App** either; that wizard deploys immediately from its own guess
   at a config and doesn't know about `deploy/fly/*.toml`.
3. **GitHub — secret.** In the repo, **Settings → Secrets and variables →
   Actions → New repository secret**, name it **`FLY_API_TOKEN`**, paste the
   token.
   - *(Optional)* if you deploy under a non-personal Fly org, add a repo
     **variable** `FLY_ORG` with the org slug (defaults to `personal`).

Then push to `main` (or **Actions → Deploy (Fly.io) → Run workflow**) and it
provisions + deploys both apps. The app **names** in the tomls (`freedback-demo`,
`freedback-discovery`) are global on Fly — if either is taken, change the `app =`
line in the toml (and `FREEDBACK_BASE_URL`) before the first run.

### Where the deployed URL shows up

Each successful run records a **GitHub Deployment** (via the job's native
`environment:` block — no extra action or script needed). That surfaces the
live URL in the usual GitHub places:

- The repo's **main page**, right sidebar → **Environments** (`fly-feedback`,
  `fly-discovery`) — each links straight to its `https://<app>.fly.dev` URL.
- The **Environments** tab under repo **Settings**, and the deployment history
  on any commit/PR that triggered a run.

The URL used is always the guaranteed `https://<app>.fly.dev` host (not a
branded subdomain), since that resolves immediately after every deploy even
before `fly certs add` / DNS is set up for a custom domain.

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

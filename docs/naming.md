# Naming & namespace findings

## The `freedback` npm homonym (checked 2026-06-21)

A package named **`freedback`** already exists on npm — in our exact niche.

- **Description:** "A free, self-hosted feedback widget for Next.js apps with
  multiple storage options and AI-powered insights".
- **History:** 59 versions published in a single 3-day burst
  (2025-05-28 → 2025-05-31), latest `0.1.59`; **no activity since** (dormant).
- **Maintainer:** `mrrxwyz <mrrxwyz@gmail.com>` (single).
- **repository.url:** not declared. **homepage:** `https://freedback.dev`.

### Consequences (decided defaults)

1. **npm name `freedback` is taken.** The web widgets (M8) MUST publish under a
   scope or a distinct name. Default choice: **`@freedback/widgets`** (npm
   scopes are independent of the unscoped name). Revisit if the org scope is
   also unavailable.
2. **`freedback.dev` is taken** by the homonym (its `homepage`). Do **not** plan
   docs/demos on `.dev`. The canonical protocol IRIs and the Pages site use
   **`freedback.net`**, which **is owned** as of 2026-06-21 and is now the stable
   base (ADR 0020).
3. Brand-collision risk is low-but-real (same niche, dormant peer). A
   deliberate name decision is owed before any public launch; the protocol-level
   IRIs are the only thing that is expensive to change later (stable-URL policy),
   and those are `freedback.net`, now confirmed and served via GitHub Pages.

### Still to verify
- npm `@freedback` org scope availability.
- npm download stats for the homonym (blocked here: `api.npmjs.org` not in the
  network egress allowlist).

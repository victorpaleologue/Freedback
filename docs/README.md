# Freedback wiki

Design docs for the project, in one place. Start with the white book if you
want the *why*; everything else here is the *how*.

## Start here

- **[The White Book](white-book.md)** — the vision, the strategy, who this is
  for, and the principles that don't bend. Read this first.
- **[Architecture](architecture.md)** — the components, how data flows through
  them, and the two-identity trust model, with a diagram.

## Using Freedback

- **[Widgets in React (and everywhere else)](widgets-react.md)** — the npm
  package, JSX usage, outcome events, a reusable wrapper.
- **[Deployment](deployment.md)** — `docker compose up`, a single container,
  environment variables, storage backends.
- **[Hosting a public demo](hosting.md)** — Fly.io (recommended) and a
  Hugging Face Docker Space alternative.

## Design decisions

- **[Architecture decision records](adr/)** — the non-obvious calls, written
  up as they were made. [`architecture.md`](architecture.md#why-these-choices)
  keeps the running index.
- **[Naming & namespace findings](naming.md)** — why the npm package is
  `@freedback/widgets` and the canonical domain is `freedback.net`.

## Project state

- **[Roadmap & issue map](roadmap.md)** — milestones, what's done, what maps
  to which GitHub issue.
- **[Agent invariants](../CLAUDE.md)** — the rules every contributor, human or
  agent, works under.
- **[Attributions](attributions.md)** — harvested-code and design provenance
  (Mangrove, Hypothesis, the Nostr NIPs).

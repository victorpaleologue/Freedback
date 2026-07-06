# The Freedback docs

So you want the details. Good — this is where Freedback is explained in full:
the vision it has carried since 2014, how the pieces actually fit together,
how to run and build on it, and the reasoning behind every non-obvious call.
Nothing here is a stub. Take the tour.

Not sure where to start? Read [the White Book](white-book.md) for the *why*,
then [the architecture](architecture.md) for the *how* — the rest you can
dip into as you need it.

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
- **[Hosting the servers](hosting.md)** — Fly.io (recommended, with the
  persistent default server) and a Hugging Face Docker Space alternative.

## Design decisions

- **[Architecture decision records](adr/)** — the non-obvious calls, written
  up as they were made: the context, the decision, the alternatives weighed,
  the consequences. [`architecture.md`](architecture.md#why-these-choices)
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

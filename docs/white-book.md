# The Freedback White Book

*The vision this project has carried since 2014 — the introductory document
of this wiki. Everything else in [`docs/`](README.md) is the how; this is the
why.*

## The vision

**Everything deserves feedback — especially when we, the people, are
impacted.**

More than that: anything that affects us *should* offer a channel to hear us.
A product. A law. A website. An app. A talk. But we cannot force every
organization to open a channel, and we refuse to wait for them.

So we make our own.

Freedback lets people carry their own feedback channel. You rate, you comment,
you tag — and you publish to a server *you* choose. Collectors aggregate it
across servers. Organizations can listen if they are wise. Either way, our
voice exists, in the open, owned by us.

## The strategy

Let's be honest: the full vision needs a critical mass of people and
organizations agreeing to use Freedback. Maybe that mass never comes. Fine!

Freedback must be worth using *today*, project by project, case by case. A
widget that drops into any page. A feedback form that exports clean, standard
data. A wire format that existing annotation tools already read. Every use
case has to justify itself now, on its own.

Growth follows the immediate use cases — but never at the vision's expense.
Libraries and applications may move fast. The protocol stays stable: our
JSON-LD context, vocabulary, and validation shapes are published at stable
URLs and must never break once released.

## Who it's for

- **Consumers.** Rate the products you buy, the pages you visit, the apps your
  store won't let you review. If it has a URI — and barcodes resolve to URIs —
  you can give it feedback, no subscription required.
- **Consumer organizations.** Discover public feedback sources, collect from
  them in bulk over plain HTTP, aggregate and analyze. No scraping, no
  gatekeeper's permission.
- **App and service providers.** Collect feedback inside your own app, keep it
  private or publish it — standard tooling instead of yet another one-off
  ratings backend.
- **People who distrust central servers.** There is no mandatory instance. Run
  your own server or pick one you trust; your data stays portable either way.
- **Everyone who wants to own their words.** Sign with your own key or stay
  pseudonymous. Keep one identity across devices and years — or don't. Your
  call, always.

## What Freedback is

Freedback is a specification and a protocol for representing and communicating
feedback about anything that has an identity.

Concretely, today:

- Feedback is a **W3C Web Annotation** (JSON-LD): a target (any URI), a body
  (stars, a scalar rating, a thumb, a comment, a tag), a motivation. Existing
  annotation tooling reads it with zero Freedback-specific code.
- Identity is a **keypair you own** (ECDSA P-256). Your public key is your
  portable name; your signature travels with your feedback and any server can
  verify it — no accounts, no shared secrets.
- Federation happens **at query time**: servers announce themselves to a
  discovery registry, and clients and collection servers find and aggregate
  feedback across all of them. No central authority. Ever.
- It ships as working software: a Rust protocol core that runs native and in
  the browser, feedback/discovery/collection servers, clients, drop-in web
  widgets, a Firefox extension, and an Android-first mobile app.

Freedback is free as in freedom: MIT-licensed, non-contaminating, usable
anywhere. For exactly what's built and how the pieces fit together, see the
[architecture overview](architecture.md); for the how of running or using any
of it, start from the [wiki index](README.md).

## Principles

**As standard as possible.** Before inventing, we look around. We prefer
existing standards, and among standards, the newer the better. That is why
Freedback rides on W3C Web Annotations, JSON-LD, schema.org, SHACL, and RFC
8785 — and why exactly one term in our whole vocabulary is net-new (the humble
thumb rating). The strongest protocol is the one the world already implements.

**Quality.** Unreliable software is unattractive. Testability is a requirement
for every piece of software we ship; every feature and fix must be proven by
automated tests, and CI is the gate. High reliability is a requirement for the
long-term vision — and reliability covers the project, not just the code:
specification, review, documentation, consistency.

**Your feedback is YOURS.** Authorship is ownership. The key that signed an
annotation is the only key that can edit it — and the only key that can delete
it. And deletion is *real*: the content is erased, and only a content-free
tombstone remains so that caches forget too and the erased entry can never be
re-ingested. The right to be forgotten, implemented, not promised.

**Subjects, not surveillance.** Straight from the original 2014 specification,
and still binding: the focus of Freedback is to provide feedback on a
*subject*, not to track user behavior. Queries are by target. We aggregate
opinions about things; we do not build profiles of people. (One narrow,
deliberately understated exception: since an author's identity is an IRI too,
it can itself be a feedback target — see `/author/` in the widgets. It is
opt-in, text-only, and nowhere close to a public score.)

**Using Freedback in Freedback.** We dogfood. The software we ship to collect
feedback should itself collect feedback about the project. Using what we
produce forces us to see it — and to be more critical about it.

## Heritage

Freedback has been brewing since 2014. This white book continues one written
long before the current implementation — the vision, the strategy, the use
cases, and most of these principles come from it, and still set this
project's course. The technology changed (Web Annotations, SHACL, Rust,
WASM); the battle cry did not.

# Using the Freedback widgets in React

The Freedback widgets are **vanilla [custom elements](https://developer.mozilla.org/en-US/docs/Web/API/Web_components/Using_custom_elements)**
(`<freedback-stars>`, `<freedback-thumb>`, `<freedback-scalar>`,
`<freedback-comment>`, `<freedback-tag>`) shipped as a single dependency-free
script, `freedback-widgets.js`. Because they are real DOM elements configured
entirely through `data-*` attributes, **React renders them natively** — React
always forwards `data-*` (and `aria-*`) attributes to the DOM, so for the common
case you can drop the tag straight into JSX with no wrapper or refs. (One
subtlety — a widget reads its `data-*` in `connectedCallback`, so the attributes
must be set *before* the element is attached; plain JSX does this, but a `ref`
that configures the element after mount would be too late. A tiny wrapper makes
this bulletproof — see [A reusable wrapper](#a-reusable-wrapper-optional).)

> **Status:** as of **1.0** the widgets ship as the npm package
> **[`@freedback/widgets`](https://www.npmjs.com/package/@freedback/widgets)** with
> an **ESM build** (side-effect import registers the elements), **bundled
> TypeScript types** (zero-config JSX), and **outcome events**
> (`freedback:published` / `freedback:deleted` / `freedback:error`). The
> dependency-free `<script src>`
> path keeps working unchanged. The "dead-simple"
> `npm add @freedback/widgets` → `import "@freedback/widgets"` → `<freedback-stars/>`
> flow now works. A separate `@freedback/react` wrapper (camelCase props + typed
> callbacks) is the only remaining stretch goal — see
> [API review](#api-review--gaps-to-dead-simple).

---

## TL;DR (the dead-simple path)

1. Install and register the elements once (side-effect import):

   ```sh
   npm add @freedback/widgets
   ```

   ```ts
   // main.tsx (or any module that runs once at startup)
   import "@freedback/widgets";
   ```

2. Drop an element into any `.tsx` — config is all `data-*`, so React just
   renders it, and the **bundled types** make it type-check with no setup:

   ```tsx
   export function ProductRating() {
     return (
       <freedback-stars
         data-target="https://shop.example/product/42"
         data-read="https://collect.example/index"
         data-publish="https://feedback.example/annotations/"
         data-sign=""
       />
     );
   }
   ```

That's it. The widget renders the current aggregate from `data-read`, and
(because `data-sign` is present) lets the visitor publish a self-signed rating to
`data-publish`. To react to the outcome in your app, listen for the
[outcome events](#outcome-events-freedbackpublished--freedbackdeleted--freedbackerror).

> **No build step?** You can still load the canonical script directly — it
> registers the same five elements as a global side effect:
>
> ```html
> <script src="https://freedback.net/widgets/freedback-widgets.js"></script>
> <!-- or a CDN: <script src="https://unpkg.com/@freedback/widgets"></script> -->
> ```

---

## Step by step (Vite + React + TypeScript)

### 1. Install + register the elements

```sh
npm add @freedback/widgets
```

Register the five custom elements once for the whole app with a **side-effect
import** (registration is global and idempotent — do it once, not per component):

```ts
// main.tsx — run once at app startup
import "@freedback/widgets";
```

The package ships both an **ESM build** (what `import` resolves to in a bundler
like Vite/webpack) and a **UMD/IIFE build** (what a `<script src>` loads). If you
prefer no bundler, load the canonical script instead (CDN or vendored copy in
`public/`):

```html
<script src="https://freedback.net/widgets/freedback-widgets.js"></script>
<!-- or pin via the package: https://unpkg.com/@freedback/widgets -->
```

You can also import the helper + identity API from the same package:

```ts
import { jcs, starBody, exportIdentity, rotateIdentity } from "@freedback/widgets";
```

### 2. TypeScript: nothing to do — types are bundled

`@freedback/widgets` ships its own `.d.ts`. It augments both
`React.JSX.IntrinsicElements` (React 19) and the global `JSX` namespace (React
≤ 18), **and** the framework-neutral `HTMLElementTagNameMap`, so all five tags
type-check with **zero consumer setup** — no hand-written shim. (Older releases
told you to add an `src/freedback.d.ts`; that is no longer needed — delete it if
you have one.) The tags also type the `onPublished` / `onDeleted` / `onError`
props for the
[outcome events](#outcome-events-freedbackpublished--freedbackdeleted--freedbackerror), and
`document.querySelector("freedback-stars")` is typed as the element.

### 3. Insert the widget in your `.tsx`

```tsx
function Feedback({ url }: { url: string }) {
  const collect = "https://collect.example/index";
  const publish = "https://feedback.example/annotations/";
  return (
    <section>
      <h3>Rate this</h3>
      <freedback-stars data-target={url} data-read={collect} data-publish={publish} data-sign="" />

      <h3>👍 / 👎</h3>
      <freedback-thumb data-target={url} data-read={collect} data-publish={publish} data-sign="" />

      <h3>Difficulty (0–10)</h3>
      <freedback-scalar
        data-target={url} data-read={collect} data-publish={publish} data-sign=""
        data-worst="0" data-best="10" data-step="1"
      />

      <h3>Comments</h3>
      <freedback-comment data-target={url} data-read={collect} data-publish={publish} data-sign="" />

      <h3>Tags</h3>
      <freedback-tag data-target={url} data-read={collect} data-publish={publish} data-sign="" />
    </section>
  );
}
```

### 4. The attributes

| Attribute | Required | Meaning |
|---|---|---|
| `data-target` | yes | the URI the feedback is *about* (your page/product/item) |
| `data-read` | for display | endpoint that returns aggregates — a collection server's `/index` **or** a feedback server's `/annotations/`. Omit for a write-only widget. |
| `data-publish` | to submit | a feedback server's `/annotations/`. Omit for a read-only widget. |
| `data-sign` | — | **presence** enables self-signed publishing (a per-browser P-256 key in IndexedDB, WebCrypto). Write it as `data-sign=""` in JSX. |
| `data-token` | — | an OAuth bearer for the app-managed identity instead of `data-sign`. `data-sign` wins if both are set. |
| `data-worst` / `data-best` / `data-step` | scalar only | the `<freedback-scalar>` scale. |

### A reusable wrapper (optional)

Plain JSX is fine, but if you use the widgets in many places — or want to be
immune to the `connectedCallback`-before-attributes timing note above — wrap them
in one small component that sets the attributes **while the element is detached**,
then appends it:

```tsx
// FreedbackWidget.tsx
import { useEffect, useRef } from "react";

type Kind = "stars" | "thumb" | "scalar" | "comment" | "tag";

export function FreedbackWidget({ kind, ...data }: { kind: Kind } & Record<`data-${string}`, string>) {
  const host = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = document.createElement(`freedback-${kind}`);
    for (const [k, v] of Object.entries(data)) el.setAttribute(k, v); // set BEFORE connect
    host.current!.replaceChildren(el);                                 // now connectedCallback sees full config
    return () => host.current?.replaceChildren();
  });
  return <div ref={host} />;
}
```

```tsx
<FreedbackWidget kind="stars" data-target={url} data-read={collect} data-publish={publish} data-sign="" />
```

The project's live showcase uses exactly this pattern — see
[`demo-react/src/FreedbackWidget.jsx`](../demo-react/src/FreedbackWidget.jsx),
which renders the **shipped** widgets against an in-browser mock backend (the
demo at `https://freedback.net`).

### 5. Notes & gotchas

- **`data-sign` needs a secure context.** WebCrypto signing requires HTTPS (or
  `localhost`). On `file://` or plain HTTP the widget falls back to read-only.
- **Booleans in JSX.** Write `data-sign=""` (presence). `data-sign={true}` also
  works because the widget checks attribute *presence*, but `=""` is clearest.
- **Reading the result.** Besides updating its own DOM (the aggregate / status
  line), each widget now **dispatches `freedback:published` / `freedback:error`
  events** so your app can react — see
  [Outcome events](#outcome-events-freedbackpublished--freedbackdeleted--freedbackerror).
- **One key per browser.** `data-sign` mints/reuses one identity per browser
  (IndexedDB). The `window.Freedback` global exposes
  `exportIdentity` / `importIdentity` / `rotateIdentity` for backup/rotation
  (issue #27).
- **Delete my feedback (right to erasure).** With `data-sign`, the widgets
  recognise the visitor's **own** annotations in fetched lists and render a
  small `×` control (`.fb-del`): per comment on `<freedback-comment>`, per
  own-tag chip on `<freedback-tag>`, and as a post-publish **undo** next to the
  aggregate on the rating widgets. It signs a delete document with the same
  stored key and `DELETE`s the annotation on the server (ADR 0021); observe the
  outcome via `freedback:deleted` / `freedback:error`.
- **No server yet?** Point `data-read`/`data-publish` at a mock during
  development, or see the live showcase on the site, which fakes the server in
  the browser so the widgets behave realistically.

---

## Outcome events (`freedback:published` / `freedback:deleted` / `freedback:error`)

After a publish or delete, the widget **dispatches a `CustomEvent` on its host
element** — additive to the existing DOM behavior (the aggregate refresh /
`.fb-status` text), so nothing you relied on changes:

| Event | When | `event.detail` |
|---|---|---|
| `freedback:published` | the POST succeeded | `{ response, annotation }` — the parsed server response and the annotation that was sent |
| `freedback:deleted` | a delete succeeded (right to erasure, ADR 0021) | `{ annotation, response }` — the erased annotation's dedup id and the raw `DELETE` response (204) |
| `freedback:error` | the POST or DELETE failed | `{ error }` — the `Error` thrown |

The events **bubble** and are **composed**, so you can also listen on a container.
The bundled types add typed `onPublished` / `onDeleted` / `onError` props and a
typed `addEventListener`.

In React, attach the listeners via a ref in `useEffect` (custom events aren't
React's synthetic `on*` props, so a ref is the reliable way):

```tsx
import { useEffect, useRef } from "react";

function ProductRating({ url }: { url: string }) {
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onPublished = (e: Event) => {
      const { response, annotation } = (e as CustomEvent).detail;
      console.log("thanks! stored:", response, "sent:", annotation);
    };
    const onError = (e: Event) => {
      console.warn("publish failed:", (e as CustomEvent).detail.error);
    };
    el.addEventListener("freedback:published", onPublished);
    el.addEventListener("freedback:error", onError);
    return () => {
      el.removeEventListener("freedback:published", onPublished);
      el.removeEventListener("freedback:error", onError);
    };
  }, []);

  return (
    <freedback-stars
      ref={ref}
      data-target={url}
      data-read="https://collect.example/index"
      data-publish="https://feedback.example/annotations/"
      data-sign=""
    />
  );
}
```

> The `ref` here only **observes** events — it does not configure the element, so
> the `connectedCallback`-before-attributes timing note does not apply (the
> `data-*` are set by JSX before mount). If you also need to set attributes
> imperatively, use the [reusable wrapper](#a-reusable-wrapper-optional), which
> can forward these listeners too.

---

## API review — gaps to "dead simple"

### Done in 1.0

The packaging/ergonomics gaps this tutorial originally surfaced are **resolved**
in the `@freedback/widgets` 1.0 package:

1. **Published npm package.** `@freedback/widgets` (the scope reserved in
   `docs/naming.md`) ships with `main`/`module`/`exports`/`types`, so
   `npm add @freedback/widgets` works. ✅
2. **ESM build + side-effect registration.** `import "@freedback/widgets"`
   registers the elements; named imports expose the helper + identity API. The
   IIFE/UMD build still powers the `<script src>` path unchanged. ✅
3. **Bundled TypeScript types.** A shipped `.d.ts` augments
   `React.JSX.IntrinsicElements` (React 19), the global `JSX` namespace
   (React ≤ 18), and `HTMLElementTagNameMap` — zero consumer setup. ✅
4. **Outcome events.** `freedback:published` / `freedback:error` are dispatched
   on each widget (see
   [Outcome events](#outcome-events-freedbackpublished--freedbackdeleted--freedbackerror)). ✅

### Still future work

5. **Optional thin React wrapper.** A separate **`@freedback/react`** package
   exposing `<FreedbackStars target=… read=… publish=… sign onPublished=… />`
   with real camelCase props + typed callbacks would be the most idiomatic React
   surface and would hide the custom-element/`data-*` details entirely. The
   custom-element + events surface in 1.0 already makes such a wrapper a thin
   layer; it is deferred, not required.

6. **Minor naming.** `data-read` is overloaded (it accepts either a collection
   `/index` or a feedback `/annotations/`), and read vs. publish are two separate
   URLs. Clearer names (e.g. `data-aggregate` / `data-source`) or a single
   `data-server` convention with derived paths could be cut in a future major.

**Bottom line:** the dead-simple ideal — `npm add @freedback/widgets`, `import`,
drop the tag into `.tsx`, observe the events — works as of 1.0. The only
remaining stretch goal is the optional `@freedback/react` wrapper.

# Using the Freedback widgets in React

The Freedback widgets are **vanilla [custom elements](https://developer.mozilla.org/en-US/docs/Web/API/Web_components/Using_custom_elements)**
(`<freedback-stars>`, `<freedback-thumb>`, `<freedback-scalar>`,
`<freedback-comment>`, `<freedback-tag>`) shipped as a single dependency-free
script, `freedback-widgets.js`. Because they are real DOM elements configured
entirely through `data-*` attributes, **React renders them natively** — React
always forwards `data-*` (and `aria-*`) attributes to the DOM, so you do not
need a wrapper component or refs to use them.

> **Status (be aware):** the widgets are **not yet published to npm**, and the
> script registers the elements as a *side effect* rather than exporting them as
> ES modules, and it ships **no TypeScript types**. So the truly "dead-simple"
> `npm add @freedback/widgets` → `import` → `<freedback-stars/>` flow is **not
> there yet** — see [API review](#api-review--gaps-to-dead-simple) below for the
> exact gaps and the plan to close them. This page documents the **best way that
> works today** plus where we're headed.

---

## TL;DR (what works today)

1. Load the script once (registers the five elements):

   ```html
   <!-- index.html -->
   <script src="https://freedback.net/widgets/freedback-widgets.js"></script>
   ```

2. Drop an element into any `.tsx` — config is all `data-*`, so React just
   renders it:

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

3. (TypeScript only) add a one-time JSX declaration so `<freedback-stars>` type-checks — [snippet below](#typescript-make-the-tags-type-check).

That's it for behavior. The widget renders the current aggregate from
`data-read`, and (because `data-sign` is present) lets the visitor publish a
self-signed rating to `data-publish`.

---

## Step by step (Vite + React + TypeScript)

### 1. Get the script into your app

Until there's an npm package, pick one of:

- **From the CDN (simplest):** the `<script>` tag above, pointing at
  `https://freedback.net/widgets/freedback-widgets.js`.
- **Vendored (pin the version, works offline):** copy `freedback-widgets.js`
  into your app's `public/` and load `/freedback-widgets.js`.
- **As a module side-effect import** (so it's part of your bundle graph): import
  the file for its registration side effect. With Vite, the robust way is to
  grab its URL and inject it once, because the file is an IIFE (not ESM):

  ```ts
  // freedback.ts — run once at app startup
  import widgetUrl from "./vendor/freedback-widgets.js?url";

  export function loadFreedbackWidgets() {
    if (document.querySelector("script[data-freedback]")) return;
    const s = document.createElement("script");
    s.src = widgetUrl;
    s.dataset.freedback = "";
    document.head.appendChild(s);
  }
  ```

  ```tsx
  // main.tsx
  import { loadFreedbackWidgets } from "./freedback";
  loadFreedbackWidgets();
  ```

  > A bare `import "./vendor/freedback-widgets.js"` *can* work in a browser
  > bundle, but the file currently also has a CommonJS `module.exports` branch,
  > so the `?url`-and-inject approach above is the predictable one. This wart
  > goes away once we ship an ESM build (see the review).

Custom-element registration is global and idempotent — load the script **once**
for the whole app, not per component.

### 2. TypeScript: make the tags type-check

The script ships no types, so add this once (e.g. `src/freedback.d.ts`). The
widgets only take `data-*` attributes, so the typing is small:

```ts
// src/freedback.d.ts
import type React from "react";

type FreedbackProps = React.HTMLAttributes<HTMLElement> & {
  "data-target": string;
  "data-read"?: string;
  "data-publish"?: string;
  "data-token"?: string;
  "data-sign"?: "";
  // <freedback-scalar> scale:
  "data-worst"?: string | number;
  "data-best"?: string | number;
  "data-step"?: string | number;
};

declare module "react" {
  namespace JSX {
    interface IntrinsicElements {
      "freedback-stars": FreedbackProps;
      "freedback-thumb": FreedbackProps;
      "freedback-scalar": FreedbackProps;
      "freedback-comment": FreedbackProps;
      "freedback-tag": FreedbackProps;
    }
  }
}
```

> On **React 19** use `declare module "react"` (above). On **React ≤ 18** use the
> global form instead: `declare global { namespace JSX { interface
> IntrinsicElements { … } } }`.

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

### 5. Notes & gotchas

- **`data-sign` needs a secure context.** WebCrypto signing requires HTTPS (or
  `localhost`). On `file://` or plain HTTP the widget falls back to read-only.
- **Booleans in JSX.** Write `data-sign=""` (presence). `data-sign={true}` also
  works because the widget checks attribute *presence*, but `=""` is clearest.
- **Reading the result.** Today the widget shows the outcome only inside its own
  DOM (the aggregate updates; errors appear in its status line). There is **no
  React callback / event** yet — see the review.
- **One key per browser.** `data-sign` mints/reuses one identity per browser
  (IndexedDB). The `window.Freedback` global exposes
  `exportIdentity` / `importIdentity` / `rotateIdentity` for backup/rotation
  (issue #27).
- **No server yet?** Point `data-read`/`data-publish` at a mock during
  development, or see the live showcase on the site, which fakes the server in
  the browser so the widgets behave realistically.

---

## API review — gaps to "dead simple"

Writing this tutorial surfaced the friction between today's widget and the goal
("`npm add`, import, drop into `.tsx`"). None of these are behavior bugs — the
widgets work — they're **packaging/ergonomics** gaps. In rough priority:

1. **No published npm package.** `widgets/package.json` is `private: true` and is
   the e2e harness, not a distributable. There is no `main`/`module`/`exports`/
   `types`. **Fix:** publish **`@freedback/widgets`** (the scope reserved in
   `docs/naming.md`) with proper `exports` + `types`, so `npm add @freedback/widgets`
   works. This is the single biggest step to "dead simple".

2. **Not an ES module; side-effect-only registration.** The file is an IIFE that
   calls `customElements.define` as a side effect; its `module.exports` exposes
   only the *pure helpers*, not the element classes, and there is no ESM `export`.
   So you can't `import "@freedback/widgets"` cleanly in a bundler. **Fix:** ship
   an **ESM build** with a side-effect entry (`import "@freedback/widgets"`
   registers the elements) — keep the IIFE/UMD build for `<script>` users.

3. **No TypeScript types shipped.** Consumers must hand-write the JSX
   augmentation in step 2. **Fix:** ship a `.d.ts` that augments
   `React.JSX.IntrinsicElements` (and a framework-neutral
   `HTMLElementTagNameMap`) so TS is zero-config.

4. **No outcome events.** The widgets surface publish success/failure only as
   their own DOM text (`.fb-agg` / `.fb-status`); a host app can't observe it.
   **Fix:** dispatch `freedback:published` and `freedback:error` `CustomEvent`s
   (detail = the annotation / error) so React can do
   `<freedback-stars onPublished={…}>`-style handling (via a ref or a thin
   wrapper). This is the main *functional* ergonomics gap.

5. **Optional thin React wrapper.** Even with the above, a `@freedback/react`
   package exposing `<FreedbackStars target=… read=… publish=… sign />` with
   real props + `onPublished`/`onError` callbacks would be the most idiomatic
   React surface (camelCase props, typed events) and would hide the
   custom-element/`data-*` details entirely.

6. **Minor naming.** `data-read` is overloaded (it accepts either a collection
   `/index` or a feedback `/annotations/`), and read vs. publish are two separate
   URLs. Consider clearer names (e.g. `data-aggregate` / `data-source`) or a
   single `data-server` convention with derived paths, when the package API is
   cut.

**Bottom line:** the *runtime* is already React-friendly (config is `data-*`, so
no wrappers/refs needed). Reaching the dead-simple ideal is mostly a
**distribution** task — publish `@freedback/widgets` (ESM + `.d.ts`), add the two
outcome events, and (optionally) a `@freedback/react` wrapper. Until then, the
CDN-script + types-shim recipe above is the supported path.

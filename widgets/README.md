# @freedback/widgets

Drop-in **feedback widgets** for [Freedback](https://freedback.net/) — a
federated, open feedback protocol whose native wire format is W3C Web Annotation
JSON-LD. The widgets are dependency-free **custom elements**: `<freedback-stars>`,
`<freedback-thumb>`, `<freedback-scalar>`, `<freedback-comment>`,
`<freedback-issue>`, and `<freedback-tag>`. They work in any framework (or
plain HTML), configured entirely through `data-*` attributes.

## Install

```sh
npm add @freedback/widgets
```

```ts
// Register the six custom elements (side-effect import). Do this once.
import "@freedback/widgets";
```

TypeScript types are **bundled** — `<freedback-stars …>` type-checks in JSX
(React 19 and React ≤ 18) and in framework-neutral DOM code with zero setup.

The helper + identity API is also exported:

```ts
import { jcs, starBody, exportIdentity } from "@freedback/widgets";
```

## No build step? Use a `<script>`

The canonical source is a single dependency-free IIFE; loading it registers the
elements globally with no bundler:

```html
<script src="https://freedback.net/widgets/freedback-widgets.js"></script>
<!-- or from a CDN: -->
<script src="https://unpkg.com/@freedback/widgets"></script>
```

## Usage

```html
<freedback-stars
  data-target="https://shop.example/product/42"
  data-read="https://collect.example/index"
  data-publish="https://feedback.example/annotations/"
  data-sign></freedback-stars>
```

| Attribute | Meaning |
|---|---|
| `data-target` | the URI the feedback is *about* (required) |
| `data-read` | aggregate read endpoint (collection `/index` or feedback `/annotations/`) |
| `data-publish` | feedback server `/annotations/` to submit to |
| `data-sign` | presence enables self-signed publishing (per-browser P-256 key, WebCrypto) |
| `data-token` | an OAuth bearer instead of `data-sign` (`data-sign` wins if both) |
| `data-worst` / `data-best` / `data-step` | `<freedback-scalar>` scale |

`<freedback-issue>` is the problem-report widget (the third feedback kind of
the original 2014 proto): a textarea plus a **Report** button, listing the
issues reported for the target. On the wire it is a plain W3C
`oa:TextualBody` under the **standard `oa:editing` motivation** ("request a
change or edit to the Target resource") — zero new vocabulary (ADR 0023).

## Outcome events

After a publish or delete, each widget dispatches a `CustomEvent` on its host
element:

- `freedback:published` — `detail = { response, annotation }` on success
- `freedback:deleted` — `detail = { annotation, response }` after a successful
  erasure (`annotation` is the erased dedup id)
- `freedback:error` — `detail = { error }` on a failed publish or delete

```ts
el.addEventListener("freedback:published", (e) => console.log(e.detail.response));
```

## Delete my feedback (right to erasure)

With `data-sign`, the widgets recognise the visitor's **own** annotations in
fetched lists (their `creator.id` matches the browser identity) and render a
small `×` control (`.fb-del`, `aria-label="Delete my feedback"`): per item on
`<freedback-comment>` and `<freedback-issue>`, per own-tag chip on
`<freedback-tag>`, and as a
post-publish **undo** next to the aggregate on the rating widgets. Clicking it
signs a delete document with the same stored P-256 key that signed the
annotation and `DELETE`s it on the feedback server (ADR 0021) — the server
erases the record and keeps only a content-free tombstone.

See [docs/widgets-react.md](https://github.com/freedback/freedback/blob/main/docs/widgets-react.md)
for the full React guide (including a `useEffect`+ref event example).

## License

Apache-2.0

# @freedback/widgets

Drop-in **feedback widgets** for [Freedback](https://freedback.net/) — a
federated, open feedback protocol whose native wire format is W3C Web Annotation
JSON-LD. The widgets are dependency-free **custom elements**: `<freedback-stars>`,
`<freedback-thumb>`, `<freedback-scalar>`, `<freedback-comment>`, and
`<freedback-tag>`. They work in any framework (or plain HTML), configured
entirely through `data-*` attributes.

## Install

```sh
npm add @freedback/widgets
```

```ts
// Register the five custom elements (side-effect import). Do this once.
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

## Outcome events

After a publish, each widget dispatches a `CustomEvent` on its host element:

- `freedback:published` — `detail = { response, annotation }` on success
- `freedback:error` — `detail = { error }` on failure

```ts
el.addEventListener("freedback:published", (e) => console.log(e.detail.response));
```

See [docs/widgets-react.md](https://github.com/freedback/freedback/blob/main/docs/widgets-react.md)
for the full React guide (including a `useEffect`+ref event example).

## License

Apache-2.0

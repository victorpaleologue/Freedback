# ADR 0009 — Custom rating scales via `sh:lessThanOrEquals`

- **Status:** accepted
- **Date:** 2026-06-21
- **Refines:** ADR 0004 (all validation in SHACL).

## Context

ADR 0004 validated ratings against **fixed** scales (stars 1..5, scalar 0..1,
thumb {0,1}) and noted that validating a value against the body's *own*
`worstRating`/`bestRating` "needs a sibling-property comparison that SHACL Core
cannot express." That was wrong: SHACL Core **does** have property-pair
constraints — `sh:lessThanOrEquals` / `sh:lessThan` compare the values of the
shape's path against the values of another property on the same focus node. No
SHACL-SPARQL required.

## Decision

`freedback:ScalarRating` carries its own scale, so validate it relative to that
scale:

```
worstRating ≤ ratingValue ≤ bestRating
```

expressed in `shapes.ttl` as two `sh:lessThanOrEquals` property shapes
(`worstRating ≤ ratingValue` and `ratingValue ≤ bestRating`), plus the datatype
and required-ness checks. The default scalar scale stays 0..1 (the body's default
bounds), but **any `worst < best` is accepted** — a 0..10 scalar with value 7
now validates.

`StarRating` (1..5) and `ThumbRating` ({0,1}) keep their fixed canonical scales —
they are not free-scale by design — so this change is scoped to scalar ratings.

The in-house validator gained a `sh:lessThanOrEquals` evaluator (a few lines): it
compares the path's numeric values to the referenced sibling property's values on
the same focus node, exactly per the SHACL Core semantics.

## Why this is still "validation lives in SHACL"

The rule is expressed entirely in `shapes.ttl` with a standard SHACL Core
constraint component; the validator just interprets it. Swapping in a full
external SHACL engine later would interpret the same shapes identically.

## Consequences

- Custom-scaled scalar feedback is now first-class and validated against its own
  declared bounds (tested: 0..10 in-range conforms; above `bestRating` / below
  `worstRating` are rejected with targeted messages).
- The validator now supports `sh:lessThanOrEquals`, reusable for any future
  property-pair constraint.
- Corrects the "SHACL Core can't do this" claim in ADR 0004 / issue #13.

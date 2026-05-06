# Validation

> Placeholder chapter. See [SPEC §7](../reference/spec.md) for the
> canonical validation bridge. **This is a Phase 4 deliverable** on the
> [roadmap](../reference/roadmap.md); none of it exists yet.

## The bridge, in one paragraph

`#[derive(Validate)]` on an input type records a per-field constraint
description into the IR. Codegen then emits a matching client-side
schema — Valibot by default, Zod via a CLI flag. The generated client
validates inputs *before* sending and outputs *after* receiving, so a
constraint added on the Rust side surfaces as a TypeScript build error
when downstream callers can't satisfy it. Validation can be disabled
per-call for hot paths.

## What ships in 0.1

The 0.1 constraint set is intentionally small and aligned with the
intersection of what Valibot and Zod express natively:

- `min`, `max` — numeric bounds.
- `length` — string and collection length.
- `pattern` — regex.
- `email`, `url` — common string formats.

Custom predicates (anything not in the list above) are recorded as
opaque tags in the IR. The user has to supply a hand-written schema
fragment that matches the tag; codegen splices it in. This is
deliberate: arbitrary-predicate schemas would force the generator to
understand semantics it can't actually evaluate.

## Why Valibot is the default

Valibot's tree-shakeable, function-per-rule shape produces smaller
bundles for the typical case (a handful of validated inputs per page),
and its type inference is stable on union-of-discriminant inputs. Zod is
more widely known and gets first-class CLI support, but the default
weights bundle size and ergonomic narrowing.

## What this chapter will cover when written

- Walkthrough of `#[derive(Validate)]` on a struct with a mix of
  `min`/`max`, `pattern`, and a custom predicate.
- The IR shape for constraints, and how the generator turns each
  constraint kind into a Valibot or Zod schema fragment.
- Per-call toggles for skipping pre-send and post-receive validation.
- Failure-mode story: what the client throws when validation fails, and
  how that interacts with the typed error union from the procedure.

## See also

- [SPEC §7 — Validation bridge](../reference/spec.md)
- [Roadmap — Phase 4](../reference/roadmap.md)
- [Type mapping](./type-mapping.md)

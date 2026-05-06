# Errors

> Placeholder chapter. See [SPEC §3.3](../reference/spec.md) for the
> canonical error model. Per-procedure error narrowing is a Phase 2
> deliverable on the [roadmap](../reference/roadmap.md).

## The model

A `#[rpc]` function returns `Result<T, E>` where `E: TautError`. The
`TautError` trait demands two things: a `code: &'static str` discriminant
and a `Serialize` payload. On the TypeScript side, every error becomes a
value of:

```ts
type ApiError<C extends string, P> = { code: C; payload: P };
```

Critically, the union is *narrowed per procedure*. A procedure that
returns `Result<User, GetUserError>` where `GetUserError` has only
`NotFound` and `Forbidden` variants produces a TS error type with
exactly those two `code` values — not the global universe of error codes
the rest of the API might emit.

## Switching on `err.code`

The whole point of typed errors is that the TS compiler can narrow the
payload type from the discriminant. The pattern looks like this:

```ts
const r = await client.getUser({ id: 1 });

if ("err" in r) {
  switch (r.err.code) {
    case "not_found":
      // r.err.payload is { id: number }
      console.warn(`no user ${r.err.payload.id}`);
      break;
    case "forbidden":
      // r.err.payload is { reason: string }
      console.warn(`denied: ${r.err.payload.reason}`);
      break;
    // No default needed: TS proves the switch is exhaustive.
  }
} else {
  // r.ok is User
  console.log(r.ok.name);
}
```

If a new error variant is added on the Rust side, the TypeScript build
fails at every call site that switches on `code` — a refactor signal,
which is the whole reason for typing the errors in the first place.

## Standard codes (under discussion)

SPEC §8 raises an open question: should the project ship a small set of
"standard" error discriminants by convention — e.g. `unauthenticated`,
`unauthorized`, `internal` — so middleware can produce them without
reaching for a project-defined error type? The current lean is yes;
final shape is deferred to Phase 2.

## See also

- [SPEC §3.3 — Errors](../reference/spec.md)
- [SPEC §8 — Open questions](../reference/spec.md)
- [Authentication guide](../guides/auth.md)

# Error codes

This page is the canonical reference for every error `code` that taut-rpc
emits on the wire. If your client receives an error envelope, the `code` field
will be one of the values listed below (or a user-defined code, see the second
section).

The wire envelope is always:

```json
{
  "ok": false,
  "error": {
    "code": "<string>",
    "payload": <code-specific-shape>
  }
}
```

The HTTP status column shows the response status the server returns for that
code. Clients should not rely on the status alone; the `code` field is the
authoritative discriminant.

## Built-in codes

These codes are emitted by the runtime itself or by the `StandardError`
helper. They are stable across taut-rpc versions and form part of the public
contract.

| Code | HTTP | Payload | Source |
|---|---|---|---|
| `decode_error` | 400 | `{ message: string }` | Server: invalid JSON in `{input}` body |
| `validation_error` | 400 | `{ errors: [{path, constraint, message}] }` | Server: `<I as Validate>::validate` failure (or client-side schema parse) |
| `not_found` | 404 | `{ procedure: string }` | Server: hit `/rpc/<unknown>` |
| `unauthenticated` | 401 | `null` | `StandardError::Unauthenticated` |
| `forbidden` | 403 | `{ reason: string }` | `StandardError::Forbidden` |
| `rate_limited` | 429 | `{ retry_after_seconds: u32 }` | `StandardError::RateLimited` |
| `internal` | 500 | `null` | `StandardError::Internal` |
| `bad_request` | 400 | `{ message: string }` | `StandardError::BadRequest` |
| `conflict` | 409 | `{ message: string }` | `StandardError::Conflict` |
| `unprocessable_entity` | 422 | `{ message: string }` | `StandardError::UnprocessableEntity` |
| `service_unavailable` | 503 | `{ retry_after_seconds: u32 }` | `StandardError::ServiceUnavailable` |
| `timeout` | 504 | `null` | `StandardError::Timeout` |
| `serialization_error` | 500 | `{ message: string }` | Server: serde failed serializing the response (rare) |
| `transport_error` | 0 | `{ message: string }` | Client-side (npm runtime): network error before reaching the server |

A few notes:

- `decode_error` only fires when the request body is not valid JSON or does
  not match the `{input}` envelope. Type-level mismatches between the JSON
  and `I` (the procedure's input type) surface as `validation_error` once
  `Validate` runs.
- `validation_error.payload.errors` is always an array, even when only one
  field failed. Each entry has a JSON-pointer-style `path` (e.g.
  `"/email"`), a machine-readable `constraint` tag (e.g. `"email"`,
  `"min_length"`), and a human-readable `message`.
- `not_found` is reserved for the *router* not finding the procedure. If
  your handler wants to signal "the requested resource doesn't exist", use a
  user-defined code or `StandardError::BadRequest` with an explanatory
  message.
- `transport_error` is never sent over the wire — the npm runtime
  synthesizes it locally when `fetch` itself rejects (offline, DNS failure,
  CORS preflight blocked, etc.). Its HTTP status is reported as `0` for
  symmetry with browser conventions.
- `serialization_error` should be impossible in practice: `O` is required to
  implement `Serialize` infallibly. If you ever see one in the wild, it
  almost certainly indicates a custom `Serialize` impl that panics or
  returns an error — please file a bug against the offending crate.

## User-defined error codes

When you derive `TautError` on your own enum, each variant becomes a code.

```rust
#[derive(taut_rpc::TautError)]
pub enum BillingError {
    CardDeclined,
    #[taut(code = "insufficient_funds", status = 402)]
    NotEnoughBalance { available_cents: u64 },
    #[taut(status = 410)]
    SubscriptionExpired,
}
```

The defaults:

- **Code**: the variant name in `snake_case`. `CardDeclined` ->
  `card_declined`, `SubscriptionExpired` -> `subscription_expired`.
- **HTTP status**: `400 Bad Request`.
- **Payload**: the variant's fields, serialized as a JSON object (or `null`
  for unit variants, or a single value for newtype variants — same rules as
  serde's default enum representation, minus the outer tag).

You override either with `#[taut(...)]`:

| Attribute | Effect |
|---|---|
| `#[taut(code = "snake_name")]` | Replace the auto-derived code. Must be globally unique within your error enum. |
| `#[taut(status = 402)]` | Override HTTP status. Must be a valid 4xx/5xx code. |

In the example above, the wire codes would be `card_declined` (400),
`insufficient_funds` (402), and `subscription_expired` (410).

A user-defined code MUST NOT collide with a built-in code. The derive macro
emits a compile error if you try.

## How to handle errors in the client

The npm runtime ships with a type-safe `isTautError` guard. It narrows both
the code and the payload, so you get full TypeScript inference inside each
branch.

```ts
import { isTautError } from "taut-rpc";

try {
  await client.users.create({ email: "not-an-email" });
} catch (e) {
  if (isTautError(e, "validation_error")) {
    // e.payload.errors is fully typed as ValidationIssue[]
    for (const issue of e.payload.errors) {
      showFieldError(issue.path, issue.message);
    }
  } else if (isTautError(e, "unauthenticated")) {
    redirectToLogin();
  } else if (isTautError(e, "rate_limited")) {
    toast(`Try again in ${e.payload.retry_after_seconds}s`);
  } else {
    throw e; // unknown error — propagate
  }
}
```

The single-argument form `isTautError(e)` returns `true` for any taut error
(any wire code, plus `transport_error`), which is useful when you only want
to distinguish "the server replied with an error" from "the request itself
threw" — but in practice, narrowing on the specific code is almost always
what you want.

For user-defined codes, the generated client exports them on a
per-procedure basis:

```ts
import { isTautError } from "taut-rpc";
import type { BillingError } from "./generated/errors";

try {
  await client.billing.charge({ cents: 500 });
} catch (e) {
  if (isTautError<BillingError, "insufficient_funds">(e, "insufficient_funds")) {
    toast(`You need ${e.payload.available_cents / 100} more dollars`);
  } else throw e;
}
```

See the [Errors concept page](../concepts/errors.md) for the full design
rationale and the [Migrating from tRPC](../guides/migrate-from-trpc.md)
guide for a comparison with `TRPCError`.

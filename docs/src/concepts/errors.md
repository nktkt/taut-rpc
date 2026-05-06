# Errors

Phase 2 of `taut-rpc` ships the typed-error story: a `TautError` trait,
a derive macro to generate it from an `enum`, a built-in `StandardError`
type, and TS-side helpers that narrow `catch` blocks by error code.
This chapter walks through the model end-to-end.

## The `TautError` trait

Every error type returned from an `#[rpc]` function must implement
`TautError`. The trait has two methods:

```rust
pub trait TautError: serde::Serialize {
    fn code(&self) -> &'static str;
    fn http_status(&self) -> u16 { 400 }
}
```

These two methods feed the wire envelope directly:

- `code()` becomes the `code` field in the response body.
- `http_status()` becomes the HTTP status of the response. The default is
  `400` — application errors are client errors unless you say otherwise.

Anything that implements `TautError` is also `Serialize`, and the
serialized form is what lands in the `payload` slot of the envelope.

## Wire envelope

Per [SPEC §4.1](../reference/spec.md), every error — application,
decode-failure, or unknown-procedure — comes back in the same shape:

```json
{
  "err": {
    "code": "<discriminant>",
    "payload": <error-specific JSON>
  }
}
```

One parser path on the client; the `code` discriminant tells you what
the `payload` is. Success responses are `{"ok": <Output>}`, never mixed
with `err`.

## `#[derive(TautError)]`

Hand-implementing `TautError` is fine for one-off types. For typical
error enums, use the derive macro:

```rust
use taut_rpc::TautError;
use serde::Serialize;

#[derive(Debug, Serialize, TautError)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum AddError {
    Overflow,
    ZeroDivisor,
}

#[taut_rpc::rpc]
async fn add(input: AddInput) -> Result<i32, AddError> {
    input.a.checked_add(input.b).ok_or(AddError::Overflow)
}
```

### Default behaviour

- The variant name is converted to `snake_case` and used as `code()`.
  `Overflow` → `"overflow"`, `ZeroDivisor` → `"zero_divisor"`.
- `http_status()` returns `400` for every variant.

### Per-variant attributes

Override either default with `#[taut(...)]`:

```rust
#[derive(Debug, Serialize, TautError)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum BillingError {
    #[taut(code = "card_declined", status = 402)]
    CardDeclined { reason: String },

    #[taut(status = 409)]
    DuplicateCharge { idempotency_key: String },
}
```

`CardDeclined` reports `code = "card_declined"` and HTTP 402.
`DuplicateCharge` keeps the auto-generated `"duplicate_charge"` code but
returns HTTP 409.

### Why the `#[serde(...)]` recommendation?

`TautError::code()` controls the wire envelope's `code` field. The
*payload* slot is whatever `Serialize` produces. Pairing the derive with

```rust
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
```

makes the serde output line up with the wire shape directly: serde puts
the variant tag in `code` and the variant data in `payload`, matching
`{"code": "...", "payload": ...}` exactly. Without it you'd get serde's
default externally-tagged shape, and the envelope would carry duplicate
or mismatched discriminants.

## `StandardError`

`taut-rpc` ships a built-in error type for the boring cases:

| Variant | `code()` | `http_status()` |
|---|---|---|
| `Unauthenticated` | `"unauthenticated"` | 401 |
| `Forbidden` | `"forbidden"` | 403 |
| `NotFound` | `"not_found"` | 404 |
| `RateLimited` | `"rate_limited"` | 429 |
| `Internal` | `"internal"` | 500 |

Use it directly when a procedure has no domain-specific failure modes:

```rust
use taut_rpc::{rpc, StandardError};

#[rpc]
async fn whoami(token: Token) -> Result<User, StandardError> {
    lookup(token).ok_or(StandardError::Unauthenticated)
}
```

…or wrap it in a domain enum when you need both:

```rust
#[derive(Debug, Serialize, TautError)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum GetUserError {
    NotFound { id: u64 },
    #[taut(status = 401)]
    Unauthenticated,
}
```

The derive doesn't require you to delegate to `StandardError`; you can
just mirror its codes in your own enum if that's clearer.

## Narrowing on the TS side

The codegen emits a per-procedure error alias for each `#[rpc]` function
that returns `Result<_, E>`:

```ts
export type Proc_add_Error = AddError;   // = { code: "overflow", ... }
                                         //  | { code: "zero_divisor", ... }
```

The runtime client throws on error responses. The thrown value carries
the `code` and `payload` from the envelope, and three helpers in the
`taut-rpc` runtime narrow it for you:

```ts
import { isTautError, assertTautError, errorMatch } from "taut-rpc/client";

try {
  const sum = await client.add({ a, b });
} catch (e) {
  if (isTautError(e, "overflow")) {
    // e.code is "overflow", e.payload narrowed to the Overflow variant
    return Number.MAX_SAFE_INTEGER;
  }
  throw e;
}
```

`assertTautError(e, "overflow")` does the same as a type predicate but
throws if the code doesn't match — convenient for tests. `errorMatch` is
a small dispatcher for switching on multiple known codes:

```ts
errorMatch(e, {
  overflow: () => /* ... */,
  zero_divisor: () => /* ... */,
});
```

If the Rust error enum gains a variant, the TS build fails everywhere
the dispatcher is non-exhaustive — the same refactor signal as switching
on a discriminated union directly.

## SPEC quirks worth knowing

- **Decode failures.** When axum's `Json` extractor rejects a malformed
  body, `taut-rpc` catches the rejection and emits the SPEC envelope
  with `code = "decode_error"` and HTTP 400. Same parser path on the
  client; it's not a special-cased transport error.
- **Unknown procedures.** A POST to `/rpc/<name>` where `<name>` isn't
  registered returns `{"err": {"code": "not_found", "payload":
  {"procedure": "<name>"}}}` with HTTP 404 — again, same envelope.
- **Per-code IR entries are Phase 4.** The IR records each procedure's
  error *type* (e.g. `AddError`), not a list of `(code, payload)` pairs
  expanded out. That expansion — which would let the TS side type
  `Proc_add_Error` as a literal discriminated union with one branch per
  code — is on the Phase 4 list. For Phase 2, narrowing relies on the
  user's `#[serde(tag = "code", content = "payload")]` enum being its
  own discriminated union, plus the runtime `isTautError`/`errorMatch`
  helpers reading the envelope's `code` field.

## See also

- [SPEC §3.3 — Errors](../reference/spec.md)
- [SPEC §4.1 — Wire format](../reference/spec.md)
- [Authentication guide](../guides/auth.md)
- [Middleware guide](../guides/middleware.md)

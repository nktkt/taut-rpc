# Troubleshooting

A field guide to the errors you are most likely to encounter when
building with taut-rpc, with the symptom you'll see, the underlying
cause, and the fix. If your problem isn't listed here, file an issue
with the exact compiler output and the offending struct definition.

## 1. `the trait `Validate` is not implemented for `MyInput``

**Symptom.** `rustc` rejects a procedure signature with a trait-bound
error pointing at an input type:

```text
error[E0277]: the trait bound `MyInput: Validate` is not satisfied
  --> src/main.rs:42:18
   |
42 | async fn create(input: MyInput) -> Result<Out, MyErr> { ... }
   |                  ^^^^^ the trait `Validate` is not implemented for `MyInput`
```

**Cause.** Every input struct used in an `#[rpc]` procedure must
participate in the validation pipeline, even if it carries no
constraints. The `#[rpc]` macro emits a `MyInput::__taut_validate(...)`
call into the generated wrapper.

**Fix.** Add the derive:

```rust
#[derive(Serialize, Deserialize, Type, Validate)]
pub struct MyInput { /* ... */ }
```

If you genuinely have nothing to validate, the derive expands to a
no-op `validate` impl — there is no runtime cost.

## 2. `the trait `TautError` is not implemented for `MyErr``

**Symptom.**

```text
error[E0277]: the trait bound `MyErr: TautError` is not satisfied
```

…on the `Result<_, MyErr>` return type of an `#[rpc]` procedure.

**Cause.** Procedure error types must implement `TautError` so the
runtime can extract a stable error `code`, an HTTP `status`, and a
typed `payload` for the wire format (see
[Errors (concepts)](../concepts/errors.md)).

**Fix.**

```rust
#[derive(Serialize, Type, taut_rpc::TautError)]
pub enum MyErr {
    #[taut(code = "not_found", status = 404)]
    NotFound,
    #[taut(code = "validation_error", status = 422)]
    Validation { errors: Vec<FieldError> },
}
```

## 3. `unknown taut attribute key`

**Symptom.** A derive macro panics during expansion:

```text
error: unknown taut attribute key `min` on enum variant
```

**Cause.** The three derives (`Type`, `Validate`, `TautError`) each
own a disjoint slice of the `#[taut(...)]` namespace. Putting a key on
the wrong derive trips the unknown-key guard.

| Key                    | Owner       |
| ---------------------- | ----------- |
| `rename`, `tag`        | `Type`      |
| `min`, `max`, `length` | `Validate`  |
| `code`, `status`       | `TautError` |

**Fix.** Move the attribute to the field/variant of the type that
carries the matching derive. If you need a constraint on a struct
field, the struct must `#[derive(Validate)]`. If you need a `code` on
a variant, the enum must `#[derive(TautError)]`.

## 4. `IR schema version mismatch: expected 1, got 0`

**Symptom.** `cargo taut gen` aborts immediately:

```text
error: IR schema version mismatch: expected 1, got 0
```

**Cause.** You are pointing the codegen at a stale `ir.json` produced
by an older `taut-rpc` version. The schema version number is bumped
on every breaking IR change.

**Fix.** Re-run the server (or its dump-mode binary) to regenerate the
IR with the current crate version:

```bash
cargo run --bin server -- --dump-ir > ir.json
cargo taut gen --from ir.json --out client/src/api.gen.ts
```

## 5. `cargo taut gen --from-binary` fails with "no such file"

**Symptom.**

```text
error: failed to spawn `target/debug/server`: No such file or directory
```

**Cause.** `--from-binary` invokes a built executable in dump mode.
If you haven't built the server yet (or you `cargo clean`ed), there
is no binary to run.

**Fix.** Build first, then generate:

```bash
cargo build --bin server
cargo taut gen --from-binary target/debug/server --out client/src/api.gen.ts
```

## 6. TS client throws `decode_error`

**Symptom.** A call rejects on the client with an error whose `code`
is `decode_error`, often with a message like `missing field "email"`.

**Cause.** The server received a payload that `serde` could not parse
into the declared input type. The most common offender is a missing
required field — either the client was generated against an older IR,
or a field was renamed without regenerating.

**Fix.** Regenerate the client (`cargo taut gen ...`) and rebuild the
TypeScript bundle. If the call is hand-rolled, double-check the
field names and types against `api.gen.ts`.

## 7. TS client throws `validation_error` from a server response

**Symptom.** A call rejects with `code: "validation_error"`, even
though the input parsed at the type level.

**Cause.** The server's `Validate` impl rejected the payload after
deserialization (length, range, regex, etc.). The per-field detail
lives in `e.payload.errors`.

**Fix.** Inspect the error payload:

```ts
try {
  await client.users.create({ name: "" });
} catch (e) {
  if (e.code === "validation_error") {
    console.log(e.payload.errors);
    // [{ path: ["name"], code: "min_length", min: 1 }]
  }
}
```

The `path` array tells you exactly which field failed which check.

## 8. Subscriptions hang in the browser

**Symptom.** A subscription opens but never delivers events; the
browser console shows a blocked CORS preflight or no network activity
at all after the initial request.

**Cause.** SSE and WebSocket transports are subject to CORS. The
default Axum router does not add CORS headers, so cross-origin
streams stall.

**Fix.** Mount a CORS layer. For development:

```rust
use tower_http::cors::CorsLayer;

let app = Router::new()
    .merge(taut_rpc::axum::router(api))
    .layer(CorsLayer::permissive());
```

For production, configure `CorsLayer` with explicit origins, methods,
and headers — `permissive()` is intentionally loose.

## 9. `JSON.stringify` fails with "Do not know how to serialize a BigInt"

**Symptom.** The TS client throws `TypeError: Do not know how to
serialize a BigInt` when sending or receiving a `u64` / `i64`.

**Cause.** Older `taut-rpc` npm packages relied on the host
`JSON.stringify`, which has no native BigInt support. Phase 4
shipped BigInt-aware encoders for both the SSE and HTTP transports.

**Fix.** Pin the latest `taut-rpc` package:

```bash
npm install taut-rpc@latest
```

If you still see the error, clear the lockfile entry and reinstall —
some monorepo setups cache the old transport.

## 10. Generated `api.gen.ts` doesn't compile

**Symptom.** `tsc` reports type errors inside `api.gen.ts`, or the
file references types that don't exist.

**Cause.** The generated client is a snapshot of a specific IR. After
the server changes (new procedure, renamed field, removed variant),
the snapshot goes stale and the IR's referenced types no longer line
up with the generated TypeScript.

**Fix.** Regenerate after every server change:

```bash
cargo run --bin server -- --dump-ir > ir.json
cargo taut gen --from ir.json --out client/src/api.gen.ts
```

Wire this into your dev loop (e.g. a `cargo watch` task or a `just`
recipe) so codegen never lags the server.

## 11. WebSocket transport not active

**Symptom.** Calling `client.subscribe(...)` over a WebSocket URL
fails with `transport not enabled` or the server returns 404 on the
WS upgrade path.

**Cause.** WebSocket support is gated behind a Cargo feature to keep
the default build small.

**Fix.** Enable the feature:

```bash
cargo build --features ws
```

Or in `Cargo.toml`:

```toml
[dependencies]
taut-rpc = { version = "0.1", features = ["ws"] }
```

## 12. `cargo build` is slow on the first run

**Symptom.** A clean `cargo build` of a taut-rpc project takes a
noticeably long time — especially the `taut-rpc-macros` crate.

**Cause.** Proc-macros are compiled as host-architecture binaries and
pull in `syn`, `quote`, and the IR builder. The first build pays for
all of that; incremental builds reuse the cached artifacts.

**Fix.** This is expected. Subsequent rebuilds (without
`cargo clean`) should be fast — typically under a second for
no-source-change rebuilds, and a few seconds for typical edits. If
you regularly `cargo clean`, consider `sccache` to keep proc-macro
artifacts hot across clean builds.

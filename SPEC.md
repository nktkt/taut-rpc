# taut-rpc — Design Specification

Status: **draft**, version 0. Subject to change until 0.1.0.

## 1. Goals

1. **Refactor safety.** Change a Rust signature → TypeScript compile error at the call site.
2. **Zero runtime schema fetch.** Generated client is static `.ts` — no `/schema.json` round-trip on boot.
3. **One source of truth.** Rust types own the contract; TS mirrors them.
4. **axum-native, not axum-locked.** First-class on axum; pluggable transport trait so other routers can adapt.
5. **Ergonomic generics.** Document the supported subset clearly; fail loudly on unsupported cases.

### Non-goals

- Polyglot servers (see README).
- Schema-first / contract-first workflows.
- Binary wire formats (CBOR/MessagePack) before 0.2.

## 2. Architecture

```
                      ┌───────────────────────┐
   #[rpc] fn / trait ──→  proc-macro emits     │
                      │  - axum handler        │
                      │  - IR entry (JSON)     │
                      └──────────┬─────────────┘
                                 │ build.rs writes IR to target/taut/ir.json
                                 ▼
                      ┌───────────────────────┐
                      │  cargo taut gen       │  ── reads IR, emits .ts
                      └───────────────────────┘
```

Three crates:

| Crate | Role |
|---|---|
| `taut-rpc` | Public API, `Router`, runtime wiring, re-exports. |
| `taut-rpc-macros` | `#[rpc]`, `#[derive(Type)]`, `#[derive(Validate)]`. |
| `taut-rpc-cli` | `cargo taut` subcommand: `gen`, `check`, `inspect`. |

The IR (intermediate representation) is the contract between macro-time and codegen-time. It is JSON, schema-versioned, and lives in `target/taut/ir.json`.

## 3. Type mapping

### 3.1 Primitive

| Rust | TypeScript | Notes |
|---|---|---|
| `bool` | `boolean` | |
| `u8`–`u32`, `i8`–`i32`, `f32`, `f64` | `number` | |
| `u64`, `i64`, `u128`, `i128` | `bigint` | configurable: `as_string` mode emits `string` |
| `String`, `&str`, `Cow<str>` | `string` | |
| `()`, `Result<T, E>` empty | `void` / discriminated union | |
| `Option<T>` | `T \| null` | configurable: `T \| undefined` per-field via `#[taut(undefined)]` |
| `Vec<T>`, `&[T]`, `[T; N]` | `T[]` / `[T, T, ...]` (fixed-size becomes tuple) | |
| `HashMap<String, V>` | `Record<string, V>` | non-string keys → `Array<[K, V]>` |
| `chrono::DateTime`, `time::OffsetDateTime` | `string` (ISO-8601) | feature-gated |
| `uuid::Uuid` | `string` | feature-gated, branded as `Uuid` |

### 3.2 User-defined

- **Structs** → TS interfaces (or type aliases for tuple structs).
- **Enums** → discriminated unions, default tag `"type"`, configurable via `#[taut(tag = "...")]`.
- **Generics** → 0.1 supports lifetime-erased monomorphic forms only. The macro records the *instantiated* types reachable from `#[rpc]` functions; never-instantiated generics never appear in the IR.

### 3.3 Errors

A `#[rpc]` function returns `Result<T, E>` where `E: TautError`. `TautError` requires `code: &'static str` plus a `Serialize` payload. The TS side gets:

```ts
type ApiError<C extends string, P> = { code: C, payload: P };
```

Per-procedure error unions are narrowed; clients don't need to handle errors a procedure can't emit.

## 4. Wire format (v0)

### 4.1 Query / mutation

```
POST /rpc/<procedure>
Content-Type: application/json
Body: { "input": <Input> }

200 OK
Body: { "ok": <Output> }
or
4xx/5xx
Body: { "err": { "code": "...", "payload": ... } }
```

`GET /rpc/<procedure>?input=<urlencoded-json>` is allowed for procedures explicitly marked `#[rpc(method = "GET")]`. Defaults: queries are POST (avoid URL length and caching surprises); explicit GETs are opt-in for cacheable reads.

### 4.2 Subscription

```
GET /rpc/<procedure>?input=...
Accept: text/event-stream

event: data\ndata: <json>\n\n
event: error\ndata: <json>\n\n
event: end\ndata:\n\n
```

WebSocket transport is identical at the message level but framed as JSON messages with `{ type, payload }`.

## 5. Server API

```rust
use taut_rpc::{Router, rpc};

#[rpc]
async fn ping() -> &'static str { "pong" }

let app: axum::Router = Router::new()
    .procedure(ping)
    .with_state(MyState { /* ... */ })
    .into_axum();
```

Middleware: standard `tower::Layer`s. Auth and tracing reuse axum's ecosystem rather than reinventing.

## 6. Client API (generated)

```ts
import { createClient } from "taut-rpc/client";
import type { Procedures } from "./api.gen";

const client = createClient<Procedures>({ url: "/rpc" });

await client.ping();
for await (const evt of client.userEvents.subscribe({ userId: 1 })) {}
```

The runtime is one small npm package (`taut-rpc`) plus the per-project generated `.gen.ts`. The runtime knows about transports; the generated file is pure types + procedure name strings.

## 7. Validation bridge

`#[derive(Validate)]` on input types emits a per-field schema description into the IR. Codegen produces a Valibot schema (Zod is opt-in). The generated client validates inputs *before* sending and validates outputs *after* receiving by default; both can be disabled per-call.

Constraints supported in 0.1: `min`, `max`, `length`, `pattern`, `email`, `url`. Custom predicates are recorded as opaque tags and require user-supplied schema fragments.

## 8. Open questions

- **Streaming uploads.** Multipart vs. chunked SSE-from-client? Likely defer to 0.2.
- **Sum types in TS.** `tag = "type"` collides with user fields named `type` — needs an escape hatch.
- **Generic procedures.** Allow `#[rpc] async fn list<T>(...)`? Almost certainly no in 0.1; force monomorphic wrappers.
- **Authentication contract.** Should errors carry an `unauthenticated` discriminant by convention? Lean yes.

## 9. Compatibility & versioning

- IR has a `"ir_version"` field; codegen refuses mismatches.
- Wire format has a `"v"` field on subscription frames; missing means v0.
- The `taut-rpc` crate and the generated client are versioned together; the runtime npm package's major version tracks the crate's.

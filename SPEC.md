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

A `#[rpc]` function returns `Result<T, E>` where `E: TautError`. `TautError` requires `code: &'static str` plus a `Serialize` payload, and an `http_status() -> u16` mapping to the response status. The TS side gets:

```ts
type ApiError<C extends string, P> = { code: C, payload: P };
```

Per-procedure error unions are narrowed; clients don't need to handle errors a procedure can't emit.

The canonical user-facing pattern is `#[derive(TautError)]` on a serde-tagged enum:

```rust
#[derive(serde::Serialize, taut_rpc::TautError, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum AddError {
    #[taut(status = 400)] Overflow,
    #[taut(status = 401)] Unauthenticated,
}
```

The derive emits `impl TautError` whose `code()` returns the snake_case'd variant name and whose `http_status()` defaults to `400`. Per-variant `#[taut(code = "...", status = N)]` attributes override either default; the `code` override also replaces the wire discriminant the serde tag sees, so the two stay in sync.

`StandardError` is the curated built-in covering common HTTP-mapped errors: `BadRequest` (400), `Unauthenticated` (401), `Forbidden` (403), `NotFound` (404), `Conflict` (409), `UnprocessableEntity` (422), `TooManyRequests` (429), `Internal` (500), `ServiceUnavailable` (503), `Timeout` (504). Use it directly for procedures that don't need a bespoke error union.

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
event: end\ndata: \n\n
```

The default subscription transport is **SSE**. WebSocket is **opt-in** via the
`ws` cargo feature on the `taut-rpc` crate; when enabled, the server mounts
`GET /rpc/_ws` and multiplexes subscriptions over a single connection. WebSocket
transport is identical at the message level but framed as JSON messages with
`{ type, payload }`.

The end-frame's canonical form is `event: end\ndata: \n\n` (a `data:` line with
a single space). The TS parser tolerates either `data: \n` or no `data` line at
all on the end frame.

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

`Router::layer<L>(layer)` wraps the `axum::Router` produced by `into_axum()` with any `L: tower::Layer<axum::routing::Route>` (the same bound `axum::Router::layer` accepts). Layers compose in onion order: the outermost call wraps the previous result, so the layer added last sees the request first.

```rust
let app = Router::new()
    .procedure(__taut_proc_ping())
    .layer(tower_http::trace::TraceLayer::new_for_http())
    .layer(axum::middleware::from_fn(auth))
    .into_axum();
```

Here `auth` runs first on the inbound path (it was added last), then `TraceLayer`, then the procedure handler. Reverse the calls to swap the order.

### 5.1 Subscriptions

```rust
use taut_rpc::{rpc, Router};
use futures::Stream;

#[rpc(stream)]
async fn ticks(input: TicksInput) -> impl Stream<Item = u64> + Send + 'static {
    async_stream::stream! { /* ... */ }
}

let app = Router::new()
    .procedure(__taut_proc_ticks())
    .into_axum();
```

The `Send + 'static` bound on the returned `impl Stream` is **mandatory** — the
runtime spawns the stream onto a task to drive SSE/WS frames, which requires
both bounds.

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

### Resolved during Phase 0/1/2

- **Authentication contract.** *Decided in Phase 2.* `StandardError::Unauthenticated`
  (HTTP 401, code `"unauthenticated"`) is the convention for "no credentials /
  bad credentials". Per-procedure auth is expressed by adding an
  `Unauthenticated` variant to a `#[derive(TautError)]` enum:
  `enum E { #[taut(status = 401)] Unauthenticated, ... }`. The discriminant
  name is normative — TS clients can branch on `err.code === "unauthenticated"`
  uniformly across procedures.

- **JsonRejection handling.** *Decided.* The Phase 1 `Router` catches axum's
  `JsonRejection` and re-emits the SPEC error envelope with
  `code = "decode_error"`, `payload.message = <rejection text>`, HTTP status
  400. Body shape: `{"err":{"code":"decode_error","payload":{"message":"..."}}}`.
- **Unknown procedure 404.** *Decided.* A request to an unregistered procedure
  name returns `{"err":{"code":"not_found","payload":{"procedure":"<name>"}}}`
  with HTTP status 404 — same envelope shape as application errors so clients
  have one parser path.
- **Health endpoint shape.** *Decided for v0.1.* `GET /rpc/_health` returns
  `text/plain` body `ok` with status 200. Simple by design; if monitoring tools
  push for a JSON `{ "status": "ok", "version": "..." }` shape we'll revisit
  in v0.2.

## 9. Compatibility & versioning

- IR has a `"ir_version"` field; codegen refuses mismatches.
- Wire format has a `"v"` field on subscription frames; missing means v0.
- The `taut-rpc` crate and the generated client are versioned together; the runtime npm package's major version tracks the crate's.

## 10. v0.1 surface

The exact set of features that ship in v0.1 (i.e. what Phase 1 of the roadmap
delivers, before errors/middleware, subscriptions, and the validation bridge
land in later phases):

- `#[rpc]` on free `async fn`s with **0 or 1** input argument. Both queries
  (default) and mutations are supported. `#[rpc(method = "GET")]` is supported.
- `#[rpc(stream)]` for `async fn ... -> impl Stream<Item = T> + Send + 'static`
  (Phase 3): **shipping in v0.1**.
- SSE transport for subscriptions (Phase 3): **shipping in v0.1**.
- WebSocket transport, feature-gated under `ws` (Phase 3): **shipping in v0.1**
  on the server side; the TS client's WS transport is **deferred** past v0.1.
- Generated client `.subscribe()` returning `AsyncIterable<T>` (Phase 3):
  **shipping in v0.1**.
- `#[derive(Type)]` for:
  - structs: named, tuple, newtype, unit;
  - enums: unit variants, tuple variants, struct variants.
- `cargo taut gen` codegen, emitting one `api.gen.ts` per project.
- `#[derive(TautError)]` macro (Phase 2): **shipping in v0.1**. Emits
  `impl TautError` with `code()` and `http_status()`, with per-variant
  `#[taut(code = "...", status = N)]` overrides.
- Per-procedure error type narrowing in codegen (Phase 2): **shipping in
  v0.1**. The CLI emits `Proc_<name>_Error` type aliases per procedure (when
  the procedure returns `Result<_, E>` with a non-empty error union) so the
  TS client narrows `err.payload` on a `switch (err.code)`.
- `Router::layer<L>(layer)` for `tower::Layer<axum::routing::Route>` (Phase 2):
  **shipping in v0.1**. Composes axum's standard middleware ecosystem.
- **State extractor support: still deferred.** axum's `State<S>` extractor is
  **not** supported in v0.1; procedures are free functions whose state must
  be reached via `OnceCell`/`static` for now. Planned for **Phase 5 or
  later**; the current Phase 2 middleware story (`Router::layer`,
  `tower::Layer`-based auth) covers the common needs in the meantime.

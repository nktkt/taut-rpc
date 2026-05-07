# Introduction

`taut-rpc` is end-to-end type-safe RPC between Rust servers and TypeScript
clients. You annotate a Rust function with `#[rpc]`, and a `cargo` subcommand
emits a single `.ts` file that gives the frontend a fully typed client — no
schema fetch at boot, no hand-written DTOs, no drift between the two halves
of the application. Queries and mutations ride JSON over HTTP, subscriptions
ride SSE (with WebSocket as an opt-in transport), validation constraints
flow from Rust attributes into Valibot or Zod schemas, and the Rust types
remain the single source of truth. Refactor a Rust signature, get a
TypeScript compile error.

## Status

**v0.1.0 release candidate.** Phases 0–5 have landed on `main`:

| Phase | Theme | State |
|---|---|---|
| 0 | Spec, smoke wire test | shipped |
| 1 | `#[rpc]` macro, IR, TS codegen | shipped |
| 2 | Typed errors, `tower::Layer` middleware | shipped |
| 3 | Subscriptions over SSE / WS | shipped |
| 4 | `#[derive(Validate)]`, Valibot/Zod bridge | shipped |
| 5 | DX polish (`cargo taut check`, `inspect`), docs, examples | finalising |

The IR is at `IR_VERSION = 1` and `cargo taut check` will reject a mismatch
in CI. Once Phase 5 closes, `0.1.0` ships to crates.io and npm. Any breaking
change after that goes through the regular semver process; the IR shape is
considered part of the public surface.

## A working snippet

The server is a plain axum app. The `#[rpc]` macro registers a procedure
into a router and records an IR fragment for codegen.

```rust
// server: src/api.rs
use taut_rpc::{rpc, Type, Validate, TautError};

#[derive(serde::Serialize, serde::Deserialize, Type, Validate)]
pub struct CreateUser {
    #[taut(length(min = 3, max = 32))]
    pub username: String,
    #[taut(email)]
    pub email: String,
}

#[derive(serde::Serialize, serde::Deserialize, Type)]
pub struct User { pub id: u64, pub username: String }

#[derive(Debug, thiserror::Error, TautError)]
pub enum ApiError {
    #[error("user already exists")]
    #[taut(code = "conflict", status = 409)]
    Conflict,
}

#[rpc(mutation)]
async fn create_user(input: CreateUser) -> Result<User, ApiError> {
    Ok(User { id: 1, username: input.username })
}

#[rpc(stream)]
async fn user_events()
    -> impl futures::Stream<Item = User> + Send + 'static
{
    futures::stream::empty()
}
```

```rust
// server: src/main.rs
#[tokio::main]
async fn main() {
    let app = axum::Router::new()
        .nest("/rpc", taut_rpc::router![create_user, user_events]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

Run the codegen once (or wire it into your build script):

```sh
cargo taut gen --out web/src/api.gen.ts
```

The TypeScript side imports the generated client. Inputs are validated
with the same constraints declared on the Rust struct; outputs are typed.

```ts
// client: web/src/main.ts
import { createApi, procedureSchemas } from "./api.gen";

const client = createApi({
  url: "/rpc",
  schemas: procedureSchemas,                  // pre-send + post-recv validation
});

const u = await client.create_user({
  username: "alice",
  email: "alice@example.com",
});                                            // typed User

for await (const e of client.user_events.subscribe()) {
  console.log(e.username);                     // typed User
}
```

## Quick links

- [Getting started](./guides/getting-started.md) — install the crates, run the
  smoke example, generate your first client.
- [Architecture](./concepts/architecture.md) — how the macro, the IR, and the
  CLI fit together.
- [SPEC.md](./reference/spec.md) — the canonical wire format, IR shape, and
  type-mapping table.
- [Examples](https://github.com/x7/taut-rpc/tree/main/examples) — one tiny app
  per phase: queries, auth, tracing, subscriptions, validation.
- [Roadmap](./reference/roadmap.md) — what landed, what is in flight, what is
  speculative.
- [Changelog](./reference/changelog.md) — per-phase additions, IR bumps, and
  wire-version notes.

## Compared to the neighbours

| | `taut-rpc` | [`rspc`](https://github.com/oscartbek/rspc) | [`tRPC`](https://trpc.io) |
|---|---|---|---|
| Server language | Rust | Rust | TypeScript |
| Transport | axum-native (HTTP / SSE / WS) | router-agnostic adapter | HTTP / WS |
| Codegen | static `.ts` from IR — no runtime fetch | static via `specta` | inferred from server types (same language) |
| Subscriptions | first-class, SSE default | yes | yes |
| Validation bridge | Rust attrs → Valibot/Zod | partial via `specta` | Zod-first, server-shared |
| Refactor safety | Rust signature change ⇒ TS compile error | same idea, less ergonomic constraints | same idea, but only inside one language |

The honest summary: if your stack is already Rust + TS, `taut-rpc` is the
shortest path from a typed handler to a typed client. If you are pure
TypeScript, use tRPC. If you need a non-axum router, `rspc` is more flexible.

## Project values

### Refactor safety over surface area

Every feature has to survive the question: *does this make a Rust-side
refactor either propagate to TS or fail loudly?* Anything that lets the two
sides drift silently — runtime registries, optional types, "any" escape
hatches — gets pushed back. The IR is intentionally small.

### No runtime schema fetch

The generated `.ts` file is a self-contained client. Booting the frontend
does not require the server to be reachable, the build does not require a
running database, and the deploy artifact is a static asset. Schema drift
shows up at `cargo taut check` time, not at 3am in production.

### axum-native, not router-agnostic

Pinning to axum lets the macro emit ordinary `axum::handler` functions, plug
into `tower::Layer` for middleware, and reuse axum's extractors for state,
headers, and auth. A router-agnostic abstraction would cost ergonomics for
flexibility most users do not need; a second adapter is on the post-0.1
list, not the 0.1 list.

### Spec drives code

`SPEC.md` is checked in alongside the implementation and the two are
expected to agree. When the implementation finds a hole, the fix is to
update the spec first, then the code. The mdBook you are reading embeds the
spec verbatim under [Reference](./reference/spec.md) so that the design and
the docs cannot drift either.

## How to read these docs

- **Concepts** — short orientations on the architecture, IR, type mapping,
  wire format, errors, and validation. Read these to build a mental model.
- **Guides** — task-oriented walkthroughs: getting started, authentication
  via `tower::Layer`, middleware, validation, and subscriptions.
- **Reference** — the spec, the roadmap, the changelog, and the constraints
  catalogue, all checked in so the book is self-contained and offline-readable.

If you are evaluating whether to adopt `taut-rpc`, start with
[Getting started](./guides/getting-started.md) and skim [Architecture](./concepts/architecture.md).
If you are extending the project, read [SPEC.md](./reference/spec.md) end to end.

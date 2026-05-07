# taut-rpc 0.1.0

> End-to-end type-safe RPC between Rust (axum) and TypeScript clients.

The first stable release. Phases 0 through 5 of the [roadmap](../ROADMAP.md)
have all landed: the workspace is set up, the macro pipeline works end to
end, errors are typed, subscriptions stream, validation flows both ways, and
the DX polish is in place. Cut on 2026-05-07.

If you are coming from a hand-rolled `axum` + `ts-rs` setup, or from rspc
and looking for an axum-native alternative, this is the version where the
contract is the build artifact: change a Rust signature, get a TypeScript
compile error, no schema fetch at runtime.

---

## What's new in 0.1.0

This release consolidates everything that landed across Phases 0–5. The
short tour, one phase at a time:

### Phase 0 — Spec & PoC

The wire format and IR shape are written down. SPEC.md is the canonical
reference; the `examples/smoke/` example is a hand-written end-to-end
round-trip (no macros, no codegen) that proves the envelope is implementable
without any of the comfort layers above it. Workspace scaffold, three crates
(`taut-rpc`, `taut-rpc-macros`, `taut-rpc-cli`), and the npm runtime
skeleton ship here.

### Phase 1 — Macros, IR, and the codegen pipeline

`#[rpc]` on a free async function and `#[derive(Type)]` on a struct or enum
are the two macros you reach for first. Together with `cargo taut gen` they
produce a single `api.gen.ts` that contains every type alias, the
`Procedures` map, and a `createApi` helper — no runtime schema fetch, no
network round-trip just to learn the shape of the API. This is the phase
where "the contract is the build artifact" became literally true.

### Phase 2 — Errors and middleware

`#[derive(TautError)]` lets a normal Rust error enum carry per-variant `code`
and `status` overrides; the wire envelope picks them up automatically. The
generated TypeScript client narrows the payload of `err` based on the
`code` you switch on, so a `switch (err.code)` reads like a discriminated
union and behaves like one. `Router::layer(...)` accepts any
`tower::Layer<axum::routing::Route>`, which means `tower-http::TraceLayer`,
`axum::middleware::from_fn` auth, CORS, compression, and the rest of the
tower ecosystem all compose with taut-rpc routers without taut-specific
glue.

### Phase 3 — Subscriptions

`#[rpc(stream)]` accepts `async fn name(input: I) -> impl Stream<Item = T>`
and turns it into an SSE endpoint by default (`event: data` / `event: error`
/ `event: end` per SPEC §4.2). The generated TS client exposes
`AsyncIterable<T>` so consumers write `for await (const x of
client.foo.subscribe()) { ... }`. WebSocket is the same story behind the
`ws` cargo feature, multiplexed across one socket — pick the transport that
fits the deployment.

### Phase 4 — Validation bridge

`#[derive(Validate)]` records the constraint set
(`length`/`min`/`max`/`pattern`/`email`/`url`/`custom`) on the input type;
the server enforces it after deserialization, the IR propagates the
constraints, and the codegen emits Valibot schemas (the default) or Zod
schemas (`--validator zod`) that the TS runtime can run pre-send and/or
post-receive. One source of truth, two languages, both sides honest about
when they reject. `IR_VERSION` bumps from 0 to 1 here to make room for
`Field.constraints`.

### Phase 5 — DX polish, then 0.1.0

The CLI gains `cargo taut check` (CI-grade IR drift detection), `cargo taut
inspect` (the IR rendered as a human-readable table), and `cargo taut mcp`
(an MCP `tools/list` manifest emitter so a taut-rpc service can be plugged
into LLM agent harnesses without hand-writing schemas). Documentation moved
to a complete mdBook with concepts, guides, tutorials, and reference. Two
full-stack examples (Vite + React, SvelteKit) prove the codegen output is
bundler-agnostic, and the release process (CHANGELOG, security policy,
contributing guide, signed tags) is written down.

---

## Showcase

The canonical end-to-end usage. Server side, in Rust:

```rust
// server: src/api.rs
use taut_rpc::{rpc, Router, Validate};

#[derive(serde::Deserialize, taut_rpc::Type, taut_rpc::Validate)]
pub struct CreateUser {
    #[taut(length(min = 3, max = 32))] pub username: String,
    #[taut(email)]                     pub email: String,
}

#[derive(serde::Serialize, taut_rpc::Type)]
pub struct User {
    pub id: u64,
    pub username: String,
}

#[derive(serde::Serialize, taut_rpc::Type, taut_rpc::TautError, thiserror::Error, Debug)]
pub enum ApiError {
    #[error("conflict")]
    #[taut(code = "conflict", status = 409)]
    Conflict,
    #[error("validation")]
    Invalid(taut_rpc::ValidationError),
}

#[rpc(mutation)]
async fn create_user(input: CreateUser) -> Result<User, ApiError> {
    Ok(User { id: 1, username: input.username })
}

#[rpc(stream)]
async fn user_events() -> impl futures::Stream<Item = User> + Send + 'static {
    async_stream::stream! { /* yield users */ }
}

#[tokio::main]
async fn main() {
    let router = Router::new()
        .procedure(create_user::descriptor())
        .procedure(user_events::descriptor());
    taut_rpc::dump_if_requested(&router);
    let app = router.into_axum();
    axum::serve(/* ... */, app).await.unwrap();
}
```

Client side, in TypeScript, after `cargo taut gen`:

```ts
// client: generated by `cargo taut gen` + 4 lines of glue
import { createApi, procedureSchemas, isTautError } from "./api.gen";

const client = createApi({
  url: "/rpc",
  schemas: procedureSchemas,
  validate: { send: true, recv: false },
});

try {
  const u = await client.create_user({ username: "alice", email: "a@b.c" });
  console.log(u.id);

  for await (const e of client.user_events.subscribe()) {
    console.log("user event", e.id);
  }
} catch (err) {
  if (isTautError(err, "conflict")) {
    console.warn("already exists");
  } else if (isTautError(err, "validation_error")) {
    console.warn(err.payload.errors);
  }
}
```

Validation runs both ways: the client checks `CreateUser` against the
generated Valibot schema before the request leaves the browser, the server
checks again after deserialization, and both sides share the same constraint
source.

---

## Install

```sh
cargo add taut-rpc taut-rpc-macros
cargo install taut-rpc-cli
npm i taut-rpc
```

The Rust crates power the server and the derive macros; the CLI emits the
TypeScript client (`cargo taut gen`); the npm package supplies the runtime
helpers the generated client imports. The npm package's major version is
locked to the `taut-rpc` crate's major version (SPEC §9), so as long as you
match `0.1.x` on both sides you are wire-compatible.

---

## Breaking changes from 0.0.0

- `IR_VERSION` bumped from `0` to `1` — old `target/taut/ir.json` files will
  be rejected by `cargo taut gen` with a clear error message. Re-run
  `cargo run` (with the binary's `dump_if_requested` step) to regenerate the
  IR before running codegen.
- `ProcedureDescriptor.handler` field renamed to `body` and re-typed from
  `ProcedureHandler` (a unary closure) to `ProcedureBody { Unary | Stream }`.
  The legacy `ProcedureHandler` type alias still resolves to `UnaryHandler`
  for source-level compatibility, but pattern-matches on the descriptor must
  switch over `body` instead of calling `handler` directly.

There are no other breaking changes from the pre-release `0.0.0-phase0`
tag. If you tracked `main` during the phase rollout you have already paid
these costs.

---

## Documentation

The documentation site is an mdBook covering concepts, guides, tutorials,
and a reference for every macro, trait, and CLI subcommand:

- **Site:** <https://nktkt.github.io/taut-rpc> *(gh-pages URL — populated
  by the `docs.yml` workflow on tag push; may take a few minutes after the
  release publishes)*
- **Source:** [`docs/`](../docs/) in the repo, build with `mdbook serve`
- **SPEC:** [`SPEC.md`](../SPEC.md) — wire format and codegen contract
- **CHANGELOG:** [`CHANGELOG.md`](../CHANGELOG.md) — full per-phase log

The mdBook is the place to start; SPEC.md is the place to land when you need
to know exactly what the wire looks like.

---

## Examples

The repo ships eight runnable examples under `examples/`. The reference set
maps one-to-one onto the roadmap phases, the full-stack set glues several
phases together inside a real frontend toolchain. All ports are disjoint so
several can run side-by-side; CORS is permissive throughout.

**Reference examples** (one phase, end to end):

- [`examples/smoke/`](../examples/smoke) — Phase 0: hand-written wire
  format reference (no macros, no codegen). Port 7700.
- [`examples/phase1/`](../examples/phase1) — Phase 1: `#[rpc]` +
  `#[derive(Type)]` + `cargo taut gen` end to end. Port 7701.
- [`examples/phase2-auth/`](../examples/phase2-auth) — Phase 2: bearer
  token middleware + `#[derive(TautError)]`. Port 7702.
- [`examples/phase2-tracing/`](../examples/phase2-tracing) — Phase 2:
  `tower-http::TraceLayer` layered over a taut-rpc router. Port 7703.
- [`examples/phase3-counter/`](../examples/phase3-counter) — Phase 3:
  `#[rpc(stream)]` + SSE consumed from `for await`. Port 7704.
- [`examples/phase4-validate/`](../examples/phase4-validate) — Phase 4:
  `#[derive(Validate)]` + Valibot bridge. Port 7705.

**Full-stack apps** (real frontend toolchains, several phases at once):

- [`examples/todo-react/`](../examples/todo-react) — Phase 5: axum +
  Vite + React 18, broadcast-channel-backed subscription keeps two browser
  tabs in sync. Ports 7710/7711.
- [`examples/counter-sveltekit/`](../examples/counter-sveltekit) — Phase 5:
  axum + SvelteKit. The Svelte counterpart to `todo-react`, proving the
  codegen output is bundler-agnostic. Port 7712.

See [`examples/README.md`](../examples/README.md) for the full catalog,
port map, and one-time setup notes (you'll need to `npm install && npm run
build` in `npm/taut-rpc/` once before any example with a TS client will
resolve).

---

## Roadmap

What's already in scope for "Beyond 0.1" is tracked in
[`ROADMAP.md`](../ROADMAP.md#beyond-01--speculative). Highlights:

- File uploads (multipart, resumable).
- CBOR / MessagePack transports.
- A second backend adapter (`actix-web` or `salvo`).
- A "thin OpenAPI" emitter for teams that want a parallel REST surface.
- Generic procedure support if a clean design emerges.
- A devtools panel that taps the runtime and shows the call stream.

The MCP manifest emitter from that list already shipped (it's `cargo taut
mcp` in this release). Everything else is exploratory and may be cut. Open
an issue with a use case if you want one of those moved up.

---

## Thanks

Authored by [@nktkt](https://github.com/nktkt). Contributions, bug reports,
and design feedback are welcome — see [`CONTRIBUTING.md`](../CONTRIBUTING.md)
for the workflow and [`SECURITY.md`](../SECURITY.md) for the security
disclosure policy.

Inspirations and prior art that shaped the design:

- **[rspc](https://github.com/oscartbek/rspc)** — for proving that a
  Rust↔TS RPC layer with macro-driven codegen is the right shape, and for
  the runtime-driven router design we deliberately diverged from in favour
  of static `api.gen.ts`.
- **[tRPC](https://trpc.io)** — for normalising the "the contract is the
  build artifact" expectation in the TypeScript ecosystem and for the
  ergonomics of typed errors on the client.
- **[axum](https://github.com/tokio-rs/axum)** — for being the right host
  router. taut-rpc routers `into_axum()` into a normal `axum::Router`, so
  every tower middleware in the ecosystem composes for free.
- **[valibot](https://valibot.dev)** — for the tree-shakeable schema
  approach that lets `procedureSchemas` ship without bloating bundles.
- **[zod](https://zod.dev)** — for being the lingua franca of TS runtime
  validation, and for graciously accepting a `--validator zod` flag.
- **[serde](https://serde.rs)** — the obvious one. Without serde the wire
  envelope would be a much harder problem.

---

## Status

**Pre-1.0.** The 0.x series follows semver in spirit but minor bumps may
include breaking changes; 1.0 will follow the strictest interpretation
once the API has had a few months of community feedback.

In practice that means:

- The wire format and IR shape are stable for the lifetime of `0.1.x`.
  `IR_VERSION = 1` and the SSE/WebSocket frame shapes documented in SPEC §4
  will not change in a patch release.
- The Rust API surface (`#[rpc]`, `#[derive(Type)]`, `#[derive(TautError)]`,
  `#[derive(Validate)]`, `Router`) is stable enough to build on. Method
  signatures may pick up new parameters with defaults; existing call sites
  will keep compiling.
- The TypeScript runtime (`createApi`, `isTautError`, the schemas surface)
  is stable. Generated `api.gen.ts` files emitted by `cargo taut gen` from
  this release will keep working with the matching `0.1.x` runtime.
- Anything in `Beyond 0.1` is out of scope for the 0.1.x lifetime unless an
  RFC moves it up.

If you find a sharp edge, a confusing error message, or a place where the
docs lie, please file an issue. The point of cutting 0.1.0 is to start
collecting that feedback in earnest.

Happy hacking.

# taut-rpc

End-to-end type-safe RPC between Rust servers and TypeScript clients.

> **Status:** Phases 0–4 landed (workspace scaffold, end-to-end pipeline,
> error model, subscriptions, validation bridge). Approaching v0.1.0.
> See `ROADMAP.md` for what's left.

## Why

If you write a Rust backend and a TypeScript frontend, you currently glue them together by hand: define a Rust handler, write an OpenAPI schema (or `ts-rs`-generated types), then wire the client. The types drift, the runtime validation is yours to write, and refactors break silently.

`taut-rpc` aims to make the wire as **taut** as the function call: change a Rust signature, get a TypeScript compile error.

## Approach

- **Server side:** an attribute macro (`#[rpc]`) on a plain Rust function or trait registers it into a router that lives on top of [axum](https://github.com/tokio-rs/axum).
- **Client side:** a `cargo` subcommand emits a single `.ts` file containing a fully typed client — no runtime reflection, no schema fetch.
- **Wire format:** JSON over HTTP for queries/mutations, SSE for subscriptions. WebSocket is opt-in.
- **Validation:** types implement a `Validate` trait (auto-derived); the client mirrors them via [Valibot](https://valibot.dev) or [Zod](https://zod.dev) schemas, also generated.

## Comparison

| | `taut-rpc` | [`rspc`](https://github.com/oscartbek/rspc) | [`taurpc`](https://github.com/MatsDK/TauRPC) | `ts-rs` + axum |
|---|---|---|---|---|
| Transport | axum (HTTP/SSE/WS) | router-agnostic | Tauri IPC only | manual |
| Codegen | `cargo taut gen` | runtime-driven | macro-time | manual |
| Status | active — Phases 0–4 landed | stalled | active (Tauri-only) | low-level |
| Subscriptions | first-class | yes | yes | n/a |
| Validation bridge | yes (Valibot default, Zod opt-in, custom)[^vbridge] | partial (via `specta`; less ergonomic) | n/a (Tauri-only IPC) | n/a (manual) |

[^vbridge]: Constraints flow Rust → IR → TS schemas; server-side enforcement is automatic (the `#[derive(Validate)]` macro wires input validation into every `#[rpc]` handler).

## Non-goals

- **Cross-language servers.** This is Rust↔TS. Adding Go, Python, etc. would force a lowest-common-denominator type system and that defeats the point.
- **gRPC compatibility.** If you need gRPC, use [`tonic`](https://github.com/hyperium/tonic).
- **Schema-first workflows.** Rust types are the source of truth.

## Quick taste (planned API)

```rust
// server: src/api.rs
use taut_rpc::{rpc, Validate};

#[derive(serde::Serialize, serde::Deserialize, taut_rpc::Type, taut_rpc::Validate)]
pub struct CreateUser {
    #[taut(length(min = 3, max = 32))]
    pub username: String,
    #[taut(email)]
    pub email: String,
}

#[derive(serde::Serialize, serde::Deserialize, taut_rpc::Type)]
pub struct User { pub id: u64, pub username: String }

#[rpc(mutation)]
async fn create_user(input: CreateUser) -> Result<User, ApiError> { /* ... */ }

#[rpc(stream)]
async fn user_events() -> impl futures::Stream<Item = UserEvent> + Send + 'static { /* ... */ }
```

```ts
// client (generated): src/api.gen.ts
import { createApi, procedureSchemas } from "./api.gen";

const client = createApi({
  url: "/rpc",
  schemas: procedureSchemas,                  // pre-send + post-recv validation
});

const u = await client.create_user({ username: "alice", email: "a@b.c" });
for await (const e of client.user_events.subscribe()) { /* typed UserEvent */ }
```

## Agent tooling

`cargo taut mcp` emits a [Model Context Protocol](https://modelcontextprotocol.io/) `tools/list` manifest from the same IR that drives the TypeScript client. Each query/mutation procedure becomes an MCP tool whose `inputSchema` is JSON Schema (Draft 2020-12), with reachable named types inlined as `$defs` and rustdoc surfaced as `description`. Drop the resulting `mcp.json` into any MCP-aware agent harness to expose your taut-rpc service as a callable toolset — no hand-written schemas.

```sh
cargo taut mcp --out target/taut/mcp.json
# or, dump straight from a built binary:
cargo taut mcp --from-binary target/debug/my-server --out -
```

## Building

Track progress in [`ROADMAP.md`](./ROADMAP.md) and read the design in [`SPEC.md`](./SPEC.md). The repo currently ships these examples:

- `examples/phase1/` — basic queries
- `examples/phase2-auth/` — middleware + bearer auth
- `examples/phase2-tracing/` — tower-http TraceLayer
- `examples/phase3-counter/` — SSE subscriptions
- `examples/phase4-validate/` — input validation
- `examples/smoke/` — Phase 0 hand-written reference

## License

Dual-licensed under either of [Apache-2.0](./LICENSE-APACHE) or [MIT](./LICENSE-MIT) at your option.

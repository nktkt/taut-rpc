# taut-rpc

End-to-end type-safe RPC between Rust servers and TypeScript clients.

> **Status:** Day 0 — design phase. Spec and roadmap below; no code yet.

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
| Status | active (new) | stalled | active (Tauri-only) | low-level |
| Subscriptions | first-class | yes | yes | n/a |
| Validation bridge | yes (Valibot/Zod) | partial | n/a | n/a |

## Non-goals

- **Cross-language servers.** This is Rust↔TS. Adding Go, Python, etc. would force a lowest-common-denominator type system and that defeats the point.
- **gRPC compatibility.** If you need gRPC, use [`tonic`](https://github.com/hyperium/tonic).
- **Schema-first workflows.** Rust types are the source of truth.

## Quick taste (planned API)

```rust
// server: src/api.rs
use taut_rpc::rpc;

#[derive(serde::Serialize, serde::Deserialize, taut_rpc::Type)]
pub struct User { id: u64, name: String }

#[rpc]
async fn get_user(id: u64) -> Result<User, ApiError> { /* ... */ }

#[rpc(stream)]
async fn user_events() -> impl Stream<Item = UserEvent> { /* ... */ }
```

```ts
// client (generated): src/api.gen.ts
import { client } from "./taut";

const u = await client.getUser({ id: 1 });        // typed User
for await (const e of client.userEvents()) { /* typed UserEvent */ }
```

## Building

Nothing to build yet. Track progress in [`ROADMAP.md`](./ROADMAP.md) and read the design in [`SPEC.md`](./SPEC.md).

## License

Dual-licensed under either of [Apache-2.0](./LICENSE-APACHE) or [MIT](./LICENSE-MIT) at your option.

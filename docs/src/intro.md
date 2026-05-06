# Introduction

`taut-rpc` is end-to-end type-safe RPC between Rust servers and TypeScript
clients. You write a Rust function, annotate it with `#[rpc]`, and a
`cargo` subcommand emits a single `.ts` file that gives the frontend a fully
typed client — no schema fetch at boot, no hand-maintained DTOs, no
`ts-rs`-shaped drift between the two halves of the application. The wire is
JSON over HTTP for queries and mutations, SSE for subscriptions, and
WebSocket as an opt-in transport. The Rust types are the source of truth;
TypeScript mirrors them.

## Status

**Day 0 (pre-0.1) — design phase.** No code has been written yet. The
specification and the roadmap are the source of truth for what the project
intends to be. Track progress in [`ROADMAP.md`](./reference/roadmap.md) and
read the design in [`SPEC.md`](./reference/spec.md).

## Quick taste

The planned shape of the API, copied from the project README:

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

## How to read these docs

The **Concepts** section explains the moving pieces — the architecture, the
IR, the type mapping, the wire format, errors, and validation. Each chapter
is a brief orientation; the canonical detail lives in `SPEC.md`, included
verbatim under **Reference**.

The **Guides** section is task-oriented and will fill in as phases land:
getting started, subscriptions (Phase 3), and authentication via
`tower::Layer` (Phase 2).

The **Reference** section embeds the spec, roadmap, and changelog so the
mdBook is self-contained: you can read it offline and have the design,
plan, and history in one place.

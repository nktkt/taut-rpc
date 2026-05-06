# Getting started

> Placeholder guide. The full tutorial lands with Phase 1 (macros + IR +
> codegen). For now, this page describes the **Phase 0 smoke test** —
> the hand-written end-to-end round-trip that exists to validate the
> wire format before any macro infrastructure is built.

## What the Phase 0 smoke is

The smoke is exactly one Rust handler and one TypeScript caller, with
no `#[rpc]` macro, no `taut-rpc::Type` derive, and no `cargo taut gen`.
The Rust side is a hand-written axum handler at `POST /rpc/<name>`; the
TS side is a hand-written `fetch` call against the same path. The
purpose is to nail down the wire envelope (`{ "input": ... }` →
`{ "ok": ... } | { "err": { ... } }`) on the smallest possible
surface, so that everything built on top of it in Phase 1 inherits a
known-good shape.

The smoke lives at `examples/smoke/` in the repository — it is
intentionally not a published crate. Treat it as an executable
specification fragment.

## Running it

From the repository root:

```sh
# Terminal 1: start the server.
cargo run -p smoke-server

# Terminal 2: run the TypeScript caller.
cd examples/smoke/client
npm install
npm run smoke
```

The server logs the request envelope it received; the client prints the
response envelope it got back. If both halves agree, Phase 0 has done
its job.

## What the smoke does **not** cover

- **No subscriptions.** SSE / WebSocket transports are Phase 3.
- **No codegen.** Both halves are hand-written; the IR doesn't exist
  yet.
- **No validation.** Inputs are taken as-is; Phase 4 adds the bridge.
- **No middleware story.** Authentication via `tower::Layer` is Phase
  2.

## What lands here when Phase 1 ships

This page becomes a real getting-started tutorial: a fresh `cargo new`,
adding `taut-rpc` as a dependency, writing the first `#[rpc]`
function, running `cargo taut gen`, and importing the typed client into
a Vite or SvelteKit project. The Phase 0 smoke material moves to a
separate "How the wire works" appendix.

## See also

- [Roadmap — Phase 0](../reference/roadmap.md)
- [Wire format](../concepts/wire-format.md)
- [The IR](../concepts/ir.md)

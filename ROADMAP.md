# Roadmap

Target for the first stable release: **0.1.0**, scoped narrow on purpose. Anything past 0.1 is exploratory and may be cut.

## Phase 0 — Spec & PoC  *(in progress)*

- [x] Problem statement and differentiation written.
- [x] Wire format and IR shape sketched (`SPEC.md`).
- [ ] Hand-written end-to-end smoke: one Rust handler, one TS caller, no macros, no codegen — just to validate the wire.
- [ ] Decide: `serde_json` only, or also `simd-json` behind a feature? (Default to `serde_json`; defer.)

**Exit criteria:** A request/response round-trip works against a hardcoded TS client. Subscriptions deferred to Phase 3.

## Phase 1 — Macro + IR + minimal codegen

- [ ] `taut-rpc-macros`: `#[rpc]` attribute on free async fns. Emit axum handler + IR fragment.
- [ ] `#[derive(Type)]`: walk the struct/enum, emit IR type entries.
- [ ] `taut-rpc-cli`: read `target/taut/ir.json`, emit `api.gen.ts` with type aliases and a `Procedures` interface keyed by procedure name.
- [ ] Runtime npm package `taut-rpc`: 50-line fetch wrapper.

**Exit criteria:** `cargo run` + `cargo taut gen` produces a working typed client for queries and mutations on a sample app. Tests cover the type mapping table from §3.1 of the spec.

## Phase 2 — Errors and middleware

- [ ] `TautError` trait + `#[derive(TautError)]`.
- [ ] Per-procedure error type narrowing in codegen.
- [ ] axum `tower::Layer` integration documented (auth, tracing examples).
- [ ] Decide: do we ship an `Unauthenticated` standard error code? (Tentative yes.)

**Exit criteria:** Errors are typed end-to-end. The TS client can `switch` on `err.code` and the type system narrows the payload.

## Phase 3 — Subscriptions

- [ ] `#[rpc(stream)]` for `impl Stream<Item = T>` returns.
- [ ] SSE transport on the server.
- [ ] WS transport behind a feature flag.
- [ ] Generated client exposes `AsyncIterable` for streams.

**Exit criteria:** A counter that ticks once a second is observable from a TS `for await`.

## Phase 4 — Validation bridge

- [ ] `#[derive(Validate)]` recording the constraint set listed in spec §7.
- [ ] Codegen emits Valibot schemas; Zod is a CLI flag.
- [ ] Pre-send and post-receive validation toggles per call.

**Exit criteria:** A constraint added on the Rust side fails the TS build if downstream callers can't satisfy it.

## Phase 5 — DX polish, then 0.1.0

- [ ] `cargo taut check` to detect IR drift in CI.
- [ ] `cargo taut inspect` to render the IR as a human table.
- [ ] Error messages on unsupported types point at spec §3 with line numbers.
- [ ] Examples: an axum + Vite + React app, an axum + SvelteKit app.
- [ ] Documentation site (mdBook) with the spec and a tutorial.
- [ ] Cut `0.1.0` to crates.io and npm.

## Beyond 0.1 — speculative

- File uploads (multipart, resumable).
- CBOR / MessagePack transports.
- A second backend adapter (`actix-web`? `salvo`?).
- A "thin OpenAPI" emitter for teams that want a parallel REST surface.
- Generic procedure support if a clean design emerges.
- A devtools panel that taps the runtime and shows the call stream.

## Principles

- **Scope discipline.** Each phase ships something usable. No half-finished phases on `main`.
- **Spec drives code, not vice versa.** When the implementation finds a hole, fix the spec first.
- **Honest comparisons.** When `rspc` does something better, say so in the docs and copy the idea.
- **Stable IR before stable API.** Once 0.1 ships, the IR shape is harder to change than the Rust API.

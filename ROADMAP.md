# Roadmap

## v0.1.0 — Released 2026-05-07

The first stable release of taut-rpc. See the [CHANGELOG](./CHANGELOG.md#010---2026-05-07)
for full details and the [release notes](./.github/RELEASE_NOTES_v0.1.0.md) for the
announcement.

Phases 0–5 of the original roadmap have all landed. Subsequent versions will be tracked
in the CHANGELOG against semver-bumped tags rather than as numbered phases.

## Post-0.1.0 — under consideration

Items that were parked outside the 0.1.0 scope. Nothing here is committed; each will be
re-evaluated against real user demand before a future release picks it up.

- **File uploads (multipart, resumable).** Open question — design pending.
- **CBOR / MessagePack transports.** Speculative — wait for a concrete user need.
- **A second backend adapter (`actix-web`? `salvo`?).** Speculative — would force a
  cleaner abstraction over the current axum-coupled runtime.
- **A "thin OpenAPI" emitter** for teams that want a parallel REST surface.
  Speculative.
- **Generic procedure support** if a clean design emerges. Still no clean design.
- **A devtools panel** that taps the runtime and shows the call stream.
  Speculative — likely a separate package if pursued.

(The MCP tools manifest emitter that originally lived in this list shipped in 0.1.0 as
`cargo taut mcp` — see the CHANGELOG.)

---

## Historical phases

The phase-numbered sections below are kept for historical context. All six phases
shipped in v0.1.0; consult the CHANGELOG for the authoritative record of what landed.

Target for the first stable release was **0.1.0**, scoped narrow on purpose. Anything
past 0.1 was treated as exploratory and subject to cut.

## Phase 0 — Spec & PoC

*Shipped in v0.1.0 — see CHANGELOG.*

- [x] Problem statement and differentiation written.
- [x] Wire format and IR shape sketched (`SPEC.md`).
- [x] Hand-written end-to-end smoke: one Rust handler, one TS caller, no macros, no codegen — just to validate the wire.
- [x] Decide: `serde_json` only, or also `simd-json` behind a feature? (Default to `serde_json`; defer.)

**Exit criteria:** A request/response round-trip works against a hardcoded TS client. Subscriptions deferred to Phase 3.

## Phase 1 — Macro + IR + minimal codegen

*Shipped in v0.1.0 — see CHANGELOG.*

- [x] `taut-rpc-macros`: `#[rpc]` attribute on free async fns. Emit axum handler + IR fragment.
- [x] `#[derive(Type)]`: walk the struct/enum, emit IR type entries.
- [x] `taut-rpc-cli`: read `target/taut/ir.json`, emit `api.gen.ts` with type aliases and a `Procedures` interface keyed by procedure name.
- [x] Runtime npm package `taut-rpc`: 50-line fetch wrapper.

**Exit criteria:** `cargo run` + `cargo taut gen` produces a working typed client for queries and mutations on a sample app. Tests cover the type mapping table from §3.1 of the spec. *(landed on main 2026-05-06)*

## Phase 2 — Errors and middleware

*Shipped in v0.1.0 — see CHANGELOG.*

- [x] `TautError` trait + `#[derive(TautError)]`.
- [x] Per-procedure error type narrowing in codegen.
- [x] axum `tower::Layer` integration documented (auth, tracing examples).
- [x] Decide: do we ship an `Unauthenticated` standard error code? (Tentative yes.)

**Exit criteria:** Errors are typed end-to-end. The TS client can `switch` on `err.code` and the type system narrows the payload. *(landed on main 2026-05-06)*

## Phase 3 — Subscriptions

*Shipped in v0.1.0 — see CHANGELOG.*

- [x] `#[rpc(stream)]` for `impl Stream<Item = T>` returns.
- [x] SSE transport on the server.
- [x] WS transport behind a feature flag.
- [x] Generated client exposes `AsyncIterable` for streams.

**Exit criteria:** A counter that ticks once a second is observable from a TS `for await`. *(landed on main 2026-05-06)*

## Phase 4 — Validation bridge

*Shipped in v0.1.0 — see CHANGELOG.*

- [x] `#[derive(Validate)]` recording the constraint set listed in spec §7.
- [x] Codegen emits Valibot schemas; Zod is a CLI flag.
- [x] Pre-send and post-receive validation toggles per call.

**Exit criteria:** A constraint added on the Rust side fails the TS build if downstream callers can't satisfy it. *(landed on main 2026-05-07)*

## Phase 5 — DX polish, then 0.1.0

*Shipped in v0.1.0 — see CHANGELOG.*

- [x] `cargo taut check` to detect IR drift in CI.
- [x] `cargo taut inspect` to render the IR as a human table.
- [x] Error messages on unsupported types point at spec §3 with line numbers.
- [x] Examples: an axum + Vite + React app, an axum + SvelteKit app.
- [x] Documentation site (mdBook) with the spec and a tutorial.
- [x] Cut `0.1.0` to crates.io and npm.

**Exit criteria:** 0.1.0 published, examples and docs site live, CLI drift detection wired into CI. *(landed on main 2026-05-07)*

## Principles

- **Scope discipline.** Each phase ships something usable. No half-finished phases on `main`.
- **Spec drives code, not vice versa.** When the implementation finds a hole, fix the spec first.
- **Honest comparisons.** When `rspc` does something better, say so in the docs and copy the idea.
- **Stable IR before stable API.** Once 0.1 ships, the IR shape is harder to change than the Rust API.

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once it reaches 0.1.0. Until then, every release MAY contain breaking changes.

## Wire / IR compatibility

`taut-rpc` exposes two forms of compatibility on top of crate semver:

- **IR version** — the integer at `taut_rpc::IR_VERSION` and the `ir_version` field
  in `target/taut/ir.json`. Bumped whenever the IR shape changes incompatibly.
  `cargo taut check` rejects a mismatch.
- **Wire version** — the `v` field on subscription frames (SPEC §9). Missing means v0.

Each entry below MUST note an IR or wire bump if one happened.

---

## [Unreleased] — Phase 3

### Added
- `#[rpc(stream)]` attribute now expands to a real subscription descriptor.
  Supports `async fn name(input: I) -> impl futures::Stream<Item = T> + Send + 'static`.
- `taut_rpc::ProcedureBody::{Unary, Stream}` enum — descriptor's body field is
  now a sum type. `taut_rpc::StreamFrame::{Data, Error}` is the per-frame
  value emitted by `StreamHandler`.
- SSE dispatch in `Router::into_axum()`: subscription procedures mount
  `GET /rpc/<name>?input=<urlencoded-json>` and emit
  `event: data` / `event: error` / `event: end` frames per SPEC §4.2.
- WebSocket transport behind cargo feature `ws`. Mounts `GET /rpc/_ws` and
  multiplexes subscriptions via the `WsMessage` wire types.
- `async-stream` is a runtime dependency to support the macro's expansion.
- Phase 3 example: `examples/phase3-counter/` — a tick counter visible from
  a TS `for await` loop.
- Documentation: `docs/src/guides/subscriptions.md` rewritten from placeholder.

### Changed
- `ProcedureDescriptor.handler` field renamed to `body` and re-typed from
  `ProcedureHandler` (a unary closure) to `ProcedureBody` (an enum). The
  legacy `ProcedureHandler` type alias still resolves to `UnaryHandler`.
- `#[rpc]` macro emission now wraps unary handlers in `ProcedureBody::Unary(...)`.

### IR
- IR_VERSION still 0. Subscription procedures' `kind = Subscription` was
  already in the IR shape; this phase wires it through to runtime dispatch.

### Wire
- SSE end-frame: canonical form is `event: end\ndata: \n\n`. The TS runtime
  accepts `data:`-with-no-content too for tolerance.
- WebSocket: feature-gated; server-side only in v0.1.

---

## [Unreleased] — Phase 2

### Added
- `#[derive(TautError)]` proc-macro: emits `impl TautError` with `code()` and `http_status()`.
  Per-variant `#[taut(code = "...", status = N)]` overrides the default snake_case'd
  variant name and the default 400 status.
- `Router::layer<L>(layer)` builder method that wraps the `axum::Router` produced by
  `into_axum()` with any `tower::Layer<axum::routing::Route>`.
- `StandardError` gained 5 new variants: `BadRequest`, `Conflict`, `UnprocessableEntity`,
  `ServiceUnavailable`, `Timeout`.
- Codegen now emits a `Proc_<name>_Error` alias per procedure (when errors exist) and a
  `procedureKinds` const map for runtime kind dispatch.
- npm runtime: `assertTautError`, `errorMatch`, and richer `isTautError` overloads for
  payload narrowing.
- Phase 2 examples: `examples/phase2-auth/` and `examples/phase2-tracing/`.
- Documentation: `docs/src/concepts/errors.md` and `docs/src/guides/auth.md` rewritten;
  new `docs/src/guides/middleware.md` covering `tower::Layer` composition.

### Changed
- `#[rpc]` macro now uses `<E as TautError>::code()` / `http_status()` for the wire
  envelope (was previously inspecting the serialized JSON for a top-level `code` field).
  Error types in `Result<T, E>` returns now must implement `TautError` — use
  `#[derive(TautError)]`.
- npm `ProcedureDef` gained a 4th type param `K extends ProcedureKind` defaulted to the
  full union, so codegen-emitted aliases pin the kind for tighter `ClientOf<P>` inference
  (this landed in Phase 1 fixup but is recorded here for completeness).

### IR
- IR_VERSION still 0.

### Wire
- No change.

---

## [Unreleased] — Phase 1

### Added

- `#[rpc]` attribute macro: free async fns, 0 or 1 input arg, query/mutation, return T or Result<T, E>.
- `#[derive(Type)]` macro for structs (named/tuple/unit), enums (unit/tuple/struct variants).
- `taut_rpc::TautType` trait + blanket impls for primitives, Option, Vec, fixed arrays, HashMap, tuples up to 4.
- `taut_rpc::ProcedureDescriptor` + type-erased `ProcedureHandler`.
- `Router::procedure(...)`, `Router::ir(...)`, `Router::into_axum(...)` rewritten for real registration.
- SPEC envelope wrapping for axum `JsonRejection` and unknown procedures.
- `taut_rpc::dump_if_requested(&router)` for `cargo taut gen --from-binary` IR extraction.
- `cargo taut gen` codegen: emits a single `api.gen.ts` with type aliases, Procedures map, `createApi` helper.
- `npm/taut-rpc` runtime: re-exported `TautError`, added `isTautError` typeguard.
- Phase 1 example (`examples/phase1/`) using the macro-driven flow end-to-end.
- Cargo features: `ir-export` (debug `/rpc/_ir` route), `uuid`, `chrono`.

### Changed

- Router's procedure list is now backed by typed `ProcedureDescriptor` (was Phase 0 stub).
- Workspace `taut-rpc` adds `taut-rpc-macros` as a dep so users only depend on `taut-rpc`.

### IR

- IR_VERSION still 0 (no shape change). Bumps to come when validation/extension fields land.

### Wire

- No change.

---

## [0.0.0-phase0] - 2026-05-06

### Added

- Initial Day-0 design docs: README, SPEC, ROADMAP.
- Workspace scaffold with three crates (`taut-rpc`, `taut-rpc-macros`, `taut-rpc-cli`).
- TypeScript runtime npm package skeleton (`taut-rpc`).
- Phase 0 hand-written smoke example (`examples/smoke/`).
- IR shape and serde types (initial `IR_VERSION = 0`).
- Type mapping module covering primitives, Option, Vec, Map, Tuple, FixedArray.
- Wire format types: `RpcRequest`, `RpcResponse`, `ErrEnvelope`, `SubFrame`, `WsMessage`.
- `TautError` trait + `StandardError` enum.
- `Validate` trait + `Constraint` vocabulary.
- HTTP and SSE transports in the npm runtime.
- mdBook docs scaffold under `docs/`.
- CI workflows (Rust fmt/clippy/test/MSRV, npm typecheck/build, mdBook build).
- `cargo deny` config.

### IR

- IR_VERSION = 0 introduced.

### Wire

- Subscription frames default to v0 (no `v` field).

---

## Release process (placeholder)

Until 0.1.0, every push to `main` is implicitly "Unreleased". A release commit
moves entries from `[Unreleased]` to `[X.Y.Z] - YYYY-MM-DD` and tags
`v0.0.0`/`v0.1.0` etc.

When cutting a release:

1. Rename `[Unreleased]` to `[X.Y.Z] - YYYY-MM-DD` and add a fresh empty
   `[Unreleased]` block above it.
2. If the IR shape changed, bump `IR_VERSION` in the same commit and add a row
   under **### IR** describing what moved.
3. If the subscription wire envelope changed, bump the `v` field default and
   add a row under **### Wire**. Keep a short note on whether the server still
   accepts the previous `v` for backward compatibility.
4. The runtime npm package's major version is bumped in lockstep with the
   `taut-rpc` crate's major version (SPEC §9). Patch and minor versions may
   diverge.
5. Tag the commit `vX.Y.Z` and publish to crates.io and npm in that order
   (Rust first so the IR is canonical before the runtime ships).

### Categories

Following Keep a Changelog, plus two project-specific categories:

- **Added** — new features.
- **Changed** — changes in existing functionality.
- **Deprecated** — soon-to-be removed features.
- **Removed** — features removed in this release.
- **Fixed** — bug fixes.
- **Security** — vulnerabilities.
- **IR** — `IR_VERSION` bumps and IR shape changes (project-specific).
- **Wire** — subscription frame `v` bumps and envelope changes (project-specific).

Pre-0.1.0 entries do not need to be exhaustive. Once 0.1.0 ships, every
user-visible change lands here in the same PR that introduces it.

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Within the 0.x series, breaking changes bump the minor version; from 1.0
onward, the strictest semver interpretation applies.

## Wire / IR compatibility

`taut-rpc` exposes two forms of compatibility on top of crate semver:

- **IR version** — the integer at `taut_rpc::IR_VERSION` and the `ir_version` field
  in `target/taut/ir.json`. Bumped whenever the IR shape changes incompatibly.
  `cargo taut check` rejects a mismatch.
- **Wire version** — the `v` field on subscription frames (SPEC §9). Missing means v0.

Each entry below MUST note an IR or wire bump if one happened.

---

## [Unreleased]

### Added
<!-- placeholder for post-0.1.0 changes -->

### Changed
- CI: bumped `actions/checkout` v4 → v6 and `actions/setup-node` v4 → v6 in
  the workflow files that were still on the older majors. Dependabot continues
  to track weekly bumps for the rest.

### Deprecated
- GitHub Actions are still using `actions/upload-artifact@v4`,
  `actions/upload-pages-artifact@v3`, `actions/deploy-pages@v4`, and
  `peaceiris/actions-mdbook@v2`, all of which run on Node.js 20. Node 20 is
  scheduled for forced migration on 2026-09-16 and full removal shortly after.
  Tracking via Dependabot — no action needed unless upstream releases lag.

### Removed
<!-- placeholder -->

### Fixed
- `docs/book.toml`: dropped the deprecated `multilingual` key (rejected by
  current mdBook). Docs site now builds and deploys to GitHub Pages on push.

### IR
<!-- placeholder; add a row to SPEC §9.1 if IR_VERSION bumps -->

### Wire
<!-- placeholder -->

---

## [0.1.0] - 2026-05-07

The first stable release. Phases 0–5 of the [roadmap](./ROADMAP.md) have all
landed; the API is stable enough that we now follow strict semver for
breaking changes (within the 0.x lifetime, breaking changes still bump the
minor — wait for 1.0 for the strictest interpretation).

### Highlights

- End-to-end type-safe RPC between Rust (axum) and TypeScript clients.
- Single source of truth: Rust types own the contract; codegen produces a
  static `api.gen.ts` with no runtime schema fetch.
- Server-side: `#[rpc]` and `#[rpc(stream)]` macros, `Router::layer(...)` for
  middleware, `Router::procedure(...)` for typed registration.
- Validation bridge: `#[derive(Validate)]` macro, server-side enforcement,
  Valibot/Zod schema codegen, client-side pre-send + post-receive validation.
- Subscriptions: SSE by default, WebSocket behind the `ws` feature, mapped to
  `AsyncIterable<T>` on the TypeScript side.
- CLI: `cargo taut gen` (codegen), `cargo taut check` (drift), `cargo taut
  inspect` (human-readable IR), `cargo taut mcp` (Model Context Protocol
  manifest).
- Documentation: a complete mdBook with concepts, guides, tutorials, and
  reference at <https://nktkt.github.io/taut-rpc>.
- Examples: smoke (Phase 0), phase1–phase4-validate (covering each phase's
  features), todo-react (Vite + React full-stack), counter-sveltekit
  (SvelteKit full-stack).

### Versioning policy

- Crate version: `0.1.0` for `taut-rpc`, `taut-rpc-macros`, `taut-rpc-cli`.
- npm package: `0.1.0` for `taut-rpc`.
- IR_VERSION: `1`. (Phase 4 bumped from 0 to 1.)
- Wire format: subscription frames are v0; queries/mutations have no version
  header (the envelope is intentionally simple).

### Breaking changes from 0.0.0

- `IR_VERSION` bumped from 0 to 1 — old `target/taut/ir.json` files will be
  rejected by `cargo taut gen`. Re-run `cargo run` (with the binary's
  `dump_if_requested` step) to regenerate.
- `ProcedureDescriptor.handler` field renamed to `body` and re-typed to
  `ProcedureBody { Unary | Stream }`.

---

## Pre-0.1.0 development log

The sections below were written incrementally during phases 1–4 and the
agent-tools side-track. They are kept for historical reference but are not
edited going forward; consolidated highlights for the 0.1.0 release are
above.

## [Unreleased] — Phase 4

### Added
- `#[derive(Validate)]` proc-macro that emits `impl Validate for X` from `#[taut(...)]`
  per-field attributes: `min`, `max`, `length(min, max)`, `pattern`, `email`, `url`,
  `custom`. The trait collects all violations into `Vec<ValidationError>`.
- Server-side input validation: `#[rpc]` and `#[rpc(stream)]` now insert a
  `<Input as Validate>::validate(&input)` call after deserialization. Failures are
  emitted as `{"err":{"code":"validation_error","payload":{"errors":[...]}}}` with
  HTTP 400 (or as one SSE `event: error` frame for subscriptions).
- Codegen: with the default `--validator valibot`, the generated `api.gen.ts` now
  exports a `<Type>Schema` per IR TypeDef plus a `procedureSchemas` const map.
  `--validator zod` emits Zod schemas. `--validator none` skips schema emission.
- npm runtime: `ClientOptions.schemas` (typically passed `procedureSchemas`) and
  `validate: { send, recv }` toggles. Pre-send and post-receive parsing throws
  `TautError("validation_error", ...)` on mismatch.
- `StandardError::ValidationFailed { errors: Vec<ValidationError> }` variant
  (code = "validation_error", status 400).
- Validate trait blanket impls for primitives, `()`, `Option<T>`, `Vec<T>` so
  that any type usable as RPC input satisfies the trait without an explicit derive.
- New runtime helpers in `validate::check`: `pattern` (compiles regex via `regex`
  crate), generalised `min`/`max` for any `Into<f64> + Copy` numeric.
- Helpers `validate::run` and `validate::collect` for macro emission.
- Phase 4 example: `examples/phase4-validate/` — form validation with both
  client-side (Valibot) and server-side enforcement.
- Documentation: `docs/src/concepts/validation.md` rewritten and a new
  `docs/src/guides/validation.md` cookbook.

### Changed
- IR shape: `Field` gained `constraints: Vec<Constraint>` (filled by `#[derive(Type)]`
  from the same `#[taut(...)]` attrs Validate reads). Codegen renders these into
  Valibot/Zod schemas.
- `IR_VERSION` bumps from 0 → 1.
- Phase 1/2/3 examples updated to derive `Validate` so they remain compatible
  with the new macro emission.

### IR
- IR_VERSION = 1. New `Field.constraints` field. Old IR JSON without this field
  is rejected by `cargo taut gen` with a clear error message.

### Wire
- New error code `validation_error` with payload shape
  `{ "errors": [{ "path": "...", "constraint": "...", "message": "..." }] }`.
- For subscriptions: validation failures emit a single `event: error` frame then
  `event: end` (no SSE-level retry).

---

## [Unreleased] — Agent tools

### Added
- `cargo taut mcp` subcommand emits an MCP (Model Context Protocol) `tools/list`
  manifest from the IR. Each query/mutation procedure becomes a tool whose
  `inputSchema` is JSON Schema (Draft 2020-12), with reachable named types
  inlined as `$defs` and rustdoc surfaced as `description`. Subscriptions are
  skipped by default; pass `--include-subscriptions` to include them.
- `taut_rpc_cli::mcp` library module exposes `render_manifest(&Ir, &McpOptions)`
  for in-process callers (integration tests, build scripts, custom tooling).
- `--from-binary` flow mirrors `cargo taut gen` so the manifest can be produced
  straight from a compiled binary via `taut_rpc::dump_if_requested`.

### IR
- IR_VERSION still 0. The MCP emitter is a pure consumer of the existing IR
  shape — no field changes, no version bump.

### Wire
- No change.

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

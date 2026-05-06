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

## [Unreleased]

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

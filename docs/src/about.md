# About taut-rpc

End-to-end type-safe RPC for Rust + TypeScript. **v0.1.0 released
2026-05-07.**

`taut-rpc` lets a Rust server and a TypeScript client share one description
of every procedure, every type, and every validation rule. Annotate a
function with `#[rpc]`, run `cargo taut codegen`, and the frontend gets a
typed client that mirrors the backend exactly. Refactor a Rust signature,
get a TypeScript compile error — no schema fetch at boot, no hand-written
DTOs, no drift.

## Project status

The project is in active development. Phase 5 (DX polish, docs, examples)
is the last gate before the `0.1.0` cut on crates.io and npm. Subsequent
releases follow the [versioning policy](#versioning-policy) below.

| Surface | State |
|---|---|
| `taut-rpc-macros`, `taut-rpc-core`, `taut-rpc-ir` | stable for `0.1` |
| `cargo-taut` (codegen, check, inspect) | stable for `0.1` |
| `@taut-rpc/client`, `@taut-rpc/runtime` | stable for `0.1` |
| MCP bridge | preview; opt-in feature |
| WebSocket transport | preview; SSE is the default |

## Authors and maintainers

`taut-rpc` is maintained by a small group of contributors. See the
[`README`](https://github.com/taut-rpc/taut-rpc/blob/main/README.md) for
the current maintainer list and the
[contributors page on GitHub](https://github.com/taut-rpc/taut-rpc/graphs/contributors)
for the full author log.

If you want to contribute, start with
[`CONTRIBUTING.md`](https://github.com/taut-rpc/taut-rpc/blob/main/CONTRIBUTING.md).
Bug reports, type-mapping edge cases, and migration stories from rspc or
tRPC are especially welcome.

## License

`taut-rpc` is dual-licensed under either of:

- [MIT license](https://github.com/taut-rpc/taut-rpc/blob/main/LICENSE-MIT)
- [Apache License, Version 2.0](https://github.com/taut-rpc/taut-rpc/blob/main/LICENSE-APACHE)

at your option. This is the standard Rust ecosystem dual license; pick
whichever fits your project.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in `taut-rpc` by you, as defined in the Apache-2.0
license, shall be dual-licensed as above, without any additional terms or
conditions.

## Sponsorship

There is no sponsorship arrangement yet. If GitHub Sponsors is enabled in
the future, a link will appear here and on the repository page. In the
meantime, the most useful kinds of support are:

- Filing detailed bug reports with a minimal repro.
- Sharing real-world IRs (`cargo taut inspect --json`) so the type-mapping
  test corpus stays honest.
- Writing migration notes when moving an existing rspc or tRPC service
  over.

## Acknowledgments

`taut-rpc` stands on the shoulders of several projects, and the design owes
each of them a debt:

- **[rspc](https://github.com/specta-rs/rspc)** — the closest neighbour in
  the Rust ecosystem. `taut-rpc` borrows the idea of a typed Rust router
  whose shape is exported to TypeScript, and tries to take a slightly
  narrower, more opinionated path: a single IR, a single codegen target
  per validator, and a strict `IR_VERSION` check.
- **[tRPC](https://trpc.io/)** — the canonical end-to-end-typed RPC
  experience in the JS world. The client API surface, the
  query/mutation/subscription split, and the "no schema at runtime"
  philosophy are all directly inspired by tRPC.
- **[axum](https://github.com/tokio-rs/axum)** — the default integration
  target on the Rust side. The `tower::Layer` middleware story in
  `taut-rpc` is a thin shell over axum's, deliberately, so existing
  middleware composes.
- **[valibot](https://valibot.dev/)** and **[zod](https://zod.dev/)** —
  the two TypeScript validators `taut-rpc` targets. The `#[derive(Validate)]`
  bridge would not be possible without their excellent schema primitives.
- **[serde](https://serde.rs/)** — the substrate everything serialises
  through. The wire format is "whatever serde-json does", and the IR
  field metadata leans on serde's attribute model.

If you find a project that should be acknowledged here and is not, please
open a PR.

## Trademark

There is no trademark claim on the name "taut-rpc". The name was chosen
because it appeared to be unique within the Rust and JavaScript package
ecosystems at the time of the `0.1.0` release. You are free to reference
the project by name in articles, talks, and derivative work.

## Code of conduct

This project follows a contributor code of conduct adapted from the
Contributor Covenant. See
[`CODE_OF_CONDUCT.md`](https://github.com/taut-rpc/taut-rpc/blob/main/CODE_OF_CONDUCT.md)
in the repository root.

In short: be kind, assume good faith, and report incidents to the
maintainer email listed in that file. Reports are handled privately.

## Security

If you find a security issue — for example, a way to bypass validation, a
way to crash a server with a crafted IR, or a code-injection path through
the TypeScript codegen — please **do not** open a public GitHub issue.

Instead, follow the process in
[`SECURITY.md`](https://github.com/taut-rpc/taut-rpc/blob/main/SECURITY.md).
That document lists the supported versions, the disclosure address, and
the expected response window.

## Versioning policy

`taut-rpc` follows [Semantic Versioning](https://semver.org/) once `0.1.0`
ships. Concretely:

- **Pre-1.0 (the `0.x` line).** A bump in the `y` of `0.y.z` may include
  breaking changes. Patch releases (`0.y.z` → `0.y.z+1`) are
  bug-fix-only and are safe to take without code review.
- **Post-1.0.** Breaking changes only happen on a major bump. Minor
  releases are additive; patch releases are bug-fix-only.
- **`IR_VERSION`.** The IR shape is part of the public surface. Any
  change to `IR_VERSION` is a breaking change and bumps the appropriate
  semver component. `cargo taut check` will refuse to operate against a
  client built for a different `IR_VERSION`, so a mismatch fails CI
  rather than silently producing a broken client.
- **MSRV (Minimum Supported Rust Version).** The MSRV is documented in
  the workspace `Cargo.toml`. Bumping the MSRV is treated as a
  minor-version change pre-1.0 and a major-version change post-1.0.
- **Deprecation.** Items are marked `#[deprecated]` for at least one
  minor cycle before removal. The deprecation message points at the
  replacement.

If you are pinning `taut-rpc` in a production codebase, pin both the Rust
crate and the npm package to the same `0.y` line and upgrade them
together — the IR check enforces this anyway, but it is easier to plan
for.

# Contributing to taut-rpc

Welcome, and thanks for considering a contribution.

## Project status

`taut-rpc` is at **Day 0**. The spec is a draft, the roadmap is aspirational, and most of the code does not exist yet. Expect things to move fast and expect breaking changes — APIs, IR shape, wire format, file layout, and even crate names are all in play until 0.1.0 ships.

If you are looking for a stable library, come back later. If you are interested in shaping the design, this is exactly the right time.

Read [`README.md`](./README.md), [`SPEC.md`](./SPEC.md), and [`ROADMAP.md`](./ROADMAP.md) before sending substantive changes. Most contribution friction comes from skipping the spec.

## Quick checklist

Before opening a PR, the short version of everything below:

1. Read `SPEC.md` if your change touches behavior on the wire, the IR, or the macro surface.
2. Run the full local check (`cargo fmt`, `cargo clippy -D warnings`, `cargo test --workspace`, plus `npm run typecheck` if you touched TS).
3. Update `SPEC.md` and `CHANGELOG.md` *in the same PR* if applicable.
4. Bump `IR_VERSION` if you changed the IR shape.
5. Add or update a regression test.

If any of those slip, say so in the PR description so reviewers know it is intentional.

## Repository layout

The workspace contains every component of the project. Top-level directories:

- `crates/taut-rpc` — runtime library (the public crate users depend on).
- `crates/taut-rpc-macros` — proc macros (`#[rpc]`, `#[derive(Type)]`, `#[derive(Validate)]`, `#[derive(TautError)]`).
- `crates/taut-rpc-cli` — the `cargo taut` subcommand (`gen`, `check`, `inspect`).
- `npm/taut-rpc` — the TypeScript runtime package shipped to npm.
- `examples/` — runnable examples; today only the Phase 0 smoke test.
- `docs/` — mdBook design docs (spec, tutorials, ADRs).

Anything outside these directories should be small (CI config, license files, top-level docs) and should have a clear reason to exist.

## Development setup

You need both a Rust and a Node toolchain:

- **Rust 1.75+** via [rustup](https://rustup.rs), stable toolchain. We do not use nightly features.
- **Node 20+** for the npm package. Older Node may work but is not tested.

Recommended (optional) tools:

- [`cargo-watch`](https://github.com/watchexec/cargo-watch) for re-running tests on save.
- [`cargo-nextest`](https://nexte.st/) for faster, prettier test output.
- [`just`](https://github.com/casey/just) — a `justfile` may appear with common recipes; until then, the commands below are the contract.

You do not need Docker, a database, or any cloud account to develop or test.

## Common commands

The full local check that mirrors CI:

```sh
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

For the TypeScript runtime:

```sh
cd npm/taut-rpc && npm install && npm run typecheck && npm run build
```

The Phase 0 end-to-end smoke (one terminal each):

```sh
# terminal 1
cd examples/smoke/server && cargo run

# terminal 2
cd examples/smoke/client && npm install && npm run start
```

If either side panics or the TS client cannot reach the server, that is a bug — file an issue with both terminals' output.

## Workflow

- **Fork and branch.** Branch off `main`, name it descriptively (`feat/ir-version-bump`, `fix/macro-span-on-generics`).
- **PRs target `main`.** We do not currently maintain release branches; everything lands on `main` and is tagged for release.
- **Spec changes ride with the code.** If a PR changes runtime behavior, wire format, or IR shape, the matching update to `SPEC.md` belongs in the same PR. The PR template asks you to confirm this; reviewers will request changes if the spec and the code disagree.
- **IR shape bumps are explicit.** Any change to the IR schema requires bumping `IR_VERSION` in `crates/taut-rpc/src/lib.rs` *and* adding a `CHANGELOG.md` entry under the `Unreleased — IR` section. Reviewers will block on a missing entry. Codegen refuses mismatched IR versions, so this is not a documentation-only step — old `target/taut/ir.json` files become invalid the moment the bump lands.
- **One commit per logical change.** Rebase to clean up before requesting review. We squash on merge for trivial PRs but prefer reviewable history for larger ones — particularly anything touching the macro or codegen.
- **Sign-off encouraged.** A `Signed-off-by:` trailer (`git commit -s`) is welcome but not required. It signals you have read the DCO-style intent of the dual-license declaration below.

PR titles follow Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`). The prefix is read by the changelog tooling.

### Review expectations

- Small, focused PRs land faster than large ones. If a change naturally splits into "refactor" + "feature", split it.
- CI must be green before review. If CI is flaky, file an issue rather than re-running until it passes.
- We will leave inline comments rather than closing PRs over disagreements; expect dialogue, not a verdict.
- Drafts are welcome — open one early to get directional feedback before writing the test matrix.

## Code style

### Rust

- `cargo fmt` clean. We use the default rustfmt config.
- `cargo clippy --all-targets -- -D warnings` clean. Suppress with `#[allow(...)]` only with a comment explaining why.
- Doc comments: `///` on every public item; `//!` at the top of modules with non-trivial responsibility.
- Never call `unwrap()` outside tests. `expect("...")` with a message that explains why the invariant holds is acceptable. In proc macros, return `syn::Error` with a useful span instead — user-facing macro errors are part of the UX.
- No `unsafe`. The workspace `Cargo.toml` sets `unsafe_code = "forbid"`; do not weaken it.

### TypeScript

- `tsc --strict` is the linter. We may add ESLint later; until then, rely on the type checker and good taste.
- Prefer `unknown` over `any`. Narrow before use.
- Public exports of `npm/taut-rpc` are an API contract; treat them like Rust public items. Breaking changes there must show up in the changelog.
- The runtime ships ESM only. Do not add CommonJS shims unless we explicitly decide to.

## Tests

- **Unit tests** live next to the code under test, in a `#[cfg(test)] mod tests` block. Keep them small and fast — the workspace test run is part of every CI invocation.
- **Integration tests across crates** live in `crates/taut-rpc/tests/`. Use these for anything that exercises the macro -> IR -> codegen pipeline as a whole.
- **Codegen tests (Phase 1+):** use [`trybuild`](https://docs.rs/trybuild) for compile-fail cases (especially macro diagnostics that target the user's span) and [`insta`](https://insta.rs/) for snapshotting generated TypeScript. Snapshots live next to their tests; review snapshot diffs as part of the PR and explain non-obvious changes in the PR description.
- **End-to-end:** the Phase 0 smoke is the canonical e2e harness. New transports and procedure shapes should grow it rather than spawn parallel harnesses.

If a bug ships without a regression test, the next PR should add one. "Cannot reproduce in a unit test" is a useful answer; "did not try" is not.

## Reporting bugs and proposing features

- Use the GitHub issue templates. They prompt for the things reviewers always need (versions, repro, expected vs. actual).
- For proposals that touch the spec, open an issue with the `spec` label *first* and let discussion happen there before writing code. The roadmap exists so you can see whether a feature is in scope for the current phase.
- Security issues: please follow `SECURITY.md` (private disclosure) rather than the public tracker.

## Scope boundaries

Some things this project will not do, restated from the README's non-goals:

- **Rust and TypeScript only.** No Go, Python, Java, or polyglot client generation. The whole design assumes the client and server share a type-system mental model; broadening that breaks the value proposition.
- **Not gRPC.** If you want gRPC, use [`tonic`](https://github.com/hyperium/tonic). We will not add `.proto` ingestion.
- **Not OpenAPI-first.** Rust types are the source of truth. A *thin OpenAPI emitter* is a speculative post-0.1 idea, but contract-first workflows are out of scope.

PRs that try to expand scope into these areas will be closed with a pointer here. Open an issue first if you think the boundary should move.

## Licensing

All contributions are dual-licensed under **MIT OR Apache-2.0** unless explicitly marked otherwise in the file header. The two license texts live at [`LICENSE-MIT`](./LICENSE-MIT) and [`LICENSE-APACHE`](./LICENSE-APACHE) at the repo root.

By submitting a pull request, you agree to license your contribution under both licenses, and you assert that you have the right to do so. If your employer has IP claims on your contributions, get clearance before submitting.

We do not require a CLA. The dual-license declaration in your PR is sufficient.

Thanks for reading this far — and for helping make the wire taut.

# Contributing to taut-rpc

Welcome, and thanks for considering a contribution.

## Project status

`taut-rpc` is **approaching v0.1.0**. The macro pipeline, IR, codegen, and both runtimes (Rust and TypeScript) are in place; HTTP and WebSocket transports both pass their end-to-end suites; docs, examples, and CI are wired up. What remains before tagging is the final polish pass: changelog grooming, doc cross-checks, and the release-workflow dry run.

Things will still move, but the shape is settled. Breaking changes between now and 0.1.0 will be small and documented in `CHANGELOG.md`. Post-0.1.0 we follow semver — IR shape and wire format both count.

If you are looking for a stable library, 0.1.0 is the right place to start. If you want to shape the design further, this is still a good moment to weigh in: every public API decision is reversible until the tag lands.

Read [`README.md`](./README.md), [`SPEC.md`](./SPEC.md), and [`ROADMAP.md`](./ROADMAP.md) before sending substantive changes. Most contribution friction comes from skipping the spec.

## Phases shipped

A short tour of how we got here, in case it helps locate code:

- **Phase 0 — smoke.** End-to-end "hello world" over HTTP: a hand-rolled server in `examples/smoke/server` and a fetch-based TS client. No macro yet; the goal was to prove the wire shape.
- **Phase 1 — macro and IR.** `#[rpc]` and the `Type`/`Validate`/`TautError` derives in `crates/taut-rpc-macros`. IR (`crates/taut-rpc/src/ir.rs`) emitted as `target/taut/ir.json` with `IR_VERSION` gating.
- **Phase 2 — codegen and CLI.** `cargo taut gen` produces TypeScript clients from IR; `cargo taut check` and `cargo taut inspect` round out the CLI. Snapshot tests via `insta` keep generated output honest.
- **Phase 3 — Rust runtime.** Server-side `Router`, error mapping, and the HTTP transport in `crates/taut-rpc`. `trybuild` covers macro diagnostics.
- **Phase 4 — TS runtime and WebSocket.** `npm/taut-rpc` ships the typed client; the `ws` feature on the Rust side and the matching TS transport carry bidirectional procedures.
- **Phase 5 — release prep.** Docs (mdBook under `docs/`), CHANGELOG, README, this file, and the GitHub Actions release workflow. No public-API additions in this phase by design.

## Quick checklist

Before opening a PR, the short version of everything below:

1. Read `SPEC.md` if your change touches behavior on the wire, the IR, or the macro surface.
2. Run the full local check (`cargo fmt`, `cargo clippy -D warnings`, `cargo nextest run --workspace` or `cargo test --workspace`, `cargo test --features ws` for WebSocket coverage, plus `npm run typecheck` if you touched TS).
3. Update `SPEC.md` and `CHANGELOG.md` *in the same PR* if applicable.
4. Bump `IR_VERSION` if you changed the IR shape (see "IR snapshot" below).
5. Add or update a regression test.

If any of those slip, say so in the PR description so reviewers know it is intentional.

## Repository layout

The workspace contains every component of the project. Top-level directories:

- `crates/taut-rpc` — runtime library (the public crate users depend on). Hosts the HTTP transport unconditionally; the WebSocket transport lives behind the `ws` feature.
- `crates/taut-rpc-macros` — proc macros (`#[rpc]`, `#[derive(Type)]`, `#[derive(Validate)]`, `#[derive(TautError)]`).
- `crates/taut-rpc-cli` — the `cargo taut` subcommand (`gen`, `check`, `inspect`).
- `npm/taut-rpc` — the TypeScript runtime package shipped to npm.
- `examples/` — runnable examples; the Phase 0 smoke plus the WebSocket chat demo.
- `docs/` — mdBook design docs (concepts, guides, tutorials, ADRs, reference).

Anything outside these directories should be small (CI config, license files, top-level docs) and should have a clear reason to exist.

## Development setup

You need both a Rust and a Node toolchain:

- **Rust 1.85+** via [rustup](https://rustup.rs), stable toolchain. We do not use nightly features.
- **Node 20+** for the npm package. Older Node may work but is not tested.

Recommended tools (the first two are now used in CI; install them locally to mirror it):

- [`cargo-nextest`](https://nexte.st/) — the canonical test runner. CI runs `cargo nextest run`; using it locally avoids "works on my machine" surprises around test isolation.
- [`mdbook`](https://rust-lang.github.io/mdBook/) — required if you touch `docs/`. Install with `cargo install mdbook`.
- [`cargo-watch`](https://github.com/watchexec/cargo-watch) for re-running tests on save.
- [`just`](https://github.com/casey/just) — a `justfile` may appear with common recipes; until then, the commands below are the contract.

You do not need Docker, a database, or any cloud account to develop or test.

## Common commands

The full local check that mirrors CI:

```sh
cargo fmt --all \
  && cargo clippy --workspace --all-targets --all-features -- -D warnings \
  && cargo nextest run --workspace \
  && cargo test --workspace --features ws
```

If you do not have `cargo-nextest` installed, `cargo test --workspace` is an acceptable substitute; CI runs nextest, so output will differ slightly.

The `--features ws` line is not optional — the WebSocket transport's tests are gated behind it, and skipping that step is how regressions slip into the bidirectional path.

For the TypeScript runtime:

```sh
cd npm/taut-rpc && npm install && npm run typecheck && npm run test && npm run build
```

The Phase 0 end-to-end smoke (one terminal each):

```sh
# terminal 1
cd examples/smoke/server && cargo run

# terminal 2
cd examples/smoke/client && npm install && npm run start
```

The WebSocket chat example follows the same shape under `examples/chat/`; see its README for the exact commands.

If either side panics or the TS client cannot reach the server, that is a bug — file an issue with both terminals' output.

### Benchmarks

Criterion microbenchmarks live under `crates/taut-rpc/benches/` (currently just the handler dispatch path in `dispatch.rs`). They are **not** run in CI because criterion runs are slow and noisy on shared runners. Run them locally when you suspect a hot-path regression:

```sh
cargo bench -p taut-rpc
```

Criterion writes baselines to `target/criterion/`; commit-relevant numbers belong in the PR description, not in the repo.

## Workflow

- **Fork and branch.** Branch off `main`, name it descriptively (`feat/ir-version-bump`, `fix/macro-span-on-generics`).
- **PRs target `main`.** We do not currently maintain release branches; everything lands on `main` and is tagged for release.
- **Spec changes ride with the code.** If a PR changes runtime behavior, wire format, or IR shape, the matching update to `SPEC.md` belongs in the same PR. The PR template asks you to confirm this; reviewers will request changes if the spec and the code disagree.
- **One commit per logical change.** Rebase to clean up before requesting review. We squash on merge for trivial PRs but prefer reviewable history for larger ones — particularly anything touching the macro or codegen.
- **Sign-off encouraged.** A `Signed-off-by:` trailer (`git commit -s`) is welcome but not required. It signals you have read the DCO-style intent of the dual-license declaration below.

PR titles follow Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `chore:`). The prefix is read by the changelog tooling.

### Review expectations

- Small, focused PRs land faster than large ones. If a change naturally splits into "refactor" + "feature", split it.
- CI must be green before review. If CI is flaky, file an issue rather than re-running until it passes.
- We will leave inline comments rather than closing PRs over disagreements; expect dialogue, not a verdict.
- Drafts are welcome — open one early to get directional feedback before writing the test matrix.

## IR snapshot

The IR is the contract between the macro and the codegen, and `target/taut/ir.json` is its on-disk form. It has its own version number because changing its shape silently would break every downstream client.

When you change the IR shape:

1. **Bump `IR_VERSION`** in `crates/taut-rpc/src/lib.rs`. The codegen refuses mismatched versions, so old `ir.json` files become invalid the moment the bump lands — that is the intended behavior, not a problem to work around.
2. **Add a row to `SPEC.md` §9.1** (the IR version table) describing what changed. One line is enough; link to the PR for context.
3. **Write a migration note in `CHANGELOG.md`** under the current `Unreleased` section, in the `IR` subsection. Tell readers what they will see if they hit the mismatch and what to do (typically: re-run `cargo taut gen`).

Reviewers will block on a missing entry in any of those three places. The cost of a stale snapshot is paid by every user, not by the PR author, so we are strict here.

## Code style

### Rust

- `cargo fmt` clean. We use the default rustfmt config.
- `cargo clippy --all-targets --all-features -- -D warnings` clean. Suppress with `#[allow(...)]` only with a comment explaining why.
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
- **Integration tests across crates** live in `crates/taut-rpc/tests/`. Use these for anything that exercises the macro -> IR -> codegen pipeline as a whole. WebSocket-specific cases live alongside HTTP cases but are gated with `#[cfg(feature = "ws")]`.
- **Codegen tests:** [`trybuild`](https://docs.rs/trybuild) for compile-fail cases (especially macro diagnostics that target the user's span) and [`insta`](https://insta.rs/) for snapshotting generated TypeScript. Snapshots live next to their tests; review snapshot diffs as part of the PR and explain non-obvious changes in the PR description.
- **End-to-end:** the Phase 0 smoke is the canonical HTTP harness; the chat example covers the WebSocket path. New transports and procedure shapes should grow these rather than spawn parallel harnesses.

If a bug ships without a regression test, the next PR should add one. "Cannot reproduce in a unit test" is a useful answer; "did not try" is not.

## Documentation contributions

The mdBook lives under `docs/` and is the canonical place for prose. Source pages are markdown under `docs/src/`. Decide where a page belongs:

- `docs/src/concepts/` — explanation. "What is the IR?", "Why does the wire format look like this?", "How do error spans work?". Aim for context and motivation, not step-by-step instructions.
- `docs/src/guides/` — task-oriented. "Add a new procedure", "Migrate an IR version", "Configure the WebSocket transport". One job per page, linkable from a runbook.
- `docs/src/tutorials/` — narrative, end-to-end. The reader follows along and ends with something that runs. Keep these few; they age fastest.
- `docs/src/reference/` — generated or near-generated material (CLI flags, IR schema, error codes). If you find yourself writing prose here, consider whether it belongs in `concepts/` instead.
- `docs/src/adr/` — architecture decision records. Append-only; do not retroactively edit a merged ADR.

To preview locally:

```sh
mdbook serve docs
```

Then open <http://localhost:3000>. `mdbook serve` watches the source and rebuilds on save. Run `mdbook build docs` once before pushing — CI runs the same and will fail on broken links.

API docs (`cargo doc`) and TS API docs (TypeDoc) are generated separately and published by the release workflow; you do not need to run them locally unless you are debugging the published output.

## Releasing

Releases are mostly automated. The short version:

1. Land all changes for the release on `main`. Make sure `CHANGELOG.md` has a real (non-`Unreleased`) section for the version.
2. Bump versions in `Cargo.toml` files and `npm/taut-rpc/package.json` in a single PR.
3. Once that PR merges, tag `main`: `git tag v0.1.0 && git push origin v0.1.0`.
4. The GitHub Actions release workflow takes over: it runs the full test matrix, then publishes to crates.io (`cargo publish` for each crate in dependency order) and to npm (`npm publish` for `npm/taut-rpc`), and finally drafts a GitHub Release with the changelog excerpt.
5. If the workflow fails partway through, fix forward — do not delete the tag. The workflow is idempotent for already-published versions and will skip them on rerun.

You need release-team permissions to push tags. If you have a release-worthy change but not the permissions, ping a maintainer in the PR.

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

All contributions are dual-licensed under **MIT OR Apache-2.0**, at the user's option, unless explicitly marked otherwise in the file header. The two license texts live at [`LICENSE-MIT`](./LICENSE-MIT) and [`LICENSE-APACHE`](./LICENSE-APACHE) at the repo root, and every published crate and the npm package carry the same dual designation in their manifests.

By submitting a pull request, you agree to license your contribution under both licenses, and you assert that you have the right to do so. If your employer has IP claims on your contributions, get clearance before submitting.

We do not require a CLA. The dual-license declaration in your PR is sufficient.

Thanks for reading this far — and for helping make the wire taut.

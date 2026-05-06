# Architecture

> Placeholder chapter. The canonical description lives in
> [SPEC §2](../reference/spec.md). This page exists to give a reader a
> two-paragraph orientation before diving into the spec.

## The three crates

`taut-rpc` ships as three Cargo crates with sharply separated roles. The
top-level `taut-rpc` crate is the public API a server author touches: it
exposes `Router`, the runtime wiring that turns registered procedures into
an `axum::Router`, and re-exports of derive macros so users only need one
dependency. The `taut-rpc-macros` crate is the proc-macro half: `#[rpc]` on
a function or trait method, plus `#[derive(Type)]` and `#[derive(Validate)]`
on user types. Macros never read the network or generate TypeScript
themselves — their only job is to emit an axum-compatible handler and a
fragment of the IR. Finally, `taut-rpc-cli` provides the
`cargo taut` subcommand with three verbs: `gen` (read IR, emit `.ts`),
`check` (detect IR drift in CI), and `inspect` (render IR as a human
table).

## Where the IR fits

The intermediate representation is the seam between macro time and codegen
time. At compile time, the macros write type and procedure descriptors into
`target/taut/ir.json`; at codegen time, `cargo taut gen` reads that file
and emits a `.ts` file. The IR is JSON, schema-versioned with an
`ir_version` field, and explicitly designed to be the *most stable surface*
in the project — once 0.1 ships, the IR shape is harder to change than the
Rust API, because it is what every generated client depends on. This split
means codegen is a pure function of the IR: no `cargo expand`, no rustc
linkage, no proc-macro re-execution. CI can check IR drift without
rebuilding the world.

## See also

- [SPEC §2 — Architecture](../reference/spec.md)
- [The IR](./ir.md)
- [Wire format](./wire-format.md)

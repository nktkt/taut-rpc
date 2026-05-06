# The IR

> Placeholder chapter. See [SPEC §2 — Architecture](../reference/spec.md)
> and [SPEC §9 — Compatibility & versioning](../reference/spec.md) for the
> canonical definition.

## The contract between macro emission and codegen

The IR — intermediate representation — is the contract between macro
emission and codegen. The proc-macro half of `taut-rpc` does not know
TypeScript exists; the CLI half does not know `syn` exists. They meet at a
JSON file. This split is deliberate. It keeps macro execution cheap (no
type system mirroring at compile time), it keeps codegen
hermetic (a pure function of the IR), and it makes the IR itself a stable,
inspectable artifact: you can `cat target/taut/ir.json` to see exactly what
your server claims to expose.

The IR carries two kinds of entries: **type descriptors** (struct shapes,
enum variants, validation constraints) and **procedure descriptors**
(name, input type, output type, error type, transport mode). Generics are
recorded in their *instantiated*, monomorphic form: a never-instantiated
generic never appears in the IR. The IR has an `ir_version` field; codegen
refuses mismatches rather than silently producing skewed output.

## Data flow

```
            ┌──────────────────────┐
  Rust src ─┤ #[rpc], #[derive]    │
            │ proc-macros          │
            └──────────┬───────────┘
                       │ emit
                       ▼
            ┌──────────────────────┐
            │ axum handler stub    │ ──► linked into your binary
            │ + IR fragment        │
            └──────────┬───────────┘
                       │ build.rs collects fragments
                       ▼
            ┌──────────────────────┐
            │ target/taut/ir.json  │ ◄── stable, schema-versioned artifact
            └──────────┬───────────┘
                       │ cargo taut gen
                       ▼
            ┌──────────────────────┐
            │ src/api.gen.ts       │ ──► imported by your TS client
            └──────────────────────┘
```

## Why a file, not a procedural pipeline

A persisted JSON file gives three concrete wins. First, `cargo taut check`
in CI can compare the committed `.gen.ts` against a fresh IR without
recompiling Rust — it just runs codegen and diffs. Second, the IR is the
unit of versioning: external tooling (a documentation site, an OpenAPI
emitter, a devtools panel) can read it without depending on
`taut-rpc-macros`. Third, debugging is straightforward: when codegen
produces unexpected TypeScript, the IR tells you whether the macro saw
what you thought it saw.

## See also

- [SPEC §2 — Architecture](../reference/spec.md)
- [SPEC §9 — Compatibility & versioning](../reference/spec.md)
- [Architecture](./architecture.md)

# CLI reference

The `taut-rpc-cli` crate ships a single binary, `cargo-taut`, that drives every
codegen and IR-tooling task: generating a TypeScript client, detecting drift in
CI, inspecting the IR, and emitting an MCP manifest. This page documents every
subcommand, every flag, and the IR-dump protocol that ties them together.

## Install

```bash
cargo install taut-rpc-cli
```

The published crate is `taut-rpc-cli`, but the binary it installs is
`cargo-taut`. Cargo discovers any binary named `cargo-<name>` on `PATH` and
exposes it as a subcommand, so once installed you can invoke it either way:

```bash
cargo taut gen --validator valibot   # via Cargo's subcommand mechanism
cargo-taut gen --validator valibot   # direct invocation, identical behaviour
```

The binary normalises both forms internally — when Cargo dispatches the
subcommand it passes `taut` as the first positional argument, which the CLI
strips before parsing. There is no separate `taut` crate to install.

Every subcommand operates on the IR (intermediate representation) emitted by
the `#[rpc]` and `#[derive(Type)]` proc-macros at compile time. The IR lives at
`target/taut/ir.json` by default; see [The IR](../concepts/ir.md) for its
schema.

## `cargo taut gen`

Generates a typed TypeScript client from the IR. This is the command you wire
into your build: every time the Rust API surface changes, re-run `gen` and
commit the regenerated `api.gen.ts`.

| Flag | Default | Description |
|---|---|---|
| `--ir <PATH>` | `target/taut/ir.json` | Path to the IR JSON file produced by the proc-macros. Mutually exclusive with `--from-binary`. |
| `--from-binary <PATH>` | — | Path to a compiled user binary. Spawns it with `TAUT_DUMP_IR` set so it dumps the IR for you and exits before binding any port. Mutually exclusive with `--ir`. |
| `--out <PATH>` | `src/api.gen.ts` | Path the generated TypeScript client is written to. Parent directories are created. |
| `--validator <KIND>` | `valibot` | Validator runtime to target: `valibot`, `zod`, or `none`. `valibot` is the v0.1 default; `zod` is opt-in for users already on Zod; `none` skips validator emission and emits pure types. |
| `--bigint <STRATEGY>` | `native` | How to render `u64`/`i64`/`u128`/`i128`: `native` (TypeScript `bigint`) or `as-string` (decimal string). Alias for `--bigint-strategy`. |

Example — generate a Valibot client straight from a built binary:

```bash
cargo taut gen --from-binary ./target/release/my-app --out web/src/api.gen.ts
```

## `cargo taut check`

Detects IR drift between the current build and a committed baseline. The
intended workflow:

1. Run `cargo taut check --write` once to bootstrap a snapshot.
2. Commit `taut/ir.snapshot.json` to git alongside your `Cargo.toml`.
3. In CI, after `cargo build`, run `cargo taut check`. If the IR has drifted
   from the committed snapshot, the command exits non-zero with a unified-diff
   summary of which procedures and types were added, removed, or changed.

This catches the "I changed the Rust API but forgot to re-run codegen" class
of bug at the PR boundary, before a stale `api.gen.ts` ships to the frontend.

| Flag | Default | Description |
|---|---|---|
| `--ir <PATH>` | `target/taut/ir.json` | Path to the current IR JSON file. Mutually exclusive with `--from-binary`. |
| `--from-binary <PATH>` | — | Spawn the user's binary to produce the current IR (same protocol as `gen`). Mutually exclusive with `--ir`. |
| `--baseline <PATH>` | `taut/ir.snapshot.json` | Path to the committed baseline snapshot to diff against. |
| `--write` | off | Overwrite `--baseline` with the current IR instead of comparing. Use this once at bootstrap and never again — its purpose is to seed the snapshot. |

Example — typical CI invocation, no flags needed in the common case:

```bash
cargo build && cargo taut check
```

The exit code is `0` if the IR matches the baseline and `1` if drift is
detected; the body of the error lists the differences.

## `cargo taut inspect`

Renders the IR for human consumption. Three output formats are supported:

- `table` (default): two ASCII tables, one for procedures and one for types,
  aligned by column. No external table crate is pulled in; this is the format
  you eyeball during day-to-day development.
- `json`: pretty-printed JSON, ready to pipe through `jq` for ad-hoc queries.
- `mermaid`: a `flowchart LR` block whose nodes are procedures and whose edges
  point at their input and output type references. Paste this into a markdown
  doc to embed your live API surface as a diagram.

| Flag | Default | Description |
|---|---|---|
| `--ir <PATH>` | `target/taut/ir.json` | Path to the IR JSON file to render. Mutually exclusive with `--from-binary`. |
| `--from-binary <PATH>` | — | Spawn the user's binary to produce the IR (same protocol as `gen`). Mutually exclusive with `--ir`. |
| `--format <KIND>` | `table` | Output format: `table`, `json`, or `mermaid`. |

Example — render a Mermaid diagram for a markdown doc:

```bash
cargo taut inspect --format mermaid > docs/api-graph.md
```

## `cargo taut mcp`

Emits a Model Context Protocol manifest in the `tools/list` response shape
(MCP spec 2025-06-18). Each non-subscription `#[rpc]` procedure becomes an MCP
tool whose `inputSchema` is a JSON Schema (Draft 2020-12) describing the wire
envelope `{"input": <value>}`. Point an MCP-aware LLM client at the resulting
file and your taut-rpc API becomes callable as tools — no glue code.

| Flag | Default | Description |
|---|---|---|
| `--ir <PATH>` | `target/taut/ir.json` | Path to the IR JSON file. Mutually exclusive with `--from-binary`. |
| `--from-binary <PATH>` | — | Spawn the user's binary to produce the IR (same protocol as `gen`). Mutually exclusive with `--ir`. |
| `--out <PATH>` | `target/taut/mcp.json` | Path to write the manifest to. Use `-` for stdout. |
| `--include-subscriptions` | off | Also emit a tool entry per subscription procedure. MCP tools are strictly request/response; subscriptions appear as one-shot calls and the streaming nature is invisible at the manifest level. |

Example — emit a manifest excluding subscriptions:

```bash
cargo taut mcp --out target/taut/mcp.json
```

## The `TAUT_DUMP_IR` environment variable

Every subcommand needs the IR. There are two ways to get it on disk:

1. **Explicit dump.** Add `taut_rpc::dump_if_requested(&router)` to the top of
   your `main()`, before any port binding or database connection. When the
   binary runs with `TAUT_DUMP_IR` set, it writes the IR and exits with status
   `0` instead of starting your server.
2. **`--from-binary` (recommended).** All four subcommands accept
   `--from-binary <PATH>`, which spawns the binary with `TAUT_DUMP_IR` set
   internally and reads the result back. You don't need to invoke
   `TAUT_DUMP_IR` yourself; it's an implementation detail.

`TAUT_DUMP_IR` accepts:

- unset or empty → no-op, the binary starts normally.
- `1`, `true`, or `stdout` → write the IR to stdout and exit.
- any other value → treated as a filesystem path; write the IR there
  (creating parent directories) and exit.

You only need to drop down to the env-var directly if you're scripting an IR
dump outside the `cargo taut` flow (e.g. piping into another tool from a
custom build script). For the common path, just call `dump_if_requested` from
`main()` and pass `--from-binary` to whichever subcommand you're running.

## Examples

One-liner usage for each subcommand, against a release binary at
`./target/release/my-app`:

```bash
# Generate a Valibot TypeScript client.
cargo taut gen --from-binary ./target/release/my-app --out web/src/api.gen.ts

# Bootstrap an IR snapshot, then in CI verify nothing drifted.
cargo taut check --from-binary ./target/release/my-app --write
cargo taut check --from-binary ./target/release/my-app

# Inspect the IR as a quick ASCII table.
cargo taut inspect --from-binary ./target/release/my-app

# Emit an MCP manifest, including subscriptions.
cargo taut mcp --from-binary ./target/release/my-app --include-subscriptions

# Or, if you've already run the binary once with TAUT_DUMP_IR set, drop
# `--from-binary` and let each subcommand read target/taut/ir.json by default.
cargo taut gen
cargo taut check
cargo taut inspect --format json | jq '.procedures[].name'
cargo taut mcp
```

# phase1 — `#[rpc]` + `#[derive(Type)]` + `cargo taut gen` end-to-end

This example demonstrates the full Phase 1 pipeline:

1. A Rust/axum server that defines its API using `#[rpc]` on async functions and
   `#[derive(Type)]` on the input/output/error types — no hand-written wire glue,
   no hand-written IR.
2. A `cargo taut gen` pass that runs the server binary in IR-dump mode and
   emits a typed `api.gen.ts`.
3. A TypeScript client that imports the generated file and calls procedures
   with full type safety from the same Rust types.

If this round-trips, Phase 1 is done: changing a Rust signature surfaces as a
TypeScript compile error at the call site, with no runtime schema fetch.

The example lives outside the cargo workspace (`exclude = ["examples"]` in the
root `Cargo.toml`) so it resolves its own dependencies through path entries
and never piggy-backs on workspace machinery.

## What it covers

- `ping() -> String` — zero-input, success-only procedure.
- `add(AddInput) -> Result<i32, AddError>` — input object plus a typed
  `Overflow` error variant.
- `get_user(GetUserInput) -> Result<User, GetUserError>` — typed struct in,
  typed struct out, typed `NotFound { id }` error.
- `get_status() -> Status` — enum return demonstrating both unit variants
  (`Online`, `Offline`) and a struct variant (`Away { since_ms }`).

All input/output/error types use `#[derive(serde::Serialize, serde::Deserialize,
taut_rpc::Type)]`. Errors additionally derive `thiserror::Error` and tag/payload
their JSON shape per SPEC §3.3.

## Run the server

The example is outside the workspace, so the usual `cargo run -p phase1-server`
**will not work**. Run it from its own directory:

```sh
cd examples/phase1/server
cargo run
```

The server binds `0.0.0.0:7701` (note the different port from the Phase 0
smoke, which uses 7700, so both can run side-by-side) and prints:

```
phase1-server listening on http://127.0.0.1:7701
```

CORS is permissive — this is a local-only example, not a deployment template.

## Generate the typed client

The npm runtime needs to be built once before the client can resolve it:

```sh
cd npm/taut-rpc
npm install
npm run build
```

Then build the server in IR-dump mode and run codegen against the binary. From
the repository root:

```sh
cd examples/phase1/server && cargo build && cd ../../..
cargo run -p taut-rpc-cli -- taut gen \
  --from-binary examples/phase1/server/target/debug/phase1-server \
  --out         examples/phase1/client/src/api.gen.ts
```

`--from-binary` re-spawns the server binary with `TAUT_DUMP_IR` set, captures
its IR, then runs the codegen step — see `dump_if_requested(&router)` in
`server/src/main.rs`. The IR never escapes `target/taut/ir.json` and no port is
ever bound during the dump.

If you'd rather drive codegen from a pre-existing IR file, you can pass
`--ir target/taut/ir.json` directly.

## Run the client

In a second terminal, with the server running and `api.gen.ts` generated:

```sh
cd examples/phase1/client
npm install
npm run start
```

`npm run build-api` is also wired up — it shells out to `cargo run -p
taut-rpc-cli -- taut gen ...` from the client directory, so you can regenerate
the client without leaving the front-end shell.

You should see roughly:

```
pong
5
{ id: 1, name: 'ada' }
err: overflow
```

## A note on procedure names

The Rust function name (`get_user`, `get_status`, ...) is the procedure name in
the IR and on the wire (`POST /rpc/get_user`). v0.1's codegen does *not*
translate `snake_case` to `camelCase` on the TS side, so the generated client
exposes procedures under their Rust names. From TypeScript, dotted-camelCase
access (`client.getUser({...})`) is not available; use the bracket form:

```ts
await client.get_user({ id: 1 });
// or, equivalently,
await client["get_user"]({ id: 1 });
```

This matches the IR-as-source-of-truth principle and avoids a surprising
identifier rename happening across the language boundary.

## What this is not

- It is **not** a deployment template. The server has permissive CORS, no auth,
  and binds `0.0.0.0`.
- It is **not** wired into any test harness yet. The exit criterion for Phase 1
  is "`cargo run` + `cargo taut gen` produces a working typed client for
  queries and mutations on a sample app" — running this end-to-end by hand is
  the test for now.

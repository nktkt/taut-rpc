# phase3-counter — `#[rpc(stream)]` + SSE end-to-end subscriptions

This example demonstrates the Phase 3 exit criterion straight from the roadmap:

> A counter that ticks once a second is observable from a TS `for await`.

The server defines an async function whose return type is
`impl futures::Stream<Item = u64> + Send + 'static`, marked with
`#[rpc(stream)]`. The macro registers it as a subscription procedure on the
SSE transport, the codegen emits a TS handle whose `.subscribe(input)` returns
an `AsyncIterable<u64>`, and the client consumes it with a plain `for await`.

If this round-trips end-to-end — Rust stream → SSE → JS async iterator — Phase
3's promise is delivered: streaming is just another procedure kind, with the
same end-to-end type safety queries and mutations already enjoy.

The example lives outside the cargo workspace (`exclude = ["examples"]` in the
root `Cargo.toml`) so it resolves its own dependencies through path entries
and never piggy-backs on workspace machinery.

## What it covers

- `ping() -> &'static str` — sanity check, plain unary query, no subscription
  involved.
- `ticks(TicksInput) -> impl Stream<Item = u64>` — the headline subscription.
  Emits `0..count` with `interval_ms` between values; demonstrates both the
  `#[rpc(stream)]` form and a typed input.
- `server_time() -> impl Stream<Item = String>` — zero-input subscription
  emitting ISO-8601 timestamps every second for three seconds. Shows the
  no-input-arg pattern: codegen drops the input parameter from `.subscribe()`.

All input types use `#[derive(serde::Serialize, serde::Deserialize,
taut_rpc::Type)]`. The wire format is SPEC §4.2 SSE
(`event: data\ndata: <json>\n\n` per item, then `event: end\ndata:\n\n`).

The Phase 4 update adds `taut_rpc::Validate` to `TicksInput` with `count` in
`1..=100` and `interval_ms` in `10..=60_000`. The server now rejects
out-of-range inputs before opening the stream — e.g. `count=10000` is
refused with `validation_error`, never produces a partial sequence, and
surfaces on the client as a `TautError` with `e.code === "validation_error"`.
The client demonstrates this by attempting `count: 200n` after the happy
path and printing the caught code.

## Run the server

The example is outside the workspace, so the usual `cargo run -p
phase3-counter-server` **will not work**. Run it from its own directory:

```sh
cd examples/phase3-counter/server
cargo build
cargo run
```

The server binds `0.0.0.0:7704` (note the different port from previous
examples — Phase 0 smoke uses 7700, Phase 1 uses 7701, Phase 2 uses
7702/7703, so all four can run side-by-side) and prints:

```
phase3-counter-server listening on http://127.0.0.1:7704
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
cd examples/phase3-counter/server && cargo build && cd ../../..
cargo run -p taut-rpc-cli -- taut gen \
  --from-binary examples/phase3-counter/server/target/debug/phase3-counter-server \
  --out         examples/phase3-counter/client/src/api.gen.ts
```

`--from-binary` re-spawns the server binary with `TAUT_DUMP_IR` set, captures
its IR, then runs the codegen step — see `dump_if_requested(&router)` in
`server/src/main.rs`. The IR never escapes `target/taut/ir.json` and no port
is ever bound during the dump.

## Run the client

In a second terminal, with the server running and `api.gen.ts` generated:

```sh
cd examples/phase3-counter/client
npm install
npm run start
```

You should see roughly:

```
ping: pong
ticks (interval 1000ms, count 5):
  tick: 0
  tick: 1
  tick: 2
  tick: 3
  tick: 4
server_time:
  t: 2026-05-06T12:34:56Z
  t: 2026-05-06T12:34:57Z
  t: 2026-05-06T12:34:58Z
rejected (count=200): validation_error
done
```

The five `tick:` lines arrive one per second over five seconds. The three
`t:` lines arrive one per second over three seconds. That cadence is the
proof: the values are not buffered into a list and returned as a batch —
they're streamed one-at-a-time over SSE and the TS `for await` resumes as
each frame arrives.

## Notes on the API shape

- `client.ticks` is a *handle*, not a callable. Subscriptions are accessed via
  `client.ticks.subscribe(input)`, returning an `AsyncIterable<u64>`. This
  mirrors SPEC §6 and is symmetric with how queries/mutations look:
  `await client.add({ a, b })` for unary, `for await (...) of
  client.ticks.subscribe({ ... })` for streaming.
- For zero-input subscriptions, `.subscribe()` takes no argument:
  `for await (const t of client.server_time.subscribe()) { ... }`.
- v0.1's codegen does *not* translate `snake_case` to `camelCase`, so
  `server_time` stays `server_time` on the TS side. Use bracket access
  (`client["server_time"].subscribe()`) if you'd rather not have the
  underscore in dotted form — both work.

## What this is not

- It is **not** a deployment template. The server has permissive CORS, no
  auth, and binds `0.0.0.0`.
- It is **not** wired into any test harness yet. The exit criterion for
  Phase 3 is "a counter that ticks once a second is observable from a TS
  `for await`" — running this end-to-end by hand is the test for now.

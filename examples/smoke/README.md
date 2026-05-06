# smoke — Phase 0 hand-written end-to-end

This example proves the taut-rpc wire format (SPEC §4) is implementable end-to-end
**without macros and without codegen**. Every byte the server emits and the client
parses is hand-written. If this works, the spec is at least implementable; if it
doesn't, the spec needs to change before any macro or codegen work happens.

It exists outside the cargo workspace on purpose (`exclude = ["examples"]` in the
root `Cargo.toml`), so it resolves its own dependencies and can't accidentally
piggy-back on workspace machinery that doesn't exist yet.

## What it covers

- `POST /rpc/ping` — trivial query, no input.
- `POST /rpc/add` — query with an input object, success path.
- `POST /rpc/get_user` — query with two outcomes:
  - `id == 1` returns a user.
  - any other `id` returns `404` with a typed `{ err: { code, payload } }` envelope.
- `GET /rpc/tick` — SSE subscription. Emits one `data` event per second for five
  seconds, then a single `end` event.
- `GET /rpc/_health` — liveness probe, returns `"ok"`.

The bodies all match SPEC §4.1 / §4.2:

```
POST /rpc/<procedure>     →  { "input": <Input> }
200                       →  { "ok": <Output> }
4xx/5xx                   →  { "err": { "code": "...", "payload": ... } }

GET /rpc/<procedure> (SSE)
event: data\ndata: <json>\n\n
event: end\ndata:\n\n
```

## Run the server

The example is outside the workspace, so the usual `cargo run -p smoke-server`
**will not work**. Run it from its own directory:

```sh
cd examples/smoke/server
cargo run
```

The server binds `0.0.0.0:7700` and prints

```
smoke-server listening on http://127.0.0.1:7700
```

CORS is permissive — this is a local-only smoke, not a deployment template.

## Run the client

In a second terminal:

```sh
cd examples/smoke/client
npm install
npm run start
```

You should see, roughly:

```
ping        -> pong
add(2, 3)   -> 5
get_user(1) -> { id: 1, name: 'ada' }
get_user(999) -> err not_found
tick: 0
tick: 1
tick: 2
tick: 3
tick: 4
tick: end
done
```

The client process exits cleanly once the SSE stream closes.

## What this is not

- It is **not** a template for how user code should look once the macros land.
  Real users will write `#[rpc] async fn ping() -> &'static str { "pong" }` and
  let the macro produce the handler.
- It is **not** wired into any test harness yet. Phase 0's exit criterion is
  "request/response round-trip works against a hardcoded TS client" — running it
  by hand is the test for now.

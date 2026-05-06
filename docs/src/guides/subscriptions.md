# Subscriptions

Subscriptions are the third procedure kind in taut-rpc, alongside queries and
mutations. They land in **Phase 3**.

## What subscriptions are

A subscription is a procedure that returns a stream of values rather than a
single value: the server opens a connection, emits zero or more frames, and
eventually closes it (cleanly or with an error). The default transport is
**Server-Sent Events** (SSE) — a plain-HTTP, text-framed protocol that
composes with every middleware in the axum and tower ecosystems unchanged.
WebSocket is available as an opt-in transport for cases where SSE is awkward
(legacy proxies, multiplexing many subscriptions over one socket).

Per [SPEC §4.2](../reference/spec.md), the wire shapes are:

```
GET /rpc/<procedure>?input=<urlencoded-json>
Accept: text/event-stream

event: data\ndata: <json>\n\n
event: error\ndata: {"code":"...","payload":...}\n\n
event: end\ndata: \n\n
```

WebSocket transport is identical at the message level but framed as JSON
messages with `{ type, payload }` shapes (see [WebSocket
transport](#websocket-transport) below).

## Server side

### The `#[rpc(stream)]` attribute

A subscription is declared by adding `stream` to the `#[rpc]` attribute on an
async function whose return type is a `Stream`:

```rust
#[rpc(stream)]
async fn x(input: Input) -> impl futures::Stream<Item = T> + Send + 'static {
    // ...
}
```

The `Send + 'static` bounds are not optional — axum requires them so the stream
can be driven on the runtime's worker pool. The macro will reject a return type
that doesn't carry both bounds.

### A complete example: `ticks`

The canonical subscription is a counter that emits `0, 1, 2, ...` at a fixed
interval. With `async_stream::stream!` the body reads almost like ordinary
async code:

```rust
use std::time::Duration;
use taut_rpc::rpc;
use futures::Stream;

#[derive(serde::Deserialize, taut_rpc::Type)]
struct TicksInput {
    count: u64,
    interval_ms: u64,
}

#[rpc(stream)]
async fn ticks(input: TicksInput) -> impl Stream<Item = u64> + Send + 'static {
    async_stream::stream! {
        for i in 0..input.count {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(input.interval_ms)).await;
            }
            yield i;
        }
    }
}
```

A few things worth calling out:

- The function is `async fn` returning `impl Stream<…>`. The outer `async`
  runs once at subscription start; the `stream!` block runs lazily as the
  consumer pulls values.
- `yield i` emits one `event: data` frame on the wire. The runtime serializes
  `i` to JSON and writes the frame.
- When the `for` loop exits, the stream ends — the runtime emits `event: end`
  and closes the response.

### Errors mid-stream

A subscription that has already started emitting data may need to abort with an
error. Yield a `taut_rpc::StreamFrame::Error { code, payload }` to send an
`event: error` frame:

```rust
#[rpc(stream)]
async fn ticks(input: TicksInput) -> impl Stream<Item = StreamFrame<u64>> + Send + 'static {
    async_stream::stream! {
        for i in 0..input.count {
            if some_condition() {
                yield StreamFrame::Error {
                    code: "rate_limited",
                    payload: serde_json::json!({ "retry_after_ms": 1000 }),
                };
                return;
            }
            yield StreamFrame::Data(i);
        }
    }
}
```

The current macro does not directly map `Result<T, E>` *items* into separate
`data` and `error` frames — that is a Phase 4+ refinement. For now, lift items
into `StreamFrame` explicitly when you need typed errors mid-stream. For
streams that can only succeed (like `ticks`), keep `Item = T` and let
panics/disconnects be handled by the transport.

## Wire format (SSE)

Three event types per [SPEC §4.2](../reference/spec.md):

- **`data`** — one stream item, JSON-encoded.
- **`error`** — terminal error frame; the stream ends after this.
- **`end`** — clean termination; no more frames will follow.

The input is passed as a URL-encoded JSON blob in the `input` query parameter,
mirroring `#[rpc(method = "GET")]` queries. This is forced by SSE: the
EventSource API and most server-push proxies only deal with `GET`.

## Client side (TypeScript)

### What codegen emits

For a `#[rpc(stream)]` procedure, `cargo taut gen` writes a procedure entry
with kind `"subscription"`:

```ts
export type Proc_ticks = ProcedureDef<TicksInput, bigint, never, "subscription">;
```

The runtime's `ClientOf<P>` mapped type recognises the `"subscription"` kind
and produces a method shaped like:

```ts
{
  subscribe(input: Input): AsyncIterable<Output>;
}
```

### Idiomatic usage

```ts
import { createClient } from "taut-rpc/client";
import type { Procedures } from "./api.gen";

const client = createClient<Procedures>({ url: "/rpc" });

for await (const tick of client.ticks.subscribe({ count: 5n, interval_ms: 1000n })) {
    console.log(tick);
}
```

`u64` on the Rust side maps to `bigint` in TypeScript by default (see [Type
mapping](../concepts/type-mapping.md)), which is why the input literals are
`5n` and `1000n` and `tick` is typed as `bigint`.

### Cancellation

Two ways to cancel a `for await` loop: `break` out of it, or call
`iterator.return()` directly. Either path triggers an
`AbortController.abort()` on the underlying fetch, so the server sees a
disconnected client and the stream is dropped on the next yield.

```ts
for await (const tick of client.ticks.subscribe({ count: 1000n, interval_ms: 100n })) {
    if (tick > 5n) break;
}
```

### Errors

An `event: error` frame from the server throws a `TautError` from inside the
`for await` loop. Wrap with `try`/`catch` to handle it:

```ts
try {
    for await (const tick of client.ticks.subscribe({ count: 5n, interval_ms: 1000n })) {
        console.log(tick);
    }
} catch (e) {
    if (e instanceof TautError) {
        console.error(`stream errored: ${e.code}`, e.payload);
    } else {
        throw e;
    }
}
```

The thrown `TautError`'s `code` is narrowed to the procedure's declared error
union, exactly as for queries and mutations.

## WebSocket transport

The WebSocket transport is feature-gated behind `cfg(feature = "ws")`. Enable
it when:

- You want **many subscriptions multiplexed over one connection** rather than
  one HTTP request per subscription.
- The deployment environment has **proxies that mishandle SSE** — some legacy
  L7 proxies buffer responses, breaking the streaming contract.

### Mounting

When the `ws` feature is enabled, the router exposes one extra endpoint:

```
GET /rpc/_ws
```

A client connects once and then sends `Subscribe` messages to start streams
over that single connection.

### Wire shape

WebSocket frames are JSON-encoded `WsMessage` values, tagged on `type`:

```
// client → server
{ "type": "subscribe", "id": "<client-correlation-id>", "procedure": "ticks", "input": { "count": 5, "interval_ms": 1000 } }
{ "type": "unsubscribe", "id": "<id>" }

// server → client
{ "type": "data",  "id": "<id>", "value": <json> }
{ "type": "error", "id": "<id>", "code": "...", "payload": <json> }
{ "type": "end",   "id": "<id>" }
```

Each `Subscribe` correlates with a series of `Data` frames terminated by
`End` (clean) or `Error` (terminal failure). The `id` is opaque to the server
— the client picks it and uses it to demux frames.

### Phase 3 status

Phase 3 ships the **server-side** WebSocket transport. The TypeScript client
transport for WebSocket is on the roadmap for **Phase 4+**; until then, the
generated client speaks SSE only. If you want WebSocket on the client today,
write directly against `WebSocket` and the `WsMessage` shape above.

## Worked example

A runnable demo lives at [`examples/phase3-counter/`](https://github.com/taut-rpc/taut-rpc/tree/main/examples/phase3-counter).
It wires up the `ticks` procedure shown above, runs the axum server, and
includes a tiny TS client that prints each frame as it arrives.

## Limits

A few rough edges worth knowing about before you depend on subscriptions in
production:

- **No `Result<T, E>` items.** v0.1 subscriptions don't auto-map a stream of
  `Result`s into typed `data` and `error` frames. Use `StreamFrame::Data` /
  `StreamFrame::Error` manually when you need typed errors mid-stream.
- **Backpressure.** The server emits frames as fast as it produces them; flow
  control over SSE is limited to HTTP/1.1 keepalive and TCP-level
  backpressure. For tighter control, use the WebSocket transport (which has
  per-frame acknowledgements implicit in the WS framing layer) or rate-limit
  inside your stream body.
- **No auto-reconnection.** The runtime does not retry on transient network
  failures. Wrap the `for await` in a retry loop with backoff if you need
  reconnect-on-drop semantics. There is also no notion of resumable cursors —
  a reconnect starts the stream over from the beginning, so encode any
  resumption state in your `Input`.

## See also

- [Roadmap — Phase 3](../reference/roadmap.md)
- [SPEC §4.2 — Subscription wire format](../reference/spec.md)
- [Wire format](../concepts/wire-format.md)
- [Errors](./errors.md)

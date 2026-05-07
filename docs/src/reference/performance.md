# Performance

This page describes the runtime cost model of taut-rpc and how it compares to
adjacent tools. Numbers are order-of-magnitude estimates measured on a modern
x86 laptop; treat them as a sanity check, not a benchmark.

## Server-side request path

A single RPC request walks four stages on the server:

1. **Deserialize** the JSON body into the handler's input type (`serde_json`).
2. **Validate** the input by running the `check::*` constraints attached to the
   procedure.
3. **Call** the user-supplied handler.
4. **Serialize** the handler's return value back into JSON.

Stages 1, 2, and 4 are framework overhead and typically total **under 50µs**
for small payloads. Once your handler does anything non-trivial — a database
round-trip, an outbound HTTP call, a CPU-bound transform — stage 3 dominates
and the framework cost becomes noise.

## Validation overhead

Each `check::*` invocation is constant-time with respect to the request
(`check::min`, `check::max`, `check::len`, etc. all reduce to a single
comparison or length read).

The current exception is `check::regex`: in v0.1 the pattern is compiled on
every request. This is on the v0.2 roadmap to be cached behind a `OnceLock`,
which will make repeated regex checks effectively free.

## Codegen output

The generated TypeScript client is a single `.ts` file. For an API with on the
order of 50 procedures the file lands **under 100KB** before minification, and
gzips to a few KB. The client never fetches the schema at runtime — everything
needed for type-safe calls is inlined at codegen time.

## SSE subscriptions

Subscriptions are delivered over Server-Sent Events. In v0.1 the keep-alive
heartbeat is **disabled by default** because of axum 0.8's stricter
`Sse::keep_alive` typing; you can opt in manually if your proxy needs it.

Reconnection on connection drop is the **client's responsibility**. The
generated client surfaces the underlying `EventSource`, so standard browser
reconnection semantics apply.

## Comparison vs tRPC + Node

For a simple in-memory query (e.g. `getUser` against a `HashMap`), a native
Rust handler is **roughly 5–10x faster** than the equivalent tRPC handler on
Node. The gap widens as handler complexity grows: anything CPU-bound (parsing,
hashing, compression, string manipulation at scale) tilts further in Rust's
favour.

If your handlers spend nearly all their time waiting on I/O, the language gap
narrows and the choice becomes about ergonomics rather than throughput.

## Comparison vs gRPC + tonic

protobuf produces a denser wire format than JSON. On a like-for-like benchmark
tonic can be **~30% faster on the wire** purely from smaller payloads and
faster decode.

taut-rpc deliberately trades that density for human-readable JSON: you can
`curl` a procedure, paste a payload into a debugger, or eyeball traffic in
DevTools without tooling. For browser-facing APIs that ergonomic win usually
outweighs the wire cost.

## Memory footprint

taut-rpc adds **roughly 500KB** to the final binary, excluding axum itself.
Per-request allocation is dominated by `serde_json`'s arena; the dispatch
layer's only steady-state overhead is **one `Arc` clone** per request to hand
the shared state into the handler closure.

## Benchmarking your own handlers

A `criterion` benchmark lives at:

```
crates/taut-rpc/benches/dispatch.rs
```

It exercises the full deserialize → validate → call → serialize path for a
representative procedure. Copy it, swap in your own handler and input fixture,
and run with `cargo bench` to get an apples-to-apples number for your code.

## Optimization knobs

A handful of knobs let you trade safety, ergonomics, or wire shape for speed:

- **`validate.send: false`** on the client skips client-side validation before
  the request goes out. The server still validates, so this is purely about
  removing a redundant check from the hot path on the browser side.
- **`--bigint-strategy as-string`** at codegen time emits large integer IDs as
  JSON strings instead of numbers. This avoids JavaScript's 53-bit precision
  cliff and is worth turning on for any ID type that can exceed `2^53`.
- **`--validator none`** at codegen time omits the runtime parse step in the
  generated client. Use it when you fully trust the server's response shape
  and want the smallest, fastest client possible.

None of these knobs are required — the defaults are tuned for correctness
first. Reach for them only when a profile points at the corresponding cost.

# phase2-tracing — `tower-http::TraceLayer` on a taut-rpc `Router`

This example demonstrates that taut-rpc is *axum-native, not axum-locked*
(SPEC §1, goal 4 and SPEC §5): once you've registered procedures, the returned
`Router` is just a `tower::Service` builder, so the entire axum middleware
ecosystem applies without any taut-specific glue. Here we layer
`tower-http::trace::TraceLayer` over the procedures and let
`tracing-subscriber` format the resulting spans/events to stdout.

## What you should see

Each HTTP request opens a span carrying:

- method (`POST`)
- path (`/rpc/echo`)
- response status (e.g. `200 OK`)
- latency in milliseconds (configured via `LatencyUnit::Millis`)

Inside that span, the `echo` procedure emits its own
`tracing::info!(?input, "echo called")` event. Because `tracing` propagates
the current span through the async runtime, no manual `#[instrument]`
decoration on the procedure is required — the procedure's event
automatically nests under the request span set up by `TraceLayer`.

## Run the server

```sh
cd examples/phase2-tracing/server
RUST_LOG=info,tower_http=debug cargo run
```

`RUST_LOG` is read by `EnvFilter::from_default_env()` in `main()`, so dialling
verbosity up or down is a runtime concern, not a code change. The
`info,tower_http=debug` profile shows the high-level request/response spans at
`INFO` plus the per-request "started processing"/"finished processing" trail
that `tower-http` emits at `DEBUG`.

The server binds `0.0.0.0:7703` (different from phase1's 7701, so both can
run side-by-side) and logs:

```
phase2-tracing-server listening on http://127.0.0.1:7703
```

## Make a request

In another terminal:

```sh
curl -X POST http://127.0.0.1:7703/rpc/echo \
  -H 'content-type: application/json' \
  -d '{"input":"hello"}'
```

The response is the SPEC §4.1 envelope:

```json
{"ok":"hello"}
```

In the server log you'll see (roughly):

```
INFO ThreadId(02) request{method=POST uri=/rpc/echo version=HTTP/1.1}: tower_http::trace::on_request: started processing request
INFO ThreadId(02) request{method=POST uri=/rpc/echo version=HTTP/1.1}: phase2_tracing_server: echo called input="hello"
INFO ThreadId(02) request{method=POST uri=/rpc/echo version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=200
```

The exact formatting depends on `tracing-subscriber` version and terminal
width, but the nesting is the load-bearing detail: the procedure's `echo
called` event is rendered with the surrounding `request{...}` span context,
because `TraceLayer` wraps the whole router (including taut-rpc's built-in
`not_found` / `decode_error` paths from §8 of the SPEC's resolved questions).

## Why no TS client here

This example is intentionally narrow: it only demonstrates the tracing wiring.
The codegen story — `#[derive(Type)]`, `cargo taut gen`, the typed
`api.gen.ts` consumer — is already covered end-to-end by
[`examples/phase1`](../phase1) (and any future `phase2-auth` companion will
cover stateful middleware in the same style). The `curl` call above is enough
to exercise the request path and observe the span output.

## What this is not

- It is **not** a deployment template. The server binds `0.0.0.0`, has no
  auth, and uses default tower-http formatters tuned for a developer's
  terminal, not a log aggregator.
- It is **not** an OpenTelemetry / OTLP example. Plugging in an OTLP exporter
  is a `tracing-subscriber` configuration choice — replace
  `tracing_subscriber::fmt()` with the layered subscriber of your choice; the
  router wiring stays exactly the same.

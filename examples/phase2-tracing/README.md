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
- path (e.g. `/rpc/echo`, `/rpc/add`, `/rpc/slow_op`)
- response status (e.g. `200 OK`, or `400` for a `validation_error`)
- latency in milliseconds (configured via `LatencyUnit::Millis`)

Inside that span, each procedure body runs inside its *own* span set up by
the `#[tracing::instrument]` attribute on the handler. The procedure span
records the input fields as structured fields, and any `tracing::info!`
event the body emits nests under both spans.

The Phase 4 `Validate` pipeline runs *before* the procedure body. When a
request fails validation, the rejection is rendered into the standard
`validation_error` envelope *inside* the `TraceLayer` span, so the failed
request still carries method/URI/status/latency in its log line.

## Procedures

| Procedure  | Input                              | Output | What it shows |
|------------|------------------------------------|--------|---------------|
| `echo`     | `String`                           | `String` | Primitive input — no `Validate` derive needed (Phase 4 blanket impls cover primitives). |
| `add`      | `AddInput { lhs: i32, rhs: i32 }`  | `i32`    | `#[derive(Validate)]` with `#[taut(min = 0, max = 1000)]` on each field, so out-of-range inputs trace as `validation_error`. |
| `slow_op`  | `()`                               | `u64`    | No input; sleeps 100 ms before returning so the `TraceLayer` `latency=…` field is non-trivial. |

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

## Make some requests

In another terminal.

### `echo` — primitive input, no validation

```sh
curl -X POST http://127.0.0.1:7703/rpc/echo \
  -H 'content-type: application/json' \
  -d '{"input":"hello"}'
```

Response (SPEC §4.1 envelope):

```json
{"ok":"hello"}
```

Server log (roughly):

```
INFO ThreadId(02) request{method=POST uri=/rpc/echo version=HTTP/1.1}: tower_http::trace::on_request: started processing request
INFO ThreadId(02) request{method=POST uri=/rpc/echo version=HTTP/1.1}:echo{input=hello}: phase2_tracing_server: echo called
INFO ThreadId(02) request{method=POST uri=/rpc/echo version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=200
```

### `add` — Validate-derived input

Happy path:

```sh
curl -X POST http://127.0.0.1:7703/rpc/add \
  -H 'content-type: application/json' \
  -d '{"input":{"lhs":2,"rhs":3}}'
```

```json
{"ok":5}
```

```
INFO ThreadId(02) request{method=POST uri=/rpc/add version=HTTP/1.1}: tower_http::trace::on_request: started processing request
INFO ThreadId(02) request{method=POST uri=/rpc/add version=HTTP/1.1}:add{lhs=2 rhs=3}: phase2_tracing_server: add computed sum=5
INFO ThreadId(02) request{method=POST uri=/rpc/add version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=200
```

Validation failure (`lhs` exceeds `max = 1000`):

```sh
curl -X POST http://127.0.0.1:7703/rpc/add \
  -H 'content-type: application/json' \
  -d '{"input":{"lhs":9999,"rhs":3}}'
```

```json
{"err":{"code":"validation_error","payload":{"issues":[{"path":["lhs"],"message":"…"}]}}}
```

```
INFO ThreadId(02) request{method=POST uri=/rpc/add version=HTTP/1.1}: tower_http::trace::on_request: started processing request
INFO ThreadId(02) request{method=POST uri=/rpc/add version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=0 ms status=400
```

Note the `add` span is *absent* from the failed-validation log: the request
short-circuits before `add`'s body (and therefore its `#[tracing::instrument]`
span) runs. The `TraceLayer` span still wraps the rejection, so you see the
4xx status alongside the URI it came from.

### `slow_op` — exercise latency reporting

```sh
curl -X POST http://127.0.0.1:7703/rpc/slow_op \
  -H 'content-type: application/json' \
  -d '{}'
```

```json
{"ok":100}
```

```
INFO ThreadId(02) request{method=POST uri=/rpc/slow_op version=HTTP/1.1}: tower_http::trace::on_request: started processing request
INFO ThreadId(02) request{method=POST uri=/rpc/slow_op version=HTTP/1.1}:slow_op: phase2_tracing_server: slow_op slept elapsed_ms=100
INFO ThreadId(02) request{method=POST uri=/rpc/slow_op version=HTTP/1.1}: tower_http::trace::on_response: finished processing request latency=100 ms status=200
```

The exact formatting depends on `tracing-subscriber` version and terminal
width, but the nesting is the load-bearing detail: each procedure's
`#[tracing::instrument]` span is rendered inside the surrounding
`request{...}` span context, because `TraceLayer` wraps the whole router
(including taut-rpc's built-in `not_found` / `decode_error` /
`validation_error` paths from §8 of the SPEC's resolved questions).

## Why no TS client here

This example is intentionally narrow: it only demonstrates the tracing wiring.
The codegen story — `#[derive(Type)]`, `cargo taut gen`, the typed
`api.gen.ts` consumer — is already covered end-to-end by
[`examples/phase1`](../phase1) (and `phase4-validate` covers the
`#[derive(Validate)]` codegen path with a TS client). The `curl` calls above
are enough to exercise the request path and observe the span output.

## What this is not

- It is **not** a deployment template. The server binds `0.0.0.0`, has no
  auth, and uses default tower-http formatters tuned for a developer's
  terminal, not a log aggregator.
- It is **not** an OpenTelemetry / OTLP example. Plugging in an OTLP exporter
  is a `tracing-subscriber` configuration choice — replace
  `tracing_subscriber::fmt()` with the layered subscriber of your choice; the
  router wiring stays exactly the same.

## See also

- The [Phase 1 example](../phase1/README.md) — the `#[rpc]` + `cargo taut gen` baseline; covers the codegen story this example deliberately skips.
- The [Phase 2 auth example](../phase2-auth/README.md) — sibling middleware example using `axum::middleware::from_fn` for auth and typed errors.
- The [Phase 4 validate example](../phase4-validate/README.md) — `#[derive(Validate)]` end-to-end with a TS client; goes deeper on every `#[taut(...)]` constraint kind that `add` only samples here.
- [Guide: Middleware](../../docs/src/guides/middleware.md) — composing `tower::Layer` stacks (including `TraceLayer`) around a taut-rpc `Router`.
- [Guide: Deployment](../../docs/src/guides/deployment.md) — picking up where this example leaves off, with structured logging and observability tuned for production.
- [Concepts: Architecture](../../docs/src/concepts/architecture.md) — why the router stays a plain `tower::Service` builder and what that buys you.

# Middleware

`taut_rpc::Router` exposes `Router::layer(...)`, which delegates to
`axum::Router::layer`. Anything that implements `tower::Layer` plugs in.
This is the entire middleware story: there is no `taut-rpc`-specific
middleware system, and the axum/`tower` ecosystem is reused unchanged.

## When to reach for `Router::layer`

`Router::layer` is the right hammer for **per-request, cross-cutting
concerns** — things that aren't about the procedure's domain logic and
that you'd want to apply to many or all routes:

- **Authentication / authorization** — bearer tokens, session cookies.
  See [Authentication](./auth.md).
- **Tracing / logging** — emit a span per request, log status and
  latency.
- **Rate limiting** — drop or delay requests above a threshold.
- **CORS** — let the browser preflight cleanly.
- **Compression** — gzip large response bodies.
- **Timeouts** — abort handlers that run too long.

Per-procedure concerns (input validation, domain authorization) belong
in the procedure body, not in a layer. The middleware layer is for
things that don't know what procedure they're in front of.

## Examples

### Tracing

```rust
use taut_rpc::{rpc, Router};
use tower_http::trace::TraceLayer;

#[rpc]
async fn ping() -> &'static str { "pong" }

let app: axum::Router = Router::new()
    .procedure(ping)
    .layer(TraceLayer::new_for_http())
    .into_axum();
```

A complete, runnable version lives at `examples/phase2-tracing/`. It
sets up `tracing_subscriber`, mounts `TraceLayer::new_for_http()`, and
prints a span per request.

### CORS

```rust
use tower_http::cors::CorsLayer;

let app: axum::Router = Router::new()
    .procedure(my_procedure)
    .layer(CorsLayer::permissive())
    .into_axum();
```

`CorsLayer::permissive()` is fine for local dev; in production lock the
allowed origins explicitly.

### Rate limiting

```rust
use std::time::Duration;
use tower::limit::RateLimitLayer;
use tower::ServiceBuilder;

let app: axum::Router = Router::new()
    .procedure(my_procedure)
    .layer(
        ServiceBuilder::new()
            .layer(RateLimitLayer::new(100, Duration::from_secs(1))),
    )
    .into_axum();
```

`tower::ServiceBuilder` is the conventional way to compose multiple
layers into one before handing them to `.layer(...)`.

## Layer composition order

`.layer(...)` calls compose **onion-style**: each call wraps the result
of the previous one. The *last* layer added is the *outermost* — it
sees the request first and the response last.

```text
   request ──▶ ┌──── outer (added last) ────┐
               │  ┌── middle ──┐            │
               │  │  ┌─inner ─┐│            │
               │  │  │ route  ││            │
               │  │  └────────┘│            │
               │  └────────────┘            │
               └────────────────────────────┘ ──▶ response
```

So the call:

```rust
Router::new()
    .procedure(my_procedure)
    .layer(TraceLayer::new_for_http())   // inner
    .layer(CorsLayer::permissive())      // outer
```

means CORS sees the request first (and gets to short-circuit a
preflight), and the trace layer wraps only the request-handling that
makes it past CORS. If you want tracing to cover *everything*, including
CORS preflights, swap the order — add `TraceLayer` last.

If you have many layers and the order matters, use
`tower::ServiceBuilder`, which composes top-to-bottom (the way most
people read code) and hands you one composite layer to pass to
`.layer(...)`.

## Layers and the built-in routes

The router serves three built-in routes alongside your procedures:

- `POST /rpc/<procedure>` — your `#[rpc]` functions.
- `GET /rpc/_health` — text/plain `ok`.
- `GET /rpc/_procedures` — JSON list of registered procedure names.

Every layer added via `Router::layer` sees **all** of these. That's
usually fine — tracing every health check is cheap, CORS on a health
check is harmless. Auth is the exception: `_health` getting a 401
breaks load balancers, and `_procedures` getting a 401 breaks tooling
that introspects the deployed surface.

When that matters, gate the auth check inside the middleware on the URI
path:

```rust
async fn require_bearer(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let needs_auth = path.starts_with("/rpc/") && !path.starts_with("/rpc/_");
    if needs_auth && !has_valid_token(&request) {
        return unauthenticated();
    }
    next.run(request).await
}
```

The same pattern applies to anything else that should run on
procedures-only.

## See also

- [Authentication](./auth.md) — full bearer-token middleware example.
- [SPEC §5 — Server API](../reference/spec.md)
- `examples/phase2-tracing/` — runnable trace-layer demo.
- `examples/phase2-auth/` — runnable bearer-auth demo.

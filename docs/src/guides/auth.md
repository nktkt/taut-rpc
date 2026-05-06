# Authentication

Authentication in `taut-rpc` is **not a `taut-rpc` feature** — it's an
axum middleware. Phase 2 makes this concrete: `taut_rpc::Router` exposes
`Router::layer(...)`, which delegates to `axum::Router::layer`. Anything
that implements `tower::Layer` works, including hand-rolled middleware
written with `axum::middleware::from_fn`. There is no parallel
`taut-rpc`-specific auth abstraction to learn.

This guide walks through bearer-token validation as a request middleware
and per-procedure auth as a typed error.

## Bearer-token middleware

The shape of an axum middleware function is:

```rust
async fn middleware(request: Request, next: Next) -> Response { ... }
```

You pull whatever you need off the request, decide whether to short-
circuit, and otherwise call `next.run(request).await`. Here's a complete
bearer-token check:

```rust
use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

async fn require_bearer(request: Request, next: Next) -> Response {
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    let Some(token) = header.and_then(|h| h.strip_prefix("Bearer ")) else {
        return unauthenticated();
    };

    if !is_valid(token) {
        return unauthenticated();
    }

    next.run(request).await
}

fn unauthenticated() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "err": {
                "code": "unauthenticated",
                "payload": null,
            },
        })),
    )
        .into_response()
}

fn is_valid(token: &str) -> bool {
    // verify a JWT, look up a session, etc.
    !token.is_empty()
}
```

Two things to notice:

1. The 401 response body is the **SPEC envelope**, not a free-form
   message. Clients parse every error response the same way; middleware
   short-circuits don't get to invent their own shape.
2. The `code` is `"unauthenticated"` — the same code the
   `StandardError::Unauthenticated` variant emits. Codes are convention,
   not enforced; staying consistent across the API matters more than the
   exact spelling.

### Wiring it up

`Router::layer` is the entry point:

```rust
use taut_rpc::{rpc, Router};

#[rpc]
async fn whoami() -> &'static str { "you" }

let app: axum::Router = Router::new()
    .procedure(whoami)
    .layer(axum::middleware::from_fn(require_bearer))
    .into_axum();
```

Every request hitting the router — including `/rpc/_health` and
`/rpc/_procedures` — flows through `require_bearer` first. If you only
want auth on the procedure routes, gate it inside the middleware on the
URI prefix; see [Middleware](./middleware.md) for the pattern.

## Per-procedure auth as a typed error

Sometimes "is this user logged in" lives in middleware, but "can this
user delete this resource" lives in the procedure itself. Pair the
domain check with a `#[derive(TautError)]` enum and the TS client
narrows on it:

```rust
use serde::Serialize;
use taut_rpc::{rpc, TautError};

#[derive(Debug, Serialize, TautError)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum DeleteUserError {
    #[taut(status = 403)]
    Forbidden { reason: String },
    NotFound { id: u64 },
}

#[rpc]
async fn delete_user(input: DeleteInput) -> Result<(), DeleteUserError> {
    if !is_admin(&input.actor) {
        return Err(DeleteUserError::Forbidden {
            reason: "admin only".into(),
        });
    }
    if !exists(input.id) {
        return Err(DeleteUserError::NotFound { id: input.id });
    }
    do_delete(input.id);
    Ok(())
}
```

The middleware proves "the request has *some* valid identity"; the
procedure proves "this identity is allowed to do *this* action." That
split keeps middleware decoupled from per-route policy and keeps the
typed-error narrative honest on the TS side — see
[Errors](../concepts/errors.md) for the matching `isTautError` /
`errorMatch` helpers.

## Where to put state

Phase 2 does **not** yet wire `axum::extract::State` into
`#[rpc]`-generated handlers. That's deferred to a later phase. Until
then, three workable patterns:

1. **`OnceCell` / `static`** — simple and stateless. Initialise the
   `OnceCell` from `main` before mounting the router, then read it from
   procedures and middleware. Fine for shared resources like a database
   pool.

2. **`tokio::task_local!`** — set per-request inside a middleware
   wrapper, read from procedures. Useful for things that genuinely vary
   per-request (a request-scoped tracing span, a tenant id from a JWT).

3. **Request extensions in a closure handler.** If you need the value
   only on a single route and `#[rpc]` doesn't fit, drop down to a plain
   `axum::routing::post(...)` closure that calls `req.extensions()`
   directly, and mount it on the underlying `axum::Router` after
   `into_axum()`.

When `State` extraction lands in a later phase, this section will
shrink to "just take a `State<S>` parameter."

## Subscription auth

Subscriptions are SSE/WebSocket on top of the same HTTP routing layer,
so the same `Router::layer(...)` wiring runs on the upgrade request.
The long-lived nature of the connection has lifecycle quirks (mid-
stream invalidation, reconnect handshakes) worth designing for; those
land with Phase 3.

## Runnable example

A complete, runnable bearer-auth example lives at
`examples/phase2-auth/`. It registers a single procedure, applies the
middleware shown above, and includes both successful and unauthenticated
test runs against it.

## See also

- [Middleware guide](./middleware.md) — broader treatment of `Router::layer`.
- [Errors](../concepts/errors.md) — narrowing typed errors on the TS side.
- [SPEC §5 — Server API](../reference/spec.md)

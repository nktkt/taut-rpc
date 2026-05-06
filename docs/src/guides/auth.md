# Authentication

> Placeholder guide. Auth is Phase 2 on the
> [roadmap](../reference/roadmap.md); the design is summarized in
> [SPEC §5](../reference/spec.md). The short version: **auth is not a
> `taut-rpc` feature, it's a `tower::Layer`** — same as anything else
> in the axum ecosystem.

## The model

`taut-rpc`'s server router compiles down to an `axum::Router`. That
means every middleware story already documented for axum and `tower`
applies unchanged: extractors for typed claims, layers for token
validation, layers for rate limiting, layers for tracing. There is no
parallel `taut-rpc`-specific middleware system to learn, and no
`taut-rpc`-specific authentication abstraction to maintain.

```rust
use taut_rpc::{Router, rpc};
use tower_http::auth::RequireAuthorizationLayer;

#[rpc]
async fn me(claims: Claims) -> Result<User, ApiError> {
    // ... use claims to look up the user
}

let app: axum::Router = Router::new()
    .procedure(me)
    .with_state(state)
    .into_axum()
    .layer(RequireAuthorizationLayer::bearer("...")); // any tower::Layer
```

## How claims reach a `#[rpc]` function

Just like in axum: parameters that implement `FromRequestParts` are
extractors. A `Claims` type that implements the trait can appear in any
`#[rpc]` function's signature, and the macro emits the appropriate
extraction call. The same applies to `axum::extract::State`,
`axum::Extension`, and friends.

## Standard error code

SPEC §8 raises the question of whether the project should reserve a
standard `unauthenticated` error discriminant by convention. The
current lean is yes — middleware can short-circuit a request with a
typed error rather than falling back to a raw HTTP 401, and the TS
client narrows it just like any procedure-defined error code. Final
shape is deferred to Phase 2.

## What this chapter will cover when written

- Worked example: JWT bearer auth with a `Claims` extractor.
- Worked example: cookie-session auth, including CSRF concerns for the
  POST-by-default wire format.
- The `unauthenticated` standard code, once it's specified.
- Subscription auth: the SSE handshake is just an HTTP request, so the
  same layers work — but the long-lived connection has lifecycle quirks
  worth calling out.

## See also

- [Roadmap — Phase 2](../reference/roadmap.md)
- [SPEC §5 — Server API](../reference/spec.md)
- [Errors](../concepts/errors.md)

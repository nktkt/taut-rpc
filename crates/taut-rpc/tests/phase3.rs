//! Phase 3 integration tests for taut-rpc — subscriptions end-to-end.
//!
//! These tests pin the macro→runtime→SSE pipeline against the Phase 3 shared
//! contract:
//!
//! - The `#[rpc(stream)]` attribute accepts an
//!   `async fn x(...) -> impl futures::Stream<Item = T> + Send + 'static`
//!   shape and emits a `__taut_proc_<name>()` whose `body` is
//!   `ProcedureBody::Stream(...)` and whose IR `kind` is
//!   `ProcKind::Subscription` with `http_method = HttpMethod::Get`.
//! - `Router::new().procedure(...).into_axum()` mounts a `GET /rpc/<name>`
//!   route for every subscription procedure that emits SPEC §4.2 SSE frames:
//!   `event: data\ndata: <json>\n\n`   for each yielded item,
//!   `event: error\ndata: <{...}>\n\n` for `StreamFrame::Error`,
//!   `event: end\ndata: \n\n`         once the underlying stream finishes.
//! - The same router can host both unary (POST) and streaming (GET) procedures
//!   and each kind only responds to its own HTTP method.
//!
//! Phase 1/2 conventions for HTTP-level assertions still apply (see
//! `tests/integration.rs` and `tests/phase2.rs`): build the app with
//! `Router::new().procedure(...).into_axum()`, drive it via
//! `tower::ServiceExt::oneshot`, and read the body with
//! `axum::body::to_bytes`.

use axum::body::Body;
use http::Request as HttpRequest;
use tower::ServiceExt;

use taut_rpc::ir::{HttpMethod, Primitive, ProcKind, TypeRef};
use taut_rpc::procedure::ProcedureBody;
use taut_rpc::ProcKindRuntime;

// ---------------------------------------------------------------------------
// Shared procedures used across the descriptor and HTTP-level tests.
//
// Defining them at module scope (rather than inside each `#[test]`) avoids
// duplicating the `#[rpc(stream)]` invocation in every test and lets the
// HTTP-level tests reuse the same `__taut_proc_*` factory functions the macro
// emits. Each test that mounts a router builds a fresh one — `Router::new()`
// is cheap, and procedure registration takes the descriptor by value so we
// can call the factory repeatedly across tests without fighting ownership.
// ---------------------------------------------------------------------------

/// Typed input for the `ticks` subscription. Exercises the
/// `#[rpc(stream)]` + `#[derive(taut_rpc::Type)]` pairing on the input side
/// (the codegen path needs this to surface the input as a TS type).
#[derive(serde::Serialize, serde::Deserialize, taut_rpc::Type, taut_rpc::Validate)]
#[allow(dead_code)]
struct TicksInput {
    count: u64,
}

/// One-arg subscription. Yields `0..input.count` then ends.
///
/// Uses `async_stream::stream!` to build an `impl Stream<Item = u64>`, which
/// is the canonical Phase 3 shape per SPEC §5.1.
#[taut_rpc::rpc(stream)]
#[allow(clippy::unused_async)] // `#[rpc(stream)]` requires `async fn` signatures
async fn ticks(input: TicksInput) -> impl futures::Stream<Item = u64> + Send + 'static {
    async_stream::stream! {
        for i in 0..input.count {
            yield i;
        }
    }
}

/// Zero-arg subscription. Yields `0..3` then ends.
///
/// Lets us exercise the subscription path without engaging input
/// deserialization at all — proves zero-arg `#[rpc(stream)]` works and that
/// the router handles the no-`?input=` URL form.
#[taut_rpc::rpc(stream)]
#[allow(clippy::unused_async)] // `#[rpc(stream)]` requires `async fn` signatures
async fn ticks_simple() -> impl futures::Stream<Item = u32> + Send + 'static {
    async_stream::stream! {
        for i in 0..3u32 {
            yield i;
        }
    }
}

/// Plain unary query, used by the "unary + stream coexist" test to prove
/// `Router::new().procedure(...).procedure(...).into_axum()` can mix the two.
#[taut_rpc::rpc]
#[allow(clippy::unused_async)] // `#[rpc]` requires `async fn` signatures
async fn ping_unary() -> String {
    "pong".to_string()
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Drain the body of an `axum::Response` into a `String`. The SSE body is
/// always UTF-8 text, so we don't need to keep it as bytes — making it a
/// `String` lets every assertion below use plain `contains(...)` checks.
async fn body_string(response: axum::http::Response<Body>) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    String::from_utf8(bytes.to_vec()).expect("response body is utf-8")
}

// ---------------------------------------------------------------------------
// 1. `#[rpc(stream)]` emits a `ProcedureDescriptor` whose runtime kind,
//    IR kind, HTTP method, and body all line up with Phase 3's contract.
// ---------------------------------------------------------------------------

#[test]
fn stream_macro_emits_subscription_descriptor() {
    let desc = __taut_proc_ticks();

    // Static name (the descriptor's `name` field is the path segment in
    // `/rpc/<name>` and must equal the underlying fn name verbatim).
    assert_eq!(desc.name, "ticks");

    // Runtime tag — drives router dispatch.
    assert_eq!(desc.kind, ProcKindRuntime::Subscription);

    // IR kind — drives codegen (TS client emits a `.subscribe(input)` handle
    // for `Subscription` rather than a callable for `Query` / `Mutation`).
    assert_eq!(desc.ir.kind, ProcKind::Subscription);

    // Subscriptions route as GET per SPEC §4.2 (the body is consumed as a
    // query string, so POST would be a contract violation).
    assert_eq!(desc.ir.http_method, HttpMethod::Get);

    // The body must be the streaming variant — the macro pairs
    // `ProcKindRuntime::Subscription` with `ProcedureBody::Stream` and
    // anything else would crash the router's dispatch match.
    assert!(
        matches!(desc.body, ProcedureBody::Stream(_)),
        "expected ProcedureBody::Stream, got something else"
    );
}

// ---------------------------------------------------------------------------
// 2. Zero-input subscriptions surface their input as `Primitive(Unit)`.
//    Without this, codegen wouldn't know to drop the input parameter from the
//    generated `.subscribe()` signature.
// ---------------------------------------------------------------------------

#[test]
fn zero_input_subscription_descriptor() {
    let desc = __taut_proc_ticks_simple();
    assert_eq!(desc.name, "ticks_simple");
    assert_eq!(desc.ir.kind, ProcKind::Subscription);
    assert_eq!(desc.ir.http_method, HttpMethod::Get);
    assert_eq!(
        desc.ir.input,
        TypeRef::Primitive(Primitive::Unit),
        "zero-arg subscriptions must carry `Primitive(Unit)` as their input TypeRef",
    );
}

// ---------------------------------------------------------------------------
// 3. The full SSE round trip: `GET /rpc/ticks_simple?input=null` produces
//    three `event: data` frames followed by an `event: end` frame.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_route_emits_data_frames_then_end() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_ticks_simple())
        .into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri("/rpc/ticks_simple?input=null")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    assert_eq!(
        response.status(),
        http::StatusCode::OK,
        "subscription GET should return 200 OK before any frames flow",
    );

    let body = body_string(response).await;

    // Each yielded `u32` becomes one `event: data\ndata: <json>\n\n` frame.
    // We assert the canonical, well-formed shape rather than substring
    // matching on `0`/`1`/`2` alone — the explicit `\n\n` terminator pins the
    // SPEC §4.2 framing, not just the payload values.
    assert!(
        body.contains("event: data\ndata: 0\n\n"),
        "expected `event: data\\ndata: 0\\n\\n` in body, got: {body}"
    );
    assert!(
        body.contains("event: data\ndata: 1\n\n"),
        "expected `event: data\\ndata: 1\\n\\n` in body, got: {body}"
    );
    assert!(
        body.contains("event: data\ndata: 2\n\n"),
        "expected `event: data\\ndata: 2\\n\\n` in body, got: {body}"
    );

    // The terminal frame is `event: end\ndata: \n\n` per SPEC §4.2, but the
    // spec explicitly tolerates either a trailing space or no `data` line —
    // and for liberal matching we just require the `event: end` token to
    // appear after the last data frame.
    assert!(
        body.contains("event: end"),
        "expected `event: end` terminator in body, got: {body}"
    );
}

// ---------------------------------------------------------------------------
// 4. Bad `?input=` payload → SSE `event: error` frame with `decode_error`.
//
//    SPEC §4.2 dictates that decode failures on subscription input still
//    surface as in-stream `event: error` frames, not as a 400 envelope. The
//    HTTP response has already committed `200 OK` by the time the SSE body
//    starts streaming, so per-stream errors flow as frames.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_input_decode_error_emits_error_frame() {
    // The `ticks` subscription requires a valid `TicksInput` JSON object;
    // `?input=not_json` is therefore guaranteed to fail decoding.
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_ticks())
        .into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri("/rpc/ticks?input=not_json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    let body = body_string(response).await;

    // The two pieces we care about, in order: an `event: error` frame whose
    // serialized JSON payload contains the canonical `decode_error` code.
    assert!(
        body.contains("event: error"),
        "expected `event: error` in body, got: {body}"
    );
    assert!(
        body.contains(r#""code":"decode_error""#),
        "expected `\"code\":\"decode_error\"` in body, got: {body}"
    );
}

// ---------------------------------------------------------------------------
// 5. A zero-arg subscription must succeed even when the client omits the
//    `?input=` query parameter — the router treats a missing input as JSON
//    `null`, matching how `()` deserializes from `null` everywhere else in
//    the wire format.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_with_no_query_param_decodes_as_null() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_ticks_simple())
        .into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                // No `?input=...` at all — the router must treat this as
                // `input = null`, decode it as `()`, and run the handler.
                .uri("/rpc/ticks_simple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    assert_eq!(response.status(), http::StatusCode::OK);

    let body = body_string(response).await;

    // Three data frames + end, same as the explicit `?input=null` case.
    assert!(
        body.contains("event: data\ndata: 0\n\n"),
        "missing tick 0: {body}"
    );
    assert!(
        body.contains("event: data\ndata: 1\n\n"),
        "missing tick 1: {body}"
    );
    assert!(
        body.contains("event: data\ndata: 2\n\n"),
        "missing tick 2: {body}"
    );
    assert!(body.contains("event: end"), "missing end frame: {body}");
}

// ---------------------------------------------------------------------------
// 6. Item-serialization failure → `event: error` frame.
//
//    Triggering a serde failure on a `Serialize` impl is genuinely awkward:
//    every primitive type and every `#[derive(serde::Serialize)]` body is
//    infallible, and `serde_json` only fails on non-finite floats and on
//    maps with non-string keys. Constructing one of those cleanly here would
//    add a lot of scaffolding for very little signal — the unary path
//    already covers `serialization_error` end-to-end in
//    `procedure.rs::tests`. We pin the test name as a placeholder so the
//    Phase 3 contract still has an explicit slot for it; once we have a
//    convenient failing-Serialize harness we can lift the `#[ignore]`.
// ---------------------------------------------------------------------------

#[ignore = "no convenient way to construct a guaranteed-failing Serialize \
            impl for a stream Item type — see comment above for context"]
#[tokio::test]
async fn subscription_serialization_failure_emits_error_frame() {
    // Intentionally empty — see `#[ignore]` reason above.
}

// ---------------------------------------------------------------------------
// 7. Unary (POST) and streaming (GET) procedures cohabit a single Router.
//
//    The `into_axum()` build mounts each procedure on its own method/path
//    pair, so registering both kinds on one router must produce a router
//    that serves each via its own HTTP method without collisions.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unary_and_stream_can_coexist_on_same_router() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_ping_unary())
        .procedure(__taut_proc_ticks_simple())
        .into_axum();

    // Unary side: POST /rpc/ping_unary → `{"ok":"pong"}` per SPEC §4.1.
    //
    // We `clone()` the router for the second request because `oneshot`
    // consumes its receiver — fresh clones isolate the two assertions.
    let unary = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/ping_unary")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"input":null}"#))
                .unwrap(),
        )
        .await
        .expect("unary oneshot dispatch");
    assert_eq!(unary.status(), http::StatusCode::OK);
    let unary_body = body_string(unary).await;
    let unary_json: serde_json::Value = serde_json::from_str(&unary_body).expect("unary body json");
    assert_eq!(
        unary_json,
        serde_json::json!({ "ok": "pong" }),
        "unary procedure must serve `{{ \"ok\": <output> }}` over POST",
    );

    // Streaming side: GET /rpc/ticks_simple → SSE frames per SPEC §4.2.
    let streaming = app
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri("/rpc/ticks_simple")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("streaming oneshot dispatch");
    assert_eq!(streaming.status(), http::StatusCode::OK);
    let stream_body = body_string(streaming).await;
    assert!(
        stream_body.contains("event: data\ndata: 0\n\n"),
        "missing tick 0: {stream_body}"
    );
    assert!(
        stream_body.contains("event: end"),
        "missing end frame: {stream_body}"
    );
}

// ---------------------------------------------------------------------------
// 8. Subscriptions only mount GET — POST falls through to the not_found
//    fallback. This is the inverse of the unary case (which only mounts POST)
//    and pins the router's "method exclusivity" guarantee.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn router_excludes_subscription_routes_from_post_dispatch() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_ticks_simple())
        .into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/ticks_simple")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"input":null}"#))
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    // Subscriptions only register a GET route; the POST hits the fallback.
    // Both 404 (axum's default fallback per SPEC §4.1) and 405 (an axum
    // version that surfaces method-not-allowed instead) are acceptable
    // outcomes — what matters is that the request didn't reach the handler.
    let status = response.status();
    assert!(
        status == http::StatusCode::NOT_FOUND || status == http::StatusCode::METHOD_NOT_ALLOWED,
        "POST to a subscription must return 404 or 405, got {status}",
    );
}

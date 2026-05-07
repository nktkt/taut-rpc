//! Phase 2 example: `tower-http`'s `TraceLayer` plugged into a taut-rpc
//! `Router` via `Router::layer(...)`.
//!
//! The point of this example is to show that taut-rpc is *axum-native, not
//! axum-locked* (SPEC §1, goal 4): once you've registered procedures, the
//! returned `Router` is just a `tower::Service` builder, so the entire axum
//! middleware ecosystem — including `tower-http::trace::TraceLayer` — applies
//! without any taut-specific glue. SPEC §5: "Middleware: standard
//! `tower::Layer`s. Auth and tracing reuse axum's ecosystem rather than
//! reinventing."
//!
//! Three things happen on every request:
//!
//! 1. `TraceLayer` opens an HTTP-level span carrying the method, URI, version,
//!    and (on response) status + latency. That's the request-level structured
//!    log — emitted at `INFO`, with sub-`DEBUG` events for `tower_http=debug`
//!    consumers who want the per-request "started processing"/"finished
//!    processing" trace.
//! 2. Each procedure carries a `#[tracing::instrument]` attribute so its body
//!    runs inside its own span with the input fields as structured fields.
//!    Because `tracing` propagates spans through the async runtime, the
//!    procedure span nests *inside* the per-request HTTP span automatically.
//! 3. The Phase 4 `Validate` pipeline runs *before* the procedure body. When a
//!    request fails validation (e.g. `add` with `lhs` out of range) the
//!    rejection is rendered into the standard `validation_error` envelope
//!    *inside* the `TraceLayer` span, so the failed request still carries
//!    method/URI/status/latency in its log line.
//!
//! Run:
//!
//! ```sh
//! cd examples/phase2-tracing/server
//! RUST_LOG=info,tower_http=debug cargo run
//! ```
//!
//! Then in another terminal:
//!
//! ```sh
//! # echo — primitive input, no validation
//! curl -X POST http://127.0.0.1:7703/rpc/echo \
//!   -H 'content-type: application/json' \
//!   -d '{"input":"hello"}'
//!
//! # add — Validate-derived input with min/max constraints
//! curl -X POST http://127.0.0.1:7703/rpc/add \
//!   -H 'content-type: application/json' \
//!   -d '{"input":{"lhs":2,"rhs":3}}'
//!
//! # slow_op — no input; sleeps 100ms so latency reporting is non-trivial
//! curl -X POST http://127.0.0.1:7703/rpc/slow_op \
//!   -H 'content-type: application/json' \
//!   -d '{}'
//! ```
//!
//! and watch the server log: each request opens an HTTP span, the procedure's
//! own span nests under it carrying the input fields, and the response line
//! carries the status + latency in milliseconds.

use serde::{Deserialize, Serialize};
use taut_rpc::{dump_if_requested, rpc, Router, Type, Validate};
use tower::ServiceBuilder;
use tracing_subscriber::EnvFilter;

// --- echo --------------------------------------------------------------------

/// Trivial procedure used to demonstrate that user-emitted `tracing` events
/// nest inside the per-request HTTP span set up by `TraceLayer`. The `String`
/// input is a primitive, so per the Phase 4 blanket impls it does *not* need
/// `#[derive(Validate)]` — the auto-impl on primitives passes everything.
///
/// The `#[tracing::instrument]` attribute opens a child span named `echo`
/// with `input` recorded as a structured field; the `tracing::info!` event
/// fires inside that span, which itself nests under the `TraceLayer` request
/// span.
#[rpc]
#[tracing::instrument(level = "info", fields(input = %input))]
async fn echo(input: String) -> String {
    tracing::info!("echo called");
    input
}

// --- add ---------------------------------------------------------------------

/// Input for `add`. Each field carries a Phase 4 numeric constraint so the
/// validation tracing path is exercised — a request with `lhs = 9999`
/// short-circuits to the standard `validation_error` envelope before
/// `add`'s body runs, and the failed-request log line still nests under the
/// `TraceLayer` span.
#[derive(Serialize, Deserialize, Type, Validate)]
pub struct AddInput {
    /// 0..=1000; demonstrates `min`/`max` on `i32`.
    #[taut(min = 0, max = 1000)]
    pub lhs: i32,
    /// Same window; pairs with `lhs` so we can show two-field error reports
    /// when both are out of range.
    #[taut(min = 0, max = 1000)]
    pub rhs: i32,
}

/// Adds `lhs + rhs` and emits a `tracing::info!` event recording both
/// inputs and the computed sum. Because the `#[tracing::instrument]`
/// attribute records the input fields on the span itself, the sum event
/// inside the body only needs to carry the *new* information (`sum`).
#[rpc]
#[tracing::instrument(level = "info", fields(lhs = input.lhs, rhs = input.rhs))]
async fn add(input: AddInput) -> i32 {
    let sum = input.lhs + input.rhs;
    tracing::info!(sum, "add computed");
    sum
}

// --- slow_op -----------------------------------------------------------------

/// No-input procedure that sleeps 100ms before returning a value. The point
/// is to show non-trivial latency in the `TraceLayer` response log line —
/// with `LatencyUnit::Millis` configured, you'll see `latency=10X ms` on the
/// `finished processing request` event, plus an explicit `slept` event from
/// the procedure's own span confirming the body actually waited.
#[rpc]
#[tracing::instrument(level = "info")]
async fn slow_op() -> u64 {
    let start = std::time::Instant::now();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let elapsed_ms = start.elapsed().as_millis() as u64;
    tracing::info!(elapsed_ms, "slow_op slept");
    elapsed_ms
}

// --- main --------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // `EnvFilter::from_default_env()` reads `RUST_LOG`, so the run instructions
    // in the README (`RUST_LOG=info,tower_http=debug`) drive verbosity without
    // any code changes. Including thread ids and the event target makes it
    // obvious which task / module each event came from when many requests are
    // in flight at once.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_thread_ids(true)
        .with_target(true)
        .init();

    // Compose the middleware stack with `tower::ServiceBuilder` so adding a
    // second layer later is a one-line edit. With a single layer this is
    // equivalent to `.layer(TraceLayer::...)` directly, but using
    // `ServiceBuilder` here documents the canonical "stacked layers" pattern
    // for anyone copying this example as a starting point.
    let trace_stack = ServiceBuilder::new().layer(
        tower_http::trace::TraceLayer::new_for_http()
            .make_span_with(tower_http::trace::DefaultMakeSpan::default().include_headers(false))
            .on_request(
                tower_http::trace::DefaultOnRequest::default().level(tracing::Level::INFO),
            )
            .on_response(
                tower_http::trace::DefaultOnResponse::default()
                    .level(tracing::Level::INFO)
                    .latency_unit(tower_http::LatencyUnit::Millis),
            ),
    );

    let router = Router::new()
        .procedure(__taut_proc_echo())
        .procedure(__taut_proc_add())
        .procedure(__taut_proc_slow_op())
        // `Router::layer(...)` records the layer; it's applied to the built
        // axum router at `into_axum()` time. Ordering note: the `TraceLayer`
        // wraps the *whole* router, so even taut-rpc's built-in not_found /
        // decode_error / validation_error responses are traced.
        .layer(trace_stack);

    // If `TAUT_DUMP_IR` is set, write the IR and exit before binding any port.
    // This keeps the example compatible with `cargo taut gen --from-binary`
    // even though the demo focuses on the runtime tracing behaviour.
    dump_if_requested(&router);

    let app = router.into_axum();

    let listener = tokio::net::TcpListener::bind("0.0.0.0:7703")
        .await
        .expect("bind 0.0.0.0:7703");

    tracing::info!("phase2-tracing-server listening on http://127.0.0.1:7703");

    axum::serve(listener, app).await.expect("server crashed");
}

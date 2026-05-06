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
//! Two things happen on every request:
//!
//! 1. `TraceLayer` opens an HTTP-level span carrying the method, URI, version,
//!    and (on response) status + latency. That's the request-level structured
//!    log — emitted at `INFO`, with sub-`DEBUG` events for `tower_http=debug`
//!    consumers who want the per-request "started processing"/"finished
//!    processing" trace.
//! 2. The `echo` procedure emits its own `tracing::info!` event. Because
//!    `tracing` propagates the current span through the async runtime, that
//!    event is recorded *inside* the request span automatically — no manual
//!    span instrumentation on the procedure required.
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
//! curl -X POST http://127.0.0.1:7703/rpc/echo \
//!   -H 'content-type: application/json' \
//!   -d '{"input":"hello"}'
//! ```
//!
//! and watch the server log: each request opens an HTTP span, the procedure's
//! `echo called` event nests under it, and the response line carries the
//! status + latency in milliseconds.

use taut_rpc::{dump_if_requested, rpc, Router};
use tracing_subscriber::EnvFilter;

// --- echo --------------------------------------------------------------------

/// Trivial procedure used to demonstrate that user-emitted `tracing` events
/// nest inside the per-request HTTP span set up by `TraceLayer`. The `?input`
/// uses `tracing`'s `Debug` field syntax so the value shows up as a structured
/// field on the event rather than being interpolated into the message.
#[rpc]
async fn echo(input: String) -> String {
    tracing::info!(?input, "echo called");
    input
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

    let router = Router::new()
        .procedure(__taut_proc_echo())
        // `Router::layer(...)` records the layer; it's applied to the built
        // axum router at `into_axum()` time. Ordering note: the `TraceLayer`
        // wraps the *whole* router, so even taut-rpc's built-in not_found /
        // decode_error responses are traced. The handful of explicit
        // `Default*` configurations below pin the levels and latency unit so
        // the demo log output is stable regardless of `tower-http`'s defaults
        // drifting between minor versions.
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(
                    tower_http::trace::DefaultMakeSpan::default().include_headers(false),
                )
                .on_request(
                    tower_http::trace::DefaultOnRequest::default().level(tracing::Level::INFO),
                )
                .on_response(
                    tower_http::trace::DefaultOnResponse::default()
                        .level(tracing::Level::INFO)
                        .latency_unit(tower_http::LatencyUnit::Millis),
                ),
        );

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

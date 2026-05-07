//! Phase 3 example server — `#[rpc(stream)]` + SSE end-to-end.
//!
//! Demonstrates the streaming procedure kind: an async function whose return
//! type is `impl futures::Stream<Item = T> + Send + 'static`, marked with
//! `#[rpc(stream)]`. The macro registers it as a subscription procedure on
//! the SSE transport (SPEC §4.2), and the codegen emits a TS handle whose
//! `.subscribe(input)` returns an `AsyncIterable<T>`.
//!
//! The classic Phase 3 exit case lives here: a counter that ticks once a
//! second is observable from a TS `for await`. We also include a
//! zero-input subscription (`server_time`) so the no-arg shape is exercised,
//! and a plain unary `ping()` for sanity.

use serde::{Deserialize, Serialize};
use taut_rpc::{dump_if_requested, rpc, Router, Type, Validate};
use tower_http::cors::CorsLayer;

// --- ping --------------------------------------------------------------------

/// Sanity-check unary query. Subscriptions and queries coexist on the same
/// router; this proves Phase 3 didn't regress the unary path.
#[rpc]
async fn ping() -> &'static str {
    "pong"
}

// --- ticks -------------------------------------------------------------------

/// Input for `ticks`. `count` is the number of values to emit; `interval_ms`
/// is the gap between them. A separate input struct rather than two scalar
/// args because v0.1's `#[rpc]` accepts 0 or 1 input — multi-arg forms are
/// expressed as a struct.
///
/// Phase 4 adds `taut_rpc::Validate` with bounds: `count` is capped at 100
/// to keep an accidental misuse from looking like an infinite stream, and
/// `interval_ms` is constrained to `[10, 60_000]` so neither a tight loop
/// (1ms) nor an unbounded gap (hours) slips through. Out-of-range inputs
/// are rejected on the server with `validation_error` before the stream
/// is started, so the client never sees a partial sequence.
#[derive(Serialize, Deserialize, Type, Validate)]
pub struct TicksInput {
    /// How many values to emit (`0..count`). Defaults to 5 in the demo
    /// client; codegen surfaces this as `bigint` since u64 maps to bigint
    /// per SPEC §3.1.
    #[taut(min = 1, max = 100)]
    pub count: u32,
    /// Milliseconds to wait between values. Defaults to 1000 in the demo.
    #[taut(min = 10, max = 60_000)]
    pub interval_ms: u32,
}

/// The headline subscription: emits `0..input.count`, sleeping `interval_ms`
/// between values. The first value is emitted immediately so a client with
/// `count = 1` doesn't have to wait an interval to see anything.
///
/// Returning `impl Stream<Item = u64> + Send + 'static` is the canonical
/// `#[rpc(stream)]` shape — `async-stream::stream!` keeps the body readable
/// while letting us await between yields.
#[rpc(stream)]
async fn ticks(input: TicksInput) -> impl futures::Stream<Item = u64> + Send + 'static {
    async_stream::stream! {
        let interval = std::time::Duration::from_millis(u64::from(input.interval_ms));
        for i in 0..u64::from(input.count) {
            if i > 0 {
                tokio::time::sleep(interval).await;
            }
            yield i;
        }
    }
}

// --- server_time -------------------------------------------------------------

/// Zero-input subscription: emits an ISO-8601 timestamp every second for
/// three seconds. The point is to exercise the no-arg `.subscribe()` shape
/// on the TS side — codegen drops the input parameter when the procedure
/// has none.
///
/// Returns `String` per item (SPEC §3.1: `chrono::DateTime` is feature-gated
/// and we don't want to pull `chrono` in for a demo, so we hand-format an
/// RFC 3339 string from the system clock).
#[rpc(stream)]
async fn server_time() -> impl futures::Stream<Item = String> + Send + 'static {
    async_stream::stream! {
        let interval = std::time::Duration::from_secs(1);
        for i in 0..3u64 {
            if i > 0 {
                tokio::time::sleep(interval).await;
            }
            yield iso8601_now();
        }
    }
}

/// Format the current `SystemTime` as an RFC 3339 / ISO-8601 string in UTC,
/// truncated to whole seconds. We avoid pulling in `chrono` or `time` here —
/// this is a 30-line implementation and a demo doesn't need a date crate.
fn iso8601_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Days from 1970-01-01 (Thursday).
    let days = (secs / 86_400) as i64;
    let mut sod = secs % 86_400;
    let hour = sod / 3600;
    sod %= 3600;
    let minute = sod / 60;
    let second = sod % 60;

    // Civil-from-days, Hinnant 2013. Returns (year, month, day) for any day
    // index counted from 1970-01-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, hour, minute, second
    )
}

// --- main --------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let router = Router::new()
        .procedure(__taut_proc_ping())
        .procedure(__taut_proc_ticks())
        .procedure(__taut_proc_server_time());

    // If `TAUT_DUMP_IR` is set, write the IR and exit before binding any port.
    // `cargo taut gen --from-binary` relies on this to extract the IR without
    // needing a working network or filesystem outside the IR target path.
    dump_if_requested(&router);

    let app = router.into_axum().layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:7704")
        .await
        .expect("bind 0.0.0.0:7704");

    println!("phase3-counter-server listening on http://127.0.0.1:7704");

    axum::serve(listener, app).await.expect("server crashed");
}

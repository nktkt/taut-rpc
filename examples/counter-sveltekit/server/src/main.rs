//! Phase 5 example server — full-stack axum + SvelteKit Counter demo.
//!
//! The server exposes a tiny in-memory counter with the four classic shapes
//! a real app needs:
//!
//!   - `current() -> u64`                       — read the current value.
//!   - `increment(IncrementInput) -> u64`       — bump by N (validated
//!                                                 1..=1000).
//!   - `reset() -> u64`                         — set to 0.
//!   - `live() -> impl Stream<Item = u64>`      — subscription that emits the
//!                                                 current value on every
//!                                                 change so the UI tracks
//!                                                 mutations from any tab.
//!
//! State is a `Mutex<u64>` plus a `tokio::sync::broadcast::Sender<u64>`. Each
//! mutation takes the lock, updates the value, and publishes the new value to
//! the broadcast channel. `live()` snapshots the current value, then forwards
//! everything the channel emits — late subscribers always see the latest
//! value first, so a freshly opened browser tab is never "blank until the
//! next mutation".
//!
//! v0.1's `#[rpc]` attribute does not yet thread an explicit state handle to
//! the procedure body (the doc comment on `Router` calls this out as a Phase
//! 2+ TODO). Until that lands, examples that need shared state reach for a
//! `OnceLock<AppState>` initialised in `main`. This is the same pattern
//! `examples/phase3-counter` will graduate to once it stops being purely
//! ephemeral; encoding it once here means the SvelteKit demo is the
//! reference for "real" v0.1 servers with state.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use taut_rpc::{dump_if_requested, rpc, Router, Type, Validate};
use tokio::sync::{broadcast, Mutex};
use tower_http::cors::CorsLayer;

// --- shared state ------------------------------------------------------------

/// Process-wide counter state.
///
/// `value` is the source of truth (held under a mutex so concurrent mutations
/// serialise cleanly), and `tx` is the broadcast hub: every mutation publishes
/// the *new* value so any number of `live()` subscribers can observe changes
/// without polling.
///
/// Capacity 64 is picked so a slow subscriber that lags behind a flurry of
/// increments still gets the latest value once it catches up; broadcast
/// channels drop the oldest items when the buffer overflows, and the next
/// mutation will republish the canonical value anyway.
struct AppState {
    value: Mutex<u64>,
    tx: broadcast::Sender<u64>,
}

/// Lazily-initialised global handle. We cannot pass state through the v0.1
/// `#[rpc]` macro yet (see module comment), so the procedure bodies reach
/// into this `OnceLock` via [`state()`]. `main` is the only writer.
static STATE: OnceLock<AppState> = OnceLock::new();

/// Accessor for the shared state. Panics with a clear message if called
/// before `main` has initialised `STATE` — that would only happen if a
/// procedure body somehow ran during static init, which the runtime
/// doesn't permit.
fn state() -> &'static AppState {
    STATE.get().expect("STATE not initialised; main must run before procedures")
}

// --- inputs ------------------------------------------------------------------

/// Input for `increment`. Wrapping a single scalar in a struct rather than
/// taking it as a bare `u64` argument is the v0.1 style — see SPEC §3.1 and
/// `examples/phase3-counter`'s `TicksInput`.
///
/// `by` is constrained to `[1, 1000]` so the demo can exercise the Phase 4
/// validation pipeline end-to-end without an "increment by 0 is fine" edge
/// case muddying the contract. The bounds are arbitrary but useful: an
/// increment of zero is the kind of accidental no-op a real API would
/// reject, and 1000 keeps a slipped finger on the keypad from sending the
/// counter to the moon.
#[derive(Serialize, Deserialize, Type, Validate)]
pub struct IncrementInput {
    /// Amount to add to the counter. Must be `1..=1000`. Uses `u32` because
    /// `taut_rpc::validate::check::min`/`max` require `Into<f64>` which `u64`
    /// can't satisfy losslessly.
    #[taut(min = 1, max = 1000)]
    pub by: u32,
}

// --- procedures --------------------------------------------------------------

/// Read-only query: returns the current counter value without mutating
/// anything. Used by the UI on first load (and as a "is the server up?"
/// probe before opening the SSE subscription).
#[rpc]
async fn current() -> u64 {
    *state().value.lock().await
}

/// Mutation: add `input.by` to the counter, broadcast the new value, return
/// it. The `#[rpc(mutation)]` attribute is what flips the procedure kind to
/// `Mutation` in the IR — without it the codegen would emit a `query`,
/// which is fine over the wire but drops the semantic distinction.
///
/// Saturating add so the demo never wraps to zero on overflow. v0.1 has no
/// overflow error type to propagate; the practical ceiling for `u64::MAX`
/// is so far above the validated `max = 1000` per call that hitting it
/// would take more clicks than a human lifetime supports.
#[rpc(mutation)]
async fn increment(input: IncrementInput) -> u64 {
    let s = state();
    let mut v = s.value.lock().await;
    *v = v.saturating_add(u64::from(input.by));
    let new = *v;
    // `send` returns `Err` only when there are zero subscribers; that's the
    // common case before any client connects, so we ignore the result.
    let _ = s.tx.send(new);
    new
}

/// Mutation: set the counter to 0 and broadcast. Same shape as `increment`
/// but with no input — the codegen will emit a zero-arg call on the TS
/// side.
#[rpc(mutation)]
async fn reset() -> u64 {
    let s = state();
    let mut v = s.value.lock().await;
    *v = 0;
    let new = *v;
    let _ = s.tx.send(new);
    new
}

/// Subscription: emit the current value once on subscribe, then every
/// subsequent broadcast. Returning `impl Stream<Item = u64> + Send +
/// 'static` is the canonical `#[rpc(stream)]` shape, and `async-stream`
/// keeps the body readable while letting us await between yields.
///
/// We snapshot the value *after* subscribing to the channel, in that order:
/// if we snapshotted first and a mutation slipped in between the snapshot
/// and the `subscribe()` call, we'd miss the publish for that mutation and
/// the UI would lag by one update until the next change. Subscribing first
/// is the standard "register the listener before reading the state" pattern
/// for race-free fan-out.
#[rpc(stream)]
async fn live() -> impl futures::Stream<Item = u64> + Send + 'static {
    let s = state();
    let mut rx = s.tx.subscribe();
    let snapshot = *s.value.lock().await;
    async_stream::stream! {
        yield snapshot;
        while let Ok(v) = rx.recv().await {
            yield v;
        }
    }
}

// --- main --------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Initialise the shared state before any procedure body can possibly run.
    // `set` is fallible only if `STATE` was already populated, which can't
    // happen here because `main` is the sole writer.
    let (tx, _rx) = broadcast::channel::<u64>(64);
    STATE
        .set(AppState {
            value: Mutex::new(0),
            tx,
        })
        .map_err(|_| "STATE already initialised")
        .expect("first set");

    let router = Router::new()
        .procedure(__taut_proc_current())
        .procedure(__taut_proc_increment())
        .procedure(__taut_proc_reset())
        .procedure(__taut_proc_live());

    // If `TAUT_DUMP_IR` is set, write the IR and exit before binding any port.
    // `cargo taut gen --from-binary` relies on this to extract the IR without
    // needing a working network or filesystem outside the IR target path.
    dump_if_requested(&router);

    let app = router.into_axum().layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:7712")
        .await
        .expect("bind 0.0.0.0:7712");

    println!("counter-sveltekit-server listening on http://127.0.0.1:7712");

    axum::serve(listener, app).await.expect("server crashed");
}

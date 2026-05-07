//! Phase 5 example server — full-stack Todo over taut-rpc.
//!
//! This is the first example wired into a real frontend stack (Vite + React)
//! rather than a Node script. The wire surface stays minimal — three
//! procedures plus one subscription — so the interesting bit is end-to-end
//! plumbing, not procedure breadth:
//!
//!   - `list_todos() -> Vec<Todo>`               (query)
//!   - `create_todo(CreateTodoInput) -> Todo`    (mutation, validated)
//!   - `complete_todo(CompleteTodoInput) -> Todo`(mutation)
//!   - `todos_changed() -> impl Stream<Vec<Todo>>` (subscription)
//!
//! State lives in a `tokio::sync::RwLock<HashMap<u64, Todo>>` accessed via a
//! `OnceLock` (`#[rpc]` procedures are free functions in v0.1; Phase 6 will
//! reintroduce `with_state` per SPEC §5). A `tokio::sync::broadcast` channel
//! fans out the entire list on every mutation, and `todos_changed` adapts
//! the receiver into an `impl Stream` via `BroadcastStream`. Lagged frames
//! are dropped silently — the next mutation will re-broadcast the same list,
//! so an SSE client that misses a frame self-heals on the next change.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use taut_rpc::{dump_if_requested, rpc, Router, Type, Validate};
use tokio::sync::{broadcast, RwLock};
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::CorsLayer;

// --- Todo --------------------------------------------------------------------

/// A single todo item. `id` is `u32` rather than `u64` for the same reason as
/// phase4-validate's `User.id` — codegen lowers `u64` to `bigint` and our
/// JSON wire format ships ids as plain JS numbers, so v0.1 sticks to the
/// number-domain integer types until v0.2 lands the bigint coercion adapter.
#[derive(Clone, Serialize, Deserialize, Type)]
pub struct Todo {
    pub id: u32,
    pub title: String,
    pub completed: bool,
}

// --- inputs ------------------------------------------------------------------

/// Input for `create_todo`. Title is `length(1..200)`: empty titles are
/// rejected client-side by the generated Valibot schema (no network call),
/// and the same constraint runs server-side via `Validate::validate` before
/// the procedure body sees the input. Rejecting both ends with one
/// `#[derive(Validate)]` is the entire Phase 4 promise.
#[derive(Serialize, Deserialize, Type, Validate)]
pub struct CreateTodoInput {
    /// 1..=199 characters. The lower bound is the meaningful one (no empty
    /// todos); the upper bound is a sanity cap to keep the demo's UI sane.
    #[taut(length(min = 1, max = 199))]
    pub title: String,
}

/// Input for `complete_todo`. The client passes the full desired state of
/// `completed` rather than a "toggle" flag so the operation is idempotent —
/// the React UI dispatches based on the current row state, and replaying the
/// same request twice is a no-op.
#[derive(Serialize, Deserialize, Type, Validate)]
pub struct CompleteTodoInput {
    pub id: u32,
    pub completed: bool,
}

// --- shared state ------------------------------------------------------------

/// Process-wide state. v0.1 procedures are free functions, so we reach state
/// through a `OnceLock` initialised in `main` rather than via a `with_state`
/// thread. The lock-then-broadcast pattern below assumes a single writer at
/// a time: we hold the write lock across the `insert`, drop it, then send
/// the new snapshot. `broadcast::Sender::send` is non-blocking and drops the
/// frame for any receiver that's lagged past the channel capacity — that is
/// the desired behaviour here, since the next mutation will re-broadcast.
struct AppState {
    todos: RwLock<HashMap<u32, Todo>>,
    next_id: RwLock<u32>,
    notify: broadcast::Sender<Vec<Todo>>,
}

static STATE: OnceLock<AppState> = OnceLock::new();

fn state() -> &'static AppState {
    STATE.get().expect("STATE not initialised")
}

/// Snapshot the current todos as a `Vec`, sorted by `id` ascending. The UI
/// expects a stable order, and `HashMap` iteration order is unspecified.
async fn snapshot_todos() -> Vec<Todo> {
    let map = state().todos.read().await;
    let mut v: Vec<Todo> = map.values().cloned().collect();
    v.sort_by_key(|t| t.id);
    v
}

/// Take a snapshot and broadcast it. Called after every mutation so any
/// `todos_changed` subscriber sees the new state. The send result is
/// intentionally ignored: if no subscribers are connected, `send` returns
/// `Err(SendError)` — which is fine, the next subscriber will pull a fresh
/// snapshot when they call `list_todos`.
async fn broadcast_change() {
    let snap = snapshot_todos().await;
    let _ = state().notify.send(snap);
}

// --- procedures --------------------------------------------------------------

/// Return the current todo list. Pure read; never broadcasts.
#[rpc]
async fn list_todos() -> Vec<Todo> {
    snapshot_todos().await
}

/// Create a new todo and broadcast the new list. The `Validate` impl on
/// `CreateTodoInput` runs *before* this body — by the time we get here,
/// `input.title` is in `1..=199`. Returns the created `Todo` so the UI
/// can render it without waiting for the broadcast round-trip.
#[rpc(mutation)]
async fn create_todo(input: CreateTodoInput) -> Todo {
    let id = {
        let mut next = state().next_id.write().await;
        let id = *next;
        *next = next.saturating_add(1);
        id
    };
    let todo = Todo {
        id,
        title: input.title,
        completed: false,
    };
    {
        let mut map = state().todos.write().await;
        map.insert(id, todo.clone());
    }
    broadcast_change().await;
    todo
}

/// Toggle a todo's `completed` flag and broadcast the new list. If the id
/// doesn't exist this is a silent no-op that returns a synthetic "not found"
/// `Todo` — the demo's UI never produces stale ids, and we don't want to
/// teach the typed-error path here (phase4-validate already covers it).
/// A real app would surface a `#[derive(TautError)]` enum.
#[rpc(mutation)]
async fn complete_todo(input: CompleteTodoInput) -> Todo {
    let updated = {
        let mut map = state().todos.write().await;
        if let Some(t) = map.get_mut(&input.id) {
            t.completed = input.completed;
            t.clone()
        } else {
            // Best-effort: return a placeholder so the demo doesn't need an
            // error envelope. Real apps return a typed error here.
            Todo {
                id: input.id,
                title: String::new(),
                completed: input.completed,
            }
        }
    };
    broadcast_change().await;
    updated
}

/// Subscribe to the full todo list. Emits the current snapshot once on
/// connection (so a new subscriber doesn't have to wait for the next
/// mutation to render) and then re-broadcasts on every change.
///
/// `BroadcastStream` adapts a `broadcast::Receiver` into a `Stream` whose
/// `Item` is `Result<T, BroadcastStreamRecvError>` — we filter the lagged
/// case out and yield only the live frames. Lagged subscribers will simply
/// miss intermediate frames; the next mutation re-broadcasts the whole
/// list, so the UI self-heals.
#[rpc(stream)]
async fn todos_changed() -> impl futures::Stream<Item = Vec<Todo>> + Send + 'static {
    let initial = snapshot_todos().await;
    let rx = state().notify.subscribe();

    async_stream::stream! {
        // Emit the current state immediately so a fresh subscriber renders
        // without waiting for someone else to mutate.
        yield initial;

        let mut stream = BroadcastStream::new(rx);
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            // Drop lagged frames; the next live frame will be a full snapshot
            // anyway, so the consumer's view re-converges automatically.
            if let Ok(snap) = item {
                yield snap;
            }
        }
    }
}

// --- main --------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Initialise process-wide state before any procedure can run. Capacity 64
    // is more than enough for a demo: every mutation broadcasts one frame and
    // the UI consumes them as fast as they arrive.
    let (tx, _rx) = broadcast::channel::<Vec<Todo>>(64);
    STATE
        .set(AppState {
            todos: RwLock::new(HashMap::new()),
            next_id: RwLock::new(1),
            notify: tx,
        })
        .ok()
        .expect("STATE already initialised");

    let router = Router::new()
        .procedure(__taut_proc_list_todos())
        .procedure(__taut_proc_create_todo())
        .procedure(__taut_proc_complete_todo())
        .procedure(__taut_proc_todos_changed());

    // If `TAUT_DUMP_IR` is set, write the IR and exit before binding any port.
    // `cargo taut gen --from-binary` relies on this to extract the IR without
    // needing a working network or filesystem outside the IR target path.
    dump_if_requested(&router);

    let app = router.into_axum().layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:7710")
        .await
        .expect("bind 0.0.0.0:7710");

    println!("todo-react-server listening on http://127.0.0.1:7710");

    axum::serve(listener, app).await.expect("server crashed");
}

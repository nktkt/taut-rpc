# Tutorial: Build a Todo app

A 14-step walkthrough from `cargo new` to a Vite + React Todo app with
typed RPC, validation on both sides, a live subscription, and a final
stretch step that gates the API behind a bearer token. Each step ends
with a **"what you should see"** box — don't skip those, they're how
you catch typos before they compound two steps later.

The finished code lives at [`examples/todo-react/`](https://github.com/x7/taut-rpc/tree/main/examples/todo-react).

## Prerequisites

Rust 1.75+, Node 20+, and `taut-rpc-cli` available either on your
`PATH` or via `cargo run -p taut-rpc-cli` from a checkout. We use the
latter form throughout.

## Step 1 — Project setup

```sh
mkdir todo-app && cd todo-app
cargo new --bin server
mkdir client
```

`server/Cargo.toml`:

```toml
[dependencies]
taut-rpc     = "0.1"
axum         = "0.7"
tokio        = { version = "1", features = ["full"] }
serde        = { version = "1", features = ["derive"] }
serde_json   = "1"
thiserror    = "1"
tower-http   = { version = "0.5", features = ["cors"] }
async-stream = "0.3"
futures      = "0.3"
tokio-stream = { version = "0.1", features = ["sync"] }
```

`server/src/main.rs` — a stub procedure to confirm the toolchain works:

```rust
use taut_rpc::{dump_if_requested, rpc, Router};
use tower_http::cors::CorsLayer;

#[rpc]
async fn ping() -> &'static str { "pong" }

#[tokio::main]
async fn main() {
    let router = Router::new().procedure(__taut_proc_ping());
    dump_if_requested(&router);
    let app = router.into_axum().layer(CorsLayer::permissive());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    println!("listening on http://127.0.0.1:8080");
    axum::serve(listener, app).await.unwrap();
}
```

**What you should see.** `cargo run` prints the listen line, and
`curl -X POST localhost:8080/rpc/ping -H 'content-type: application/json' -d '{}'`
returns `{"ok":"pong"}`.

## Step 2 — The `Todo` model

```rust
use serde::{Deserialize, Serialize};
use taut_rpc::Type;

#[derive(Serialize, Deserialize, Type, Clone, Debug)]
pub struct Todo {
    pub id: u32,
    pub title: String,
    pub completed: bool,
}
```

`#[derive(Type)]` records the shape into the IR; codegen emits a matching
`interface Todo` later. We use `u32` rather than `u64` so the TS side gets
`number` rather than `bigint` — fine for an in-memory list.

**What you should see.** `cargo build` is still clean. We're just
declaring the shape, no procedures yet.

## Step 3 — `list_todos` query

In-memory storage with `OnceLock<Mutex<...>>` — simple, no `State<S>`
plumbing required:

```rust
use std::sync::{Mutex, OnceLock};

static TODOS: OnceLock<Mutex<Vec<Todo>>> = OnceLock::new();
fn todos() -> &'static Mutex<Vec<Todo>> {
    TODOS.get_or_init(|| Mutex::new(Vec::new()))
}

#[rpc]
async fn list_todos() -> Vec<Todo> {
    todos().lock().unwrap().clone()
}
```

Register it in `main`:

```rust
let router = Router::new()
    .procedure(__taut_proc_ping())
    .procedure(__taut_proc_list_todos());
```

**What you should see.**
`curl -X POST localhost:8080/rpc/list_todos -H 'content-type: application/json' -d '{}'`
returns `{"ok":[]}` — an empty list, but typed.

## Step 4 — `create_todo` mutation with validation

Title must be non-empty and bounded so a 5MB paste doesn't end up in the
list:

```rust
use taut_rpc::Validate;

#[derive(Serialize, Deserialize, Type, Validate)]
pub struct CreateTodo {
    #[taut(length(min = 1, max = 200))]
    pub title: String,
}

#[derive(Serialize, Deserialize, Type, thiserror::Error, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum CreateTodoError {
    #[error("storage capacity exceeded")]
    CapacityExceeded,
}

#[rpc(mutation)]
async fn create_todo(input: CreateTodo) -> Result<Todo, CreateTodoError> {
    let mut list = todos().lock().unwrap();
    if list.len() >= 10_000 {
        return Err(CreateTodoError::CapacityExceeded);
    }
    let id = list.last().map(|t| t.id + 1).unwrap_or(1);
    let todo = Todo { id, title: input.title, completed: false };
    list.push(todo.clone());
    Ok(todo)
}
```

Add `.procedure(__taut_proc_create_todo())` to the router.

The `length(min = 1, max = 200)` attribute does double duty: the
macro-generated handler runs it server-side before `create_todo`
executes, and codegen lowers it to a Valibot schema so the *client*
also rejects bad input before it leaves the browser.

**What you should see.**
`curl -X POST localhost:8080/rpc/create_todo -H 'content-type: application/json' -d '{"input":{"title":"buy milk"}}'`
returns `{"ok":{"id":1,"title":"buy milk","completed":false}}`. Sending
`{"input":{"title":""}}` returns HTTP 422 with
`{"err":{"code":"validation_error","payload":{...}}}`.

## Step 5 — `complete_todo` mutation

```rust
#[derive(Serialize, Deserialize, Type, Validate)]
pub struct CompleteTodo { pub id: u32 }

#[derive(Serialize, Deserialize, Type, thiserror::Error, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum CompleteTodoError {
    #[error("no todo with id {id}")]
    NotFound { id: u32 },
}

#[rpc(mutation)]
async fn complete_todo(input: CompleteTodo) -> Result<Todo, CompleteTodoError> {
    let mut list = todos().lock().unwrap();
    let todo = list.iter_mut()
        .find(|t| t.id == input.id)
        .ok_or(CompleteTodoError::NotFound { id: input.id })?;
    todo.completed = true;
    Ok(todo.clone())
}
```

Register it. `NotFound { id }` carries a structured payload — the TS
client narrows it as `{ code: "not_found"; payload: { id: number } }`.

**What you should see.** Calling `complete_todo` with `id: 1` after a
successful `create_todo` returns the same row with `completed: true`.
With an unknown id, the response is
`{"err":{"code":"not_found","payload":{"id":99}}}` and HTTP 400.

## Step 6 — Wire the Router

The full `main` for the record:

```rust
#[tokio::main]
async fn main() {
    let router = Router::new()
        .procedure(__taut_proc_ping())
        .procedure(__taut_proc_list_todos())
        .procedure(__taut_proc_create_todo())
        .procedure(__taut_proc_complete_todo());

    dump_if_requested(&router);

    let app = router.into_axum().layer(CorsLayer::permissive());
    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    println!("todo-server listening on http://127.0.0.1:8080");
    axum::serve(listener, app).await.unwrap();
}
```

`CorsLayer::permissive()` is fine for local dev; tighten it for prod.

**What you should see.** `cargo run` prints the listen line and serves
all four procedures. `curl localhost:8080/rpc/_procedures` lists them.

## Step 7 — `cargo taut gen` for the first time

Bridge to TypeScript:

```sh
cd server && cargo build && cd ..
cargo taut gen \
  --from-binary server/target/debug/todo-server \
  --out         client/src/api.gen.ts
```

This sets `TAUT_DUMP_IR=1` and runs the binary; `dump_if_requested`
writes the IR and exits before binding the port. The CLI reads the IR
and emits TypeScript interfaces for `Todo`, `CreateTodo`, `CompleteTodo`,
the error enums, a `procedureSchemas` map of Valibot schemas, and a
`createApi` factory.

**What you should see.** `client/src/api.gen.ts` exists, starts with
`// DO NOT EDIT — generated by taut-rpc-cli.`, and `interface Todo`
matches your Rust struct field-for-field.

## Step 8 — Vite + React client setup

```sh
cd client
npm create vite@latest . -- --template react-ts
npm install
npm install taut-rpc valibot
```

Two notes:

1. The Vite scaffold rewrites `src/`; either move `api.gen.ts` back in
   afterward or run codegen *after* scaffolding.
2. `taut-rpc` ships the runtime (`createClient`, `TautError`,
   `isTautError`) that the generated file imports from.

Add a regen script to `client/package.json`:

```json
"scripts": {
  "dev": "vite",
  "build-api": "cd .. && cargo taut gen --from-binary server/target/debug/todo-server --out client/src/api.gen.ts"
}
```

**What you should see.** `npm run dev` serves the Vite splash on
`http://localhost:5173/`. `npm run build-api` regenerates `api.gen.ts`
whenever the Rust API changes.

## Step 9 — The typed client with `createApi`

Replace `client/src/App.tsx`:

```tsx
import { useEffect, useState } from "react";
import { createApi, procedureSchemas, type Todo } from "./api.gen";

const api = createApi({
  url: "http://127.0.0.1:8080",
  schemas: procedureSchemas,
  validate: { send: true, recv: true },
});

export default function App() {
  const [todos, setTodos] = useState<Todo[]>([]);
  useEffect(() => { api.list_todos().then(setTodos); }, []);
  return (
    <ul>
      {todos.map((t) => (
        <li key={t.id}>{t.completed ? "[x] " : "[ ] "}{t.title}</li>
      ))}
    </ul>
  );
}
```

Passing `schemas: procedureSchemas` flips on validation in *both*
directions: inputs are parsed before they leave the browser, outputs are
parsed before they reach `setTodos`. Server-side drift surfaces here.

**What you should see.** With both servers running, opening
`http://localhost:5173` renders an empty `<ul>` and the network tab
shows one `POST /rpc/list_todos` returning `{"ok":[]}`.

## Step 10 — Render the list, hook the submit form

```tsx
const [todos, setTodos] = useState<Todo[]>([]);
const [title, setTitle] = useState("");

const refresh = () => api.list_todos().then(setTodos);
useEffect(() => { refresh(); }, []);

async function onSubmit(e: React.FormEvent) {
  e.preventDefault();
  await api.create_todo({ title });
  setTitle("");
  refresh();
}

async function onComplete(id: number) {
  await api.complete_todo({ id });
  refresh();
}

return (
  <main>
    <h1>Todos</h1>
    <form onSubmit={onSubmit}>
      <input value={title} onChange={(e) => setTitle(e.target.value)} />
      <button type="submit">Add</button>
    </form>
    <ul>
      {todos.map((t) => (
        <li key={t.id}>
          <button onClick={() => onComplete(t.id)} disabled={t.completed}>
            {t.completed ? "Done" : "Complete"}
          </button>{" "}{t.title}
        </li>
      ))}
    </ul>
  </main>
);
```

**What you should see.** Type "buy milk", click *Add* — the input clears
and "buy milk" appears in the list. Click *Complete*, the button text
flips to "Done" and disables.

## Step 11 — The `todos_changed` subscription

Polling on every mutation is wasteful and racy. A subscription pushes
notifications instead:

```rust
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use futures::StreamExt;

static EVENTS: OnceLock<broadcast::Sender<()>> = OnceLock::new();
fn events() -> &'static broadcast::Sender<()> {
    EVENTS.get_or_init(|| broadcast::channel(64).0)
}

#[rpc(stream)]
async fn todos_changed() -> impl futures::Stream<Item = u64> + Send + 'static {
    let rx = events().subscribe();
    BroadcastStream::new(rx).filter_map(|r| async move { r.ok().map(|_| 0u64) })
}
```

In `create_todo` and `complete_todo`, fire an event after mutating:

```rust
let _ = events().send(());
```

Stream item is `u64` — a tick value where the *arrival* is the signal,
not the value. Subscriptions ride SSE by default; codegen surfaces this
as `api.todos_changed.subscribe()` returning `AsyncIterable<number>`.

**What you should see.** Re-run `cargo build && npm run build-api` so
`api.gen.ts` picks up the new procedure (now with a `Proc_todos_changed`
entry of kind `"subscription"`). Then
`curl --no-buffer 'http://127.0.0.1:8080/rpc/todos_changed?input='`
streams `event: data\ndata: 0\n\n` every time you `create_todo` from
another terminal.

## Step 12 — Auto-refresh on subscription events

```tsx
useEffect(() => {
  const ac = new AbortController();
  (async () => {
    for await (const _ of api.todos_changed.subscribe()) {
      if (ac.signal.aborted) return;
      refresh();
    }
  })();
  return () => ac.abort();
}, []);
```

Two `useEffect` hooks now: one runs `list_todos` on mount, the other
opens the SSE stream and re-fetches on every frame. The cleanup function
aborts the underlying `fetch` so the stream closes cleanly when the
component unmounts.

**What you should see.** Open the page in two browser tabs. Adding a
todo in tab A shows up in tab B within ~50ms. Tab B's network tab shows
a long-lived `GET /rpc/todos_changed` connection with content-type
`text/event-stream` and `data: 0` frames arriving live.

## Step 13 — Error handling on the form

Right now an empty title throws an unhandled rejection. Catch it and
render inline:

```tsx
import { isTautError } from "taut-rpc";

const [error, setError] = useState<string | null>(null);

async function onSubmit(e: React.FormEvent) {
  e.preventDefault();
  setError(null);
  try {
    await api.create_todo({ title });
    setTitle("");
  } catch (err) {
    if (isTautError(err, "validation_error")) {
      setError(err.payload.issues?.[0]?.message ?? "invalid input");
    } else if (isTautError(err, "capacity_exceeded")) {
      setError("storage full — delete some todos first");
    } else {
      throw err;
    }
  }
}
```

```tsx
{error && <p style={{ color: "red" }}>{error}</p>}
```

`isTautError(e, code)` is a type-narrowing predicate from the runtime:
inside the `if`, TypeScript knows `e.payload` matches the type declared
on that `code`. Narrowing by `code` is the contract — see the
[Errors](../concepts/errors.md) chapter.

**What you should see.** Submitting an empty title (or one over 200
chars) leaves the input untouched and shows a red error message — *no
network request is made*, because validation runs client-side before
send. The browser network tab confirms: no `POST /rpc/create_todo` for
the rejected attempts.

## Step 14 (stretch) — Bearer-token auth via `tower::Layer`

Auth isn't a `taut-rpc` feature; it's a plain `tower::Layer`. Here's a
minimal bearer gate that lets public procedures through and 401s the
rest:

```rust
use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response, Json};

async fn auth(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let public = matches!(path,
        "/rpc/_health" | "/rpc/_procedures" | "/rpc/_ir" | "/rpc/ping");
    if public { return next.run(request).await; }

    let token = request.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    if token != Some("secret-token") {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "err": { "code": "unauthenticated", "payload": null }
            })),
        ).into_response();
    }
    next.run(request).await
}
```

Apply it in `main`:

```rust
let app = router
    .into_axum()
    .layer(axum::middleware::from_fn(auth))
    .layer(CorsLayer::permissive());
```

The 401 body is the [SPEC §4.1](../reference/spec.md) envelope, **not**
a free-form message. Returning anything else here would let middleware
short-circuits drift from procedure errors.

On the client, send the token:

```tsx
const api = createApi({
  url: "http://127.0.0.1:8080",
  schemas: procedureSchemas,
  headers: { Authorization: "Bearer secret-token" },
});
```

**What you should see.** Without a token,
`curl -X POST localhost:8080/rpc/list_todos -H 'content-type: application/json' -d '{}'`
returns HTTP 401 and `{"err":{"code":"unauthenticated","payload":null}}`.
Adding `-H 'authorization: Bearer secret-token'` returns the list as
before. The React client keeps working because it now sends the header
on every request.

## Where to next

You've built the canonical end-to-end shape: typed handlers, generated
client, validation on both sides, a live subscription, typed errors, and
bearer-token auth as a tower layer. The pieces compose; nothing on this
page is special-cased for "todos."

- [Validation](../guides/validation.md) — every constraint kind beyond `length`.
- [Subscriptions](../guides/subscriptions.md) — `StreamFrame`, mid-stream
  errors, the WebSocket transport.
- [Authentication](../guides/auth.md) — per-procedure typed errors and
  state plumbing patterns.
- [Errors](../concepts/errors.md) — the wire envelope and `isTautError`
  narrowing in detail.

# todo-react — full-stack axum + Vite + React Todo over taut-rpc

Phase 5's flagship example: a real, batteries-included Todo app whose
**only** wire contract is the generated `api.gen.ts`. Everything the React
UI knows about the server's procedures — input/output shapes, validator
schemas, procedure kinds, subscription handles — is read from the file
codegen produces. There is no hand-written API client.

The point of this example is to show that taut-rpc's "the contract is the
build artifact" promise survives all the way into a normal frontend stack
(Vite dev server, HMR, React 18, browser SSE). Earlier examples ran
under Node/`tsx`; this one is the first that exercises codegen output
inside a bundler.

## What it covers

- `list_todos() -> Vec<Todo>` — unary query, returns the current list.
- `create_todo(CreateTodoInput) -> Todo` — mutation with
  `#[derive(Validate)]` on the input. The title is constrained to
  `length(1..200)`, so the empty-input case fails *before* the network
  call thanks to the generated Valibot schema.
- `complete_todo(CompleteTodoInput) -> Todo` — mutation that toggles a
  todo's `completed` flag.
- `todos_changed() -> impl Stream<Item = Vec<Todo>>` — subscription that
  re-broadcasts the full list on every mutation. The React UI subscribes
  once on mount and replaces its local state on every frame, so creates
  and toggles from another tab show up live.

In-memory state lives in a `tokio::sync::RwLock<HashMap<u64, Todo>>` and a
`tokio::sync::broadcast` channel feeds the subscription. CORS is permissive
because Vite proxies `/rpc` to the Rust server in dev — see below.

The example lives outside the cargo workspace (`exclude = ["examples"]` in
the root `Cargo.toml`) so it resolves its own dependencies through path
entries and never piggy-backs on workspace machinery.

## Run sequence

The pipeline is the standard taut-rpc dance (build server → `cargo taut
gen` → run client) with one extra one-time setup step for the npm runtime.

### 1. Build the npm runtime (once per checkout)

```sh
cd npm/taut-rpc
npm install
npm run build
```

### 2. Build the server in IR-dump mode and run codegen

From the repository root:

```sh
cd examples/todo-react/server && cargo build && cd ../../..
cargo run -p taut-rpc-cli -- taut gen \
  --from-binary examples/todo-react/server/target/debug/todo-react-server \
  --out         examples/todo-react/client/src/api.gen.ts
```

`--from-binary` re-spawns the server binary with `TAUT_DUMP_IR` set,
captures its IR, then runs the codegen step. The IR never escapes
`target/taut/ir.json` and no port is bound during the dump.

The generated `api.gen.ts` exports `procedureSchemas` alongside `createApi`;
the React app wires both into `createApi` so validation runs on every
mutation in both directions.

### 3. Start the server

```sh
# terminal A
cd examples/todo-react/server
cargo run
```

Binds `0.0.0.0:7710` (Phase 0 smoke uses 7700, Phase 1 uses 7701, Phase 2
uses 7702/7703, Phase 3 uses 7704, Phase 4 uses 7705, so this and all
prior examples can run side-by-side). Vite dev runs on 7711.

### 4. Start the Vite dev server

```sh
# terminal B
cd examples/todo-react/client
npm install
npm run dev
```

Vite serves `http://127.0.0.1:7711` and proxies `/rpc` to
`http://127.0.0.1:7710` (`vite.config.ts`). The React app uses a relative
`url: "/rpc"` so the same code runs unchanged behind any reverse proxy in
production.

Open the URL in two browser tabs and add/toggle todos in one — the other
updates live via the SSE subscription.

## How the React app uses `api.gen.ts`

`src/App.tsx` imports `createApi` and `procedureSchemas` from the
generated file directly:

```ts
import { createApi, procedureSchemas } from "./api.gen";

const api = createApi({
  url: "/rpc",
  schemas: procedureSchemas,
});
```

That single object exposes:

- `api.list_todos()` — returns `Promise<Todo[]>`, statically typed off the
  generated `Procedures` map.
- `api.create_todo({ title })` — Promise; rejects synchronously (before
  network) if `title` is empty, with a `TautError("validation_error")`.
- `api.complete_todo({ id, completed })` — Promise.
- `api.todos_changed.subscribe()` — `AsyncIterable<Todo[]>`, consumed in a
  `useEffect` via `for await`.

There is no hand-written wire code. The contract is the file.

## What this is not

- It is **not** a deployment template. The server has permissive CORS,
  no auth, and binds `0.0.0.0`. The React app has no error boundary, no
  optimistic UI, no service-worker caching. State lives in a `HashMap` and
  vanishes on restart.
- It is **not** a styling showcase. The CSS is the bare minimum to make the
  list legible; the point is the wire contract, not the design system.

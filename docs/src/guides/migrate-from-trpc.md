# Migrating from tRPC (Node)

> This guide is for teams who are happy with tRPC's developer
> experience but want to move the server to Rust. taut-rpc is
> deliberately shaped like tRPC on the TS side — the client surface
> stays familiar, the server surface changes from a TS-runtime builder
> to a Rust trait + macro.

## Why migrate?

Three things motivate the move:

1. **Rust performance.** A tRPC procedure in Node spends most of its
   wall time in the JS event loop and in Zod's runtime validation.
   The same procedure in taut-rpc validates once at the boundary and
   then executes against typed Rust structs — no per-call schema
   walk, no V8 GC pauses on the hot path.
2. **Refactor safety across the boundary.** tRPC carries types from
   the server to the client at the type level — fast, but it relies
   on `tsc` agreeing with itself across packages and on Zod schemas
   staying in sync with whatever you actually return. taut-rpc's IR
   is the source of truth: rename a Rust field, regenerate, and the
   TS compiler points at every callsite that needs to change.
3. **Single binary deploy.** A tRPC server is a Node process plus a
   `node_modules` tree plus whatever runtime you ship. A taut-rpc
   server is one statically linked binary. Smaller image, faster
   cold start, no `npm audit` in the deploy pipeline.

If none of those apply, stay on tRPC. taut-rpc isn't a strict upgrade
— it's a different point in the design space.

## Procedure mapping table

The procedure is the unit of migration. Almost every tRPC pattern has
a direct taut-rpc analogue:

| tRPC                                                          | taut-rpc                                                            |
| ------------------------------------------------------------- | ------------------------------------------------------------------- |
| `t.procedure.input(z.object({...})).query(({input}) => ...)`  | `#[rpc] async fn name(input: I) -> Result<O, E>`                    |
| `t.procedure.input(...).mutation(...)`                        | same — `#[rpc]` covers both; the HTTP method is decided by codegen  |
| `t.procedure.subscription(() => observable(...))`             | `#[rpc] async fn name(input: I) -> impl Stream<Item = O>`           |
| `t.router({ user: userRouter, ... })`                         | a Rust `trait` per service, mounted via `Router::merge(...)`        |
| `t.middleware(async ({ ctx, next }) => ...)`                  | `tower::Layer` applied with `Router::layer(...)`                    |
| `protectedProcedure = t.procedure.use(authMw)`                | `Router::route("...", handler).layer(AuthLayer::new())`             |
| `inferRouterInputs<AppRouter>` / `inferRouterOutputs`         | generated `AppClient` exposes input/output types directly           |
| `createTRPCProxyClient<AppRouter>(...)`                       | `import { createClient } from "./generated"`                        |

The shape of a "router" is the same idea: a tree of named procedures.
The mechanism is different — tRPC builds the tree at runtime from
chained builder calls; taut-rpc reads it from `#[rpc]` traits at
compile time and emits both the axum routes and the TS client.

## Input validation

This is where tRPC and taut-rpc diverge most.

In tRPC, validation is a runtime concern. You hand `t.procedure.input`
a Zod (or Yup, or Valibot) schema; on every call, that schema runs
against the parsed JSON, and the inferred type flows into your
handler. The schema is part of your application code.

In taut-rpc, validation is a *compile-time* concern that emits a
runtime check. You annotate fields:

```rust
#[derive(taut_rpc::Type)]
pub struct CreateUser {
    #[taut(min_length = 1, max_length = 64)]
    pub name: String,
    #[taut(email)]
    pub email: String,
}
```

The macro reads those `#[taut(...)]` attributes, lowers them into IR,
and emits two artifacts:

- a TS file that re-expresses the same constraints as a Zod (or
  Valibot, configurable in `taut.toml`) schema, used by the client;
- a Rust validator implementation on `CreateUser` that the generated
  axum handler calls before invoking your `#[rpc]` function.

Server-side validation runs from the trait — there is no runtime call
into Zod on the server, and there is no separate schema file to keep
in sync with the struct. The Rust struct is the schema.

The practical migration step: pull each Zod schema apart into a Rust
struct with `#[taut(...)]` attrs. The attribute set is intentionally
similar to Zod's vocabulary (`min`, `max`, `email`, `url`, `regex`,
`min_length`, `max_length`).

## Output types

tRPC infers the TS output type from whatever your handler returns —
which is itself often the result of a Zod `parse`. The chain is
"Zod schema → inferred TS type → return value → inferred client type".

taut-rpc generates TS types directly from Rust structs via the IR.
The chain is "Rust struct → IR → emitted TS interface". There is no
intermediate schema language for output types because outputs do not
need runtime validation on the server (they came from your code) and
the client trusts the wire format because the server signed off on
the IR at build time.

If you have tRPC procedures that return Zod-validated *output* (rare,
but supported via `.output(schema)`), drop the output schema during
migration — the Rust return type is enough.

## Subscriptions

tRPC subscriptions are an `observable(emit => { ... })` over a
WebSocket link, with RxJS-flavored ergonomics on the client.

taut-rpc subscriptions are an `async` function returning an
`impl Stream<Item = T>` on the server, exposed to TS as an
`AsyncIterable<T>`. The transport is configurable: SSE for one-way
streams (the default, simpler), WebSocket when you need the client to
push back into the same channel.

```rust
#[rpc]
async fn ticks(input: TicksInput) -> impl Stream<Item = Tick> {
    tokio_stream::wrappers::IntervalStream::new(
        tokio::time::interval(Duration::from_secs(1)),
    )
    .map(|_| Tick { at: now() })
}
```

```ts
for await (const tick of client.ticks.subscribe({})) {
    console.log(tick.at);
}
```

The `for await` loop replaces the `subscribe({ next, error,
complete })` callback shape. If you have RxJS pipelines on the
client, wrap the iterable with `from(asyncIterable)` from `rxjs` —
the rest of your pipeline keeps working.

## Middleware

tRPC middleware is a function that takes `{ ctx, next, path, type }`
and either calls `next({ ctx: ... })` or throws. It composes by
chaining `.use(...)` on a procedure builder.

taut-rpc inherits axum's middleware story, which is `tower::Layer`.
A layer wraps a service and gets to inspect the request, mutate
extensions, short-circuit, or call through. Apply it with
`Router::layer(...)`:

```rust
let app = Router::new()
    .merge(user_service.into_router())
    .layer(AuthLayer::new(jwt_decoder))
    .layer(TraceLayer::new_for_http());
```

The two-tower-layer pattern (`AuthLayer` + `TraceLayer`) covers the
common tRPC middleware cases: auth context, logging, rate limits,
request timing. For per-procedure middleware (the tRPC
`protectedProcedure` pattern), apply the layer to a sub-router rather
than the top-level one.

## Error handling

In tRPC, you throw `TRPCError({ code: "UNAUTHORIZED", message: ... })`
and the framework serializes it for the client.

In taut-rpc, your handler returns `Result<T, E>` where `E: TautError`.
`TautError` is a small trait — it asks for a code (mapped to an HTTP
status) and a serializable payload. The default derive on an enum
covers the common case:

```rust
#[derive(taut_rpc::Error)]
pub enum UserError {
    #[taut(code = "NOT_FOUND", status = 404)]
    NotFound,
    #[taut(code = "FORBIDDEN", status = 403)]
    Forbidden { reason: String },
}
```

The generated TS client gets a tagged union as the error type, so the
catch site can `switch` on `err.code` and TypeScript narrows the
payload.

## Two-process vs one-process

A tRPC stack is usually two Node processes (or one, if your frontend
is server-rendered from the same Node app): a tRPC *server* and a
tRPC *client*. The client side is a thin wrapper that knows how to
serialize calls and deserialize responses.

A taut-rpc stack is one Rust process on the server and any TS runtime
on the client (browser, Node, Bun, Deno, edge worker). The Rust
server replaces the Node tRPC server; the TS client side is
*symmetric* — same call shape, same `await client.user.create({...})`
ergonomics, same generated input/output types. From a frontend
developer's perspective, the migration is mostly invisible: imports
change from `@/server/trpc` to `@/generated/taut-client`, and the
underlying transport changes from tRPC's batching JSON-RPC to
taut-rpc's per-procedure HTTP, but the call sites do not.

## Migration plan

Migrating a live tRPC API in one shot is rarely worth it. The
recommended path:

1. **Stand up taut-rpc beside tRPC.** Mount the Rust server at
   `/rpc/...` and leave the Node tRPC server at `/trpc/...`. The
   browser can call both.
2. **Move read-only procedures (queries) first.** Queries are the
   easiest to migrate: they're idempotent, they have no side effects
   to coordinate, and a bug only causes a stale read, not a corrupted
   write. Pick one feature surface (say, the user profile read path)
   and port its queries.
3. **Keep mutations and subscriptions on tRPC initially.** Mutations
   often touch shared state and may have invariants that are easier
   to hold in one place during the transition. Subscriptions usually
   live behind a WebSocket gateway that's worth migrating
   independently.
4. **Migrate one feature at a time.** A "feature" here means a
   coherent slice — user, billing, notifications — not a single
   procedure. Move all the procedures for one feature, including the
   mutations, before moving on. This keeps invariants local.
5. **Decommission tRPC last.** Once every feature is on taut-rpc,
   delete the Node server, the tRPC client wrapper, and the Zod
   schemas that backed them. The TS call sites stay as they are.

A typical large migration runs 4–8 weeks per service of moderate
size, with the bulk of the time spent on (4), feature-by-feature
porting, not on the framework swap itself.

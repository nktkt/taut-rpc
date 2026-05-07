# Getting started

This guide walks you from an empty directory to a typed, validated,
streaming RPC API in about thirty minutes. By the end you will have a
Rust server with one query, one mutation, and one subscription, plus a
TypeScript client that calls all three with full end-to-end types.

If you only want to skim, the shape is: install the crates, write a
`#[rpc]` function, run `cargo taut gen`, import the generated client.
Everything else in this guide is filling in that loop with real code.

## Prerequisites

You will need:

- **Rust 1.75 or newer.** `rustup update stable` if you are unsure.
- **Node 20 or newer.** Older Node lacks the `ReadableStream` async
  iterator we rely on for subscriptions.
- **Cargo** (ships with Rust) and **npm** (ships with Node).

No database, no Docker, no extra services. The server uses an in-memory
state and binds to `127.0.0.1:3000`.

## Install

Create a fresh workspace:

```sh
cargo new --bin myapp
cd myapp
```

Add the runtime crates to the server:

```sh
cargo add taut-rpc taut-rpc-macros tokio --features tokio/full
cargo add serde --features derive
```

Install the codegen CLI once, globally:

```sh
cargo install taut-rpc-cli
```

That gives you the `cargo taut` subcommand. Verify with `cargo taut
--version`.

Then scaffold a client directory and install the TypeScript runtime:

```sh
mkdir -p client/src
cd client
npm init -y
npm i taut-rpc
npm i -D typescript
cd ..
```

## Your first procedure

Open `src/main.rs` and replace its contents with a ten-line server:

```rust
use taut_rpc::{Router, serve};
use taut_rpc_macros::rpc;

#[rpc]
async fn ping() -> String {
    "pong".to_string()
}

#[tokio::main]
async fn main() {
    let router = Router::new().procedure(ping);
    serve(router, "127.0.0.1:3000").await.unwrap();
}
```

Run it:

```sh
cargo run
```

You should see `taut-rpc listening on 127.0.0.1:3000`. Leave it
running; the next steps assume it is up.

## Generate the TypeScript client

In a second terminal, point `cargo taut gen` at the binary you just
built. It introspects the embedded IR and emits a single `.ts` file:

```sh
cargo taut gen \
  --from-binary target/debug/myapp \
  --out client/src/api.gen.ts
```

The generated file exports a `createApi` factory plus types for every
input and output in your router. Re-run this command any time you
change a `#[rpc]` signature; in larger projects, wire it into a
`cargo watch` or pre-commit hook.

## Use it from TypeScript

Create `client/src/main.ts`:

```ts
import { createApi } from "taut-rpc";
import type { Api } from "./api.gen";

const client = createApi<Api>({ url: "http://127.0.0.1:3000" });

const reply = await client.ping.query();
console.log(reply); // "pong"
```

Run it with `npx tsx src/main.ts` from the `client/` directory. The
return type of `client.ping.query()` is inferred as `string` — no
manual type annotations, no runtime casts.

## Add a typed input

A `ping` with no arguments is not very interesting. Add an `add`
procedure that takes two integers. In `src/main.rs`:

```rust
use taut_rpc::Type;
use serde::Deserialize;

#[derive(Type, Deserialize)]
struct AddInput {
    a: i32,
    b: i32,
}

#[rpc]
async fn add(input: AddInput) -> i32 {
    input.a + input.b
}
```

Register it on the router:

```rust
let router = Router::new()
    .procedure(ping)
    .procedure(add);
```

Restart the server, regenerate the client (`cargo taut gen ...`), and
the TS side gets a typed `client.add.query({ a, b })`:

```ts
const sum = await client.add.query({ a: 2, b: 3 });
console.log(sum); // 5
```

If you pass `{ a: "two", b: 3 }` the TypeScript compiler rejects it
before the request is even built.

## Add validation

Compile-time types stop typos, but they do not stop a caller sending
`a: 999_999_999`. Add `taut-rpc`'s `Validate` derive:

```rust
use taut_rpc::Validate;

#[derive(Type, Deserialize, Validate)]
struct AddInput {
    #[taut(min = 0, max = 1000)]
    a: i32,
    #[taut(min = 0, max = 1000)]
    b: i32,
}
```

The macro generates a `validate()` impl that the runtime calls before
your handler ever sees the input. Out-of-range values come back to the
client as a structured `ValidationError` with the offending field
names — the TS client surfaces them as a typed exception, not a 400.

## Add an error type

Even with validation, `2 + 1000` overflows your business rules. Model
that as a domain error:

```rust
use taut_rpc::TautError;

#[derive(TautError, Debug)]
enum AddError {
    Overflow,
}

#[rpc]
async fn add(input: AddInput) -> Result<i32, AddError> {
    input.a.checked_add(input.b).ok_or(AddError::Overflow)
}
```

`#[derive(TautError)]` flows the variant names into the IR, so the TS
side gets a discriminated union:

```ts
try {
  const sum = await client.add.query({ a: 999, b: 999 });
} catch (e) {
  if (e.kind === "Overflow") {
    // e is narrowed to AddError["Overflow"]
  }
}
```

No string-matching on error messages, no `instanceof` against a generic
`RpcError`.

## Add a subscription

Streams use `#[rpc(stream)]` and return any `Stream<Item = T>`. Here is
a one-second tick:

```rust
use futures::stream::{self, Stream};
use std::time::Duration;
use tokio::time::interval;
use tokio_stream::wrappers::IntervalStream;
use tokio_stream::StreamExt;

#[rpc(stream)]
async fn ticks() -> impl Stream<Item = u64> {
    IntervalStream::new(interval(Duration::from_secs(1)))
        .enumerate()
        .map(|(i, _)| i as u64)
}
```

Register it with `.procedure(ticks)`. The transport upgrades the
request to SSE automatically — no per-procedure transport choice in
your code.

## Test the subscription from TypeScript

`client.ticks` exposes a `.subscribe()` that returns an async iterable:

```ts
for await (const tick of client.ticks.subscribe()) {
  console.log("tick", tick);
  if (tick >= 3) break;
}
```

Breaking out of the loop closes the underlying SSE connection. The
inferred type of `tick` is `number` — same IR-driven typing as the
query and mutation paths.

## Where to go next

You now have all four shapes — typed input, validation, errors,
streams — wired end-to-end. The deeper guides cover the production
concerns:

- **[Authentication](./auth.md)** — extracting bearer tokens via
  `tower::Layer`, request context, and per-procedure guards.
- **[Middleware](./middleware.md)** — composing logging, rate limits,
  and tracing across the whole router.
- **[Deployment](./deployment.md)** — single-binary deploys, behind
  reverse proxies, and the SSE keepalive story.

If you would rather learn by building something larger end-to-end, two
worked tutorials follow this guide directly:

- **[Tutorial: Todo app](../tutorials/todo.md)** — query, mutation,
  optimistic updates, and a SQLite-backed store.
- **[Tutorial: Chat app](../tutorials/chat.md)** — fan-out
  subscriptions, presence, and reconnect semantics.

## See also

- [Concepts: the IR](../concepts/ir.md) — what `cargo taut gen` reads.
- [Concepts: wire format](../concepts/wire-format.md) — the envelope
  every request and response shares.
- [Reference: `#[rpc]` attribute](../reference/rpc-attribute.md)

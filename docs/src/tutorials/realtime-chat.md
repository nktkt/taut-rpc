# Tutorial: Real-time chat with subscriptions

In this tutorial we build a small chat application end-to-end. By the end, you
will have:

- A typed `Message` model.
- A `send_message` mutation that validates input.
- A `messages_since` query for backfill on connect.
- A `messages_live` subscription that streams new messages over SSE.
- A TypeScript client that backfills, subscribes, renders optimistically, and
  reconnects on failure.

The pieces are small individually, but together they cover the realistic shape
of a live feature: write, read-back, push, recover.

## 1. The `Message` type

Start with the data. A message has an id, an author, a body, and a timestamp
in milliseconds since the epoch:

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Message {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub ts_ms: u64,
}
```

`Clone` matters here — broadcast channels hand each subscriber its own copy, so
the value must be cloneable. `u64` for the id keeps things simple; in a real
deployment you would use a snowflake-style id or a UUID.

## 2. The `send_message` mutation

Sending a message is a write, so it is a mutation. The input carries the
author and body, and the body is bounded by `length(1..1000)` to reject empty
strings and absurdly large payloads at the boundary:

```rust
use validator::Validate;

#[derive(Debug, Deserialize, Validate, schemars::JsonSchema)]
pub struct SendInput {
    #[validate(length(min = 1, max = 64))]
    pub author: String,
    #[validate(length(min = 1, max = 1000))]
    pub body: String,
}

#[mutation]
pub async fn send_message(
    ctx: &Ctx,
    input: SendInput,
) -> Result<Message, ApiError> {
    input.validate()?;

    let msg = Message {
        id: ctx.state.next_id(),
        author: input.author,
        body: input.body,
        ts_ms: now_ms(),
    };

    ctx.state.push(msg.clone()).await;
    let _ = ctx.state.tx.send(msg.clone());
    Ok(msg)
}
```

Two things happen on success: the message is pushed onto the shared log, and
it is broadcast to live subscribers. The `let _ =` swallows the
`SendError` you get when there are zero subscribers — that is not a failure,
just a quiet moment.

## 3. The `messages_since` query

Live streams alone are not enough. A client that reconnects after a hiccup
needs to ask "what did I miss?" That is `messages_since`:

```rust
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SinceInput {
    /// Return messages with id strictly greater than this.
    pub after_id: u64,
    /// Cap the response.
    pub limit: u32,
}

#[query]
pub async fn messages_since(
    ctx: &Ctx,
    input: SinceInput,
) -> Result<Vec<Message>, ApiError> {
    let limit = input.limit.min(500) as usize;
    let log = ctx.state.log.read().await;
    Ok(log
        .iter()
        .filter(|m| m.id > input.after_id)
        .take(limit)
        .cloned()
        .collect())
}
```

We cap `limit` server-side; never trust a client to behave. `after_id = 0`
returns the head of the log, which is what a fresh client uses on first
connect.

## 4. The `messages_live` subscription

Subscriptions return a `Stream`. We obtain a `broadcast::Receiver` and turn it
into a stream that filters out lag errors:

```rust
use futures::Stream;
use tokio_stream::{wrappers::BroadcastStream, StreamExt};

#[subscription]
pub async fn messages_live(
    ctx: &Ctx,
) -> Result<impl Stream<Item = Message>, ApiError> {
    let rx = ctx.state.tx.subscribe();
    Ok(BroadcastStream::new(rx).filter_map(|r| r.ok()))
}
```

`BroadcastStream` yields `Result<T, BroadcastStreamRecvError>`; the `Err` case
means the subscriber lagged and dropped messages. Filtering with `r.ok()`
silently skips them. If you would rather close the stream on lag, match on
the error and return `None` for the whole stream.

## 5. Wiring shared state

The two server-side pieces — the log and the broadcast channel — live in a
single struct:

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{broadcast, RwLock};

pub struct ChatState {
    pub log: RwLock<Vec<Message>>,
    pub tx: broadcast::Sender<Message>,
    next: AtomicU64,
}

impl ChatState {
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self {
            log: RwLock::new(Vec::new()),
            tx,
            next: AtomicU64::new(1),
        }
    }

    pub fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    pub async fn push(&self, msg: Message) {
        self.log.write().await.push(msg);
    }
}
```

Pick a `capacity` (say, 256) that comfortably exceeds the burstiest second of
traffic you expect. Subscribers that fall further behind than `capacity` will
see lag errors — exactly the case the `filter_map` above swallows.

For a real app, swap `Vec<Message>` for a ring buffer or a database. The
shape of the API does not change.

## 6. Try it with curl

With the server running on `:8080`, send one message and listen on another
terminal:

```bash
# Terminal A: subscribe.
curl -N http://localhost:8080/sub/messages_live
```

```bash
# Terminal B: send.
curl -X POST http://localhost:8080/mut/send_message \
  -H 'content-type: application/json' \
  -d '{"author":"alice","body":"hello"}'
```

Terminal A should print an SSE frame:

```
event: message
data: {"id":1,"author":"alice","body":"hello","ts_ms":1715000000000}
```

If you do not see the frame, check that the server is actually listening on
`:8080` and that nothing (a proxy, your browser dev tools) is buffering the
response. The `-N` flag disables curl's own buffering.

## 7. A TypeScript client

The client does two things on startup: backfill, then subscribe. The
generated SDK gives you typed `query`, `mutation`, and `subscription` helpers.

```ts
import { client, type Message } from "./generated/chat-client";

let lastId = 0;
const view: Message[] = [];

async function backfill() {
  const batch = await client.query.messages_since({
    after_id: lastId,
    limit: 200,
  });
  for (const m of batch) {
    view.push(m);
    lastId = Math.max(lastId, m.id);
  }
  render(view);
}

async function subscribe() {
  for await (const m of client.subscription.messages_live()) {
    if (m.id <= lastId) continue; // dedupe vs. backfill
    view.push(m);
    lastId = m.id;
    render(view);
  }
}

await backfill();
await subscribe();
```

The dedupe check matters. Between the backfill returning and the
subscription handshake completing, a message can arrive that the live
stream then re-delivers. Always reconcile by id.

## 8. Optimistic UI

Waiting for a network round-trip before showing your own message feels bad.
Render it immediately, mark it provisional, and reconcile when the server
echoes it back:

```ts
type Pending = Message & { pending: true; tempId: string };

async function send(author: string, body: string) {
  const tempId = crypto.randomUUID();
  const optimistic: Pending = {
    id: 0,
    author,
    body,
    ts_ms: Date.now(),
    pending: true,
    tempId,
  };
  view.push(optimistic);
  render(view);

  try {
    const real = await client.mutation.send_message({ author, body });
    // Replace the optimistic entry. The subscription will deliver the
    // same message; the dedupe check in subscribe() will drop it.
    const idx = view.findIndex(
      (m) => "tempId" in m && (m as Pending).tempId === tempId,
    );
    if (idx >= 0) view[idx] = real;
    lastId = Math.max(lastId, real.id);
    render(view);
  } catch (err) {
    // Mark the optimistic entry as failed, or drop it.
    const idx = view.findIndex(
      (m) => "tempId" in m && (m as Pending).tempId === tempId,
    );
    if (idx >= 0) view.splice(idx, 1);
    render(view);
    throw err;
  }
}
```

Two subtleties:

- The optimistic message has `id: 0`, which is below any real id, so the
  dedupe in `subscribe()` does the right thing if the subscription event
  beats the mutation response.
- We replace by `tempId`, not by content. Two users can send the same
  body within a second.

## 9. Reconnection

Networks fail. Wrap the for-await in a retry loop with exponential backoff
and re-backfill before resubscribing so you do not lose messages sent during
the gap:

```ts
async function runForever() {
  let delay = 500;
  while (true) {
    try {
      await backfill();
      delay = 500; // reset on a clean connect
      for await (const m of client.subscription.messages_live()) {
        if (m.id <= lastId) continue;
        view.push(m);
        lastId = m.id;
        render(view);
      }
      // Stream ended cleanly — server restart, perhaps. Reconnect.
    } catch (err) {
      console.warn("subscription failed, retrying", err);
    }
    await sleep(delay + Math.random() * 250);
    delay = Math.min(delay * 2, 30_000);
  }
}
```

The jitter (`Math.random() * 250`) is not decorative: when a server restarts
it gets reconnected to by every client at once. Without jitter they all hit
it on the same millisecond.

## 10. Multiple clients

The broadcast channel handles fan-out for free. Each `subscribe()` call gets
its own `Receiver`, and `tx.send(msg)` delivers to all of them. There is
nothing extra to write on the server.

What you do need to think about:

- **Channel capacity vs. slowest subscriber.** The slowest subscriber sets
  the floor for how much memory the channel holds. A capacity of 256 is
  fine for chat; for high-throughput streams you may need to disconnect
  laggers.
- **Author identity.** This tutorial trusts the client to send `author`.
  In production you would derive the author from an authenticated session
  on the server and ignore the field on input.
- **Persistence.** `Vec<Message>` is lost on restart. Swap it for a
  database (or at minimum, an append-only file) before anyone relies on
  this for real conversations.

## What you built

A complete real-time feature in well under a hundred lines of server code:
a validated mutation, a backfill query, an SSE subscription, and a client
that survives reconnects and feels instant on send. The same shape — write,
read-back, push, recover — works for notifications, presence, live
dashboards, collaborative editing, and most other "things change and the
client wants to know" features. Sub the model, keep the structure.

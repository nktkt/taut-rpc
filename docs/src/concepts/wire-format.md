# Wire format

The wire format is the JSON contract between a `taut-rpc` server and any
client — generated TypeScript or otherwise. Phase 5 finalizes the v0.1
shape: HTTP for queries and mutations, SSE for subscriptions, and an
opt-in WebSocket transport for multiplexed streams. This page is the
narrative companion to [SPEC §4](../reference/spec.md), which is the
canonical source.

## Shape, in one sentence

Every procedure call carries an `{"input": …}` request body and replies
with an `{"ok": …}` or `{"err": …}` envelope. Subscriptions speak the
same envelope, framed onto SSE events or WebSocket messages.

## Query and mutation: HTTP

The default transport for queries and mutations is `POST` with a JSON
body. A procedure `get_user(id: u64) -> Result<User, GetUserError>`
produces:

**Request**

```http
POST /rpc/get_user HTTP/1.1
Content-Type: application/json

{ "input": { "id": 1 } }
```

**Successful response**

```http
HTTP/1.1 200 OK
Content-Type: application/json

{ "ok": { "id": 1, "name": "Ada Lovelace" } }
```

**Error response**

```http
HTTP/1.1 404 Not Found
Content-Type: application/json

{
  "err": {
    "code": "user_not_found",
    "payload": { "id": 1 }
  }
}
```

A few rules govern the envelope:

- The body is always exactly one of `{"ok": …}` or `{"err": …}`. Clients
  branch on key presence, not on HTTP status alone. The status is
  advisory and follows convention: `4xx` for caller-induced errors and
  `5xx` for server-induced errors.
- Success status is always `200`. Error status comes from
  `TautError::http_status()` on the server side.
- The `Content-Type` is `application/json` in both directions. There is
  no chunking, no streaming — a single request maps to a single response.
- Each error's `payload` is *typed per-procedure*. A procedure that can
  only emit `not_found` and `forbidden` produces a TS error union with
  exactly those discriminants; the client does not need to handle codes
  the procedure cannot emit.

### Method override

By default every procedure mounts as `POST`. Annotating a procedure with
`#[rpc(method = "GET")]` opts it into a `GET` route for cache-friendly
reads:

```http
GET /rpc/list_articles?input=%7B%22limit%22%3A20%7D HTTP/1.1
```

The `input` query parameter is the URL-encoded JSON of the input value.
The response envelope is identical to the `POST` form.

The `#[rpc(method = "GET")]` attribute is a Phase 1 stub on the macro;
the runtime wiring that mounts the route on the axum router lands in
Phase 4. Defaulting to `POST` avoids URL-length surprises and aligns with
the JSON-body envelope for inputs that are not trivially flat.

## Subscription: SSE

The default subscription transport is **Server-Sent Events**. A client
opens a `GET` request with the input encoded into the query string:

```http
GET /rpc/ticks?input=%7B%22count%22%3A3%7D HTTP/1.1
Accept: text/event-stream
```

The server responds with `Content-Type: text/event-stream` and emits a
sequence of frames terminated by exactly one `end` (clean completion) or
one `error` (terminal failure):

```
event: data
data: {"tick": 0}

event: data
data: {"tick": 1}

event: data
data: {"tick": 2}

event: end
data: 

```

Three named events are defined:

- **`data`** — one item from the stream. The `data:` line carries the
  JSON of a single `Output` value (not wrapped in `{"ok": …}`; the SSE
  event name plays that role).
- **`error`** — a terminal error. The payload is the same envelope shape
  as HTTP errors: `{"code": "...", "payload": …}`. After an `error`
  frame the server closes the stream.
- **`end`** — clean completion. The canonical form is `event: end\ndata:
  \n\n` (a `data:` line with a single space). The TypeScript parser
  tolerates either `data: \n` or no `data` line at all on the end frame.

Cancellation on SSE is implicit: the client closes the underlying TCP
connection and the server drops the spawned stream task. There is no
explicit cancellation message.

## Subscription: WebSocket

Behind the `ws` cargo feature on the `taut-rpc` crate, the runtime
mounts an additional endpoint:

```http
GET /rpc/_ws HTTP/1.1
Upgrade: websocket
```

A client opens this connection once and then multiplexes any number of
subscriptions over it. Frames are JSON-encoded `WsMessage` values, each
carrying an `id` chosen by the client to demultiplex responses:

```
// client → server
{ "type": "subscribe",   "id": "<id>", "procedure": "ticks",
  "input": { "count": 5, "interval_ms": 1000 } }
{ "type": "unsubscribe", "id": "<id>" }

// server → client
{ "type": "data",  "id": "<id>", "value": <json> }
{ "type": "error", "id": "<id>", "code": "...", "payload": <json> }
{ "type": "end",   "id": "<id>" }
```

A `subscribe` is answered by zero or more `data` frames terminated by
exactly one `end` (clean) or one `error` (terminal). An `unsubscribe`
asks the server to drop the stream early; the server may emit any
already-buffered `data` frames before the corresponding `end`.

The `id` is opaque to the server. Clients typically use a counter or a
UUID. Reusing an `id` while a subscription is active is undefined; reuse
after `end`/`error` is fine.

Phase 3 shipped the server side of this transport; the TypeScript client
runtime for WebSocket is on the Phase 4+ roadmap. Until then, the
generated client speaks SSE and code that wants WebSocket framing speaks
to `WsMessage` directly.

## Built-in error codes

The runtime emits a small set of codes regardless of which procedure was
called: `decode_error`, `validation_error`, `not_found` (for unknown
procedures), `internal`, and the `StandardError` variants
(`unauthenticated`, `forbidden`, `rate_limited`, …). The full list with
HTTP status, payload shape, and source lives in
[Error codes](../reference/error-codes.md). User-defined codes from
`#[derive(TautError)]` ride the same envelope.

## Versioning

Subscription frames may carry a leading `event: v` frame whose `data:`
line is a small integer version marker. This reserves room for the
subscription protocol to evolve — adding new event names, changing
end-frame shape, etc. — without an SSE/WS client reading old data as new
or vice versa. v0.1 servers do not emit `event: v`, and clients that
receive no version frame interpret the stream as v0 implicitly.

Queries and mutations have no version frame: the envelope shape is part
of v0 by definition, and any breaking change there would be a wire-level
protocol bump (a new path prefix, e.g. `/rpc/v1/<name>`), not an in-band
marker. See [SPEC §9](../reference/spec.md) for the compatibility plan.

## Worked examples

### Query with input and output

A procedure `add(a: i32, b: i32) -> i32`:

```http
POST /rpc/add HTTP/1.1
Content-Type: application/json

{ "input": { "a": 2, "b": 3 } }
```

```http
HTTP/1.1 200 OK
Content-Type: application/json

{ "ok": 5 }
```

### Mutation with a typed error

A procedure `charge(cents: u64) -> Result<Receipt, BillingError>` whose
error enum has `#[taut(code = "card_declined", status = 402)]`:

```http
POST /rpc/charge HTTP/1.1
Content-Type: application/json

{ "input": { "cents": 1500 } }
```

```http
HTTP/1.1 402 Payment Required
Content-Type: application/json

{
  "err": {
    "code": "card_declined",
    "payload": { "reason": "insufficient_funds" }
  }
}
```

### Subscription with normal completion

A `ticks` subscription that emits three values and ends:

```http
GET /rpc/ticks?input=%7B%22count%22%3A3%7D HTTP/1.1
Accept: text/event-stream
```

```
event: data
data: {"tick": 0}

event: data
data: {"tick": 1}

event: data
data: {"tick": 2}

event: end
data: 

```

### Subscription cancelled by the client (WebSocket)

The client subscribes, reads two values, then unsubscribes. The server
flushes `end` for the cancelled `id`:

```
→ { "type": "subscribe",   "id": "s1", "procedure": "ticks",
    "input": { "count": 100 } }
← { "type": "data",  "id": "s1", "value": { "tick": 0 } }
← { "type": "data",  "id": "s1", "value": { "tick": 1 } }
→ { "type": "unsubscribe", "id": "s1" }
← { "type": "end",   "id": "s1" }
```

On SSE the client would simply close the connection; there is no
matching frame from the server.

### Decode error envelope

Posting non-JSON, or JSON that does not match the `{"input": …}`
envelope, surfaces as the built-in `decode_error` code:

```http
POST /rpc/add HTTP/1.1
Content-Type: application/json

{ "a": 2, "b": 3 }
```

```http
HTTP/1.1 400 Bad Request
Content-Type: application/json

{
  "err": {
    "code": "decode_error",
    "payload": { "message": "missing field `input`" }
  }
}
```

Type-level mismatches between the JSON inside `input` and the
procedure's `Input` type surface as `validation_error` once `Validate`
runs — see [Validation](./validation.md) for the boundary between the
two.

## See also

- [SPEC §4 — Wire format](../reference/spec.md)
- [Error codes](../reference/error-codes.md)
- [Errors](./errors.md)
- [Subscriptions guide](../guides/subscriptions.md)

# Wire format

> Placeholder chapter. See [SPEC §4](../reference/spec.md) for the
> canonical wire format.

## Shape, in one sentence

JSON over HTTP for queries and mutations, SSE for subscriptions,
WebSocket opt-in. POST is the default for all procedures; explicit
`#[rpc(method = "GET")]` opts a procedure into a `GET` route for
cache-friendly reads.

## Example: a query round-trip

A procedure `get_user(id: u64) -> Result<User, ApiError>` produces:

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

## What's notable

- The envelope is always `{ "ok": ... }` or `{ "err": ... }`; clients
  branch on key presence, not on HTTP status alone. The status code is
  advisory and follows convention (`4xx` for caller-induced errors,
  `5xx` for server-induced errors).
- The error payload is *typed per-procedure*. A procedure that can only
  emit `not_found` and `forbidden` produces a TS error union limited to
  exactly those discriminants — the client doesn't have to handle errors
  the procedure can't emit.
- Subscriptions ride SSE by default (`text/event-stream`) with `data`,
  `error`, and `end` events. The WebSocket framing is the same payload
  shape wrapped in `{ type, payload }` messages, behind a feature flag.

## See also

- [SPEC §4 — Wire format](../reference/spec.md)
- [Errors](./errors.md)
- [Subscriptions guide](../guides/subscriptions.md)

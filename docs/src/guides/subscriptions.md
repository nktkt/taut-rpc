# Subscriptions

> Placeholder guide. **Subscriptions are a Phase 3 deliverable** on the
> [roadmap](../reference/roadmap.md). Nothing in this chapter exists
> yet; the page describes the planned shape so readers know what to
> expect.

## The intended ergonomics

A subscription is a `#[rpc]` function returning `impl Stream<Item = T>`,
flagged with the `stream` argument:

```rust
#[rpc(stream)]
async fn user_events(user_id: u64) -> impl Stream<Item = UserEvent> {
    // produce a Stream of UserEvent values
}
```

On the TypeScript side, the generated client exposes the procedure as an
`AsyncIterable`:

```ts
for await (const evt of client.userEvents.subscribe({ userId: 1 })) {
    // evt is fully typed as UserEvent
}
```

The intent is that nothing in the call site looks special: a stream
procedure is a `for await` loop, period. Cancellation is a matter of
breaking out of the loop or aborting the underlying request.

## Transports

Subscriptions ride **Server-Sent Events** by default. The wire shape:

```
GET /rpc/user_events?input=%7B%22user_id%22%3A1%7D
Accept: text/event-stream

event: data\ndata: { ... }\n\n
event: error\ndata: { ... }\n\n
event: end\ndata:\n\n
```

**WebSocket** is the same payload model wrapped in `{ type, payload }`
messages, available behind a feature flag for environments where SSE is
inadequate (bidirectional messaging, certain proxy setups). The default
is SSE because it composes with HTTP middleware unchanged.

## Why Phase 3 and not earlier

Subscriptions amplify any rough edge in the type system, the IR, or the
error model. Pinning down the query/mutation path first means
subscriptions inherit a tested error envelope, a tested codegen pass,
and a tested IR shape. Trying to ship streams alongside the basics
would have doubled the design surface for Phase 1.

## What this chapter will cover when written

- Worked example: a counter that ticks once per second, observed from
  TS.
- Backpressure and cancellation: what happens when the consumer is
  slow, when the consumer disconnects, when the server wants to end the
  stream.
- The error event: how typed errors compose with stream procedures.
- WebSocket opt-in: when to flip the feature, what changes on the wire.

## See also

- [Roadmap — Phase 3](../reference/roadmap.md)
- [SPEC §4.2 — Subscription wire format](../reference/spec.md)
- [Wire format](../concepts/wire-format.md)

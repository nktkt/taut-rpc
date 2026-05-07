//! WebSocket transport for taut-rpc subscriptions (SPEC §4.2).
//!
//! Feature-gated behind `cfg(feature = "ws")` and mounted at
//! `GET /rpc/_ws` by [`crate::router::Router::into_axum`] when the feature is
//! enabled. Wire-level framing reuses [`crate::wire::WsMessage`]: a single
//! socket multiplexes many logical streams keyed by `id`, with the client
//! sending [`crate::wire::WsMessage::Subscribe`] / [`crate::wire::WsMessage::Unsubscribe`]
//! and the server sending [`crate::wire::WsMessage::Data`] /
//! [`crate::wire::WsMessage::Error`] / [`crate::wire::WsMessage::End`] for
//! each `id`.
//!
//! In v0.1 only **subscription** procedures are reachable via WS. Queries and
//! mutations remain on `POST /rpc/<name>` per SPEC §4.1 — making them
//! addressable over WS as well is plausible (a single bidirectional channel
//! per browser tab) but explicitly out of scope here: SPEC §4.2 frames
//! WebSocket as the "alternative subscription transport", so a non-subscription
//! procedure name in a Subscribe frame surfaces a `not_subscription` error and
//! an immediate `End` for that id.
//!
//! # Lifecycle
//!
//! 1. Client opens a WebSocket to `/rpc/_ws`.
//! 2. For each `Subscribe { id, procedure, input }` the server spawns a task
//!    that drives the procedure's [`crate::procedure::StreamHandler`] and
//!    forwards each yielded [`crate::procedure::StreamFrame`] as a
//!    `Data { id, value }` (or `Error { id, err }`) envelope, terminating
//!    with `End { id }`.
//! 3. `Unsubscribe { id }` aborts that task via the matching
//!    [`futures::stream::AbortHandle`].
//! 4. On socket disconnect every active task is aborted; in-flight frames in
//!    the writer mpsc are dropped along with the writer.
//!
//! # Notes for implementers
//!
//! axum 0.7's [`axum::extract::ws::Message::Text`] carries a [`String`]
//! (axum 0.8 switched to `Utf8Bytes`); the dispatch loop in
//! [`ws_route::handle_socket`] codes against the 0.7 shape on purpose. If/when
//! this crate moves to axum 0.8 the `Text(t)` arm will need to switch to
//! `t.to_string()` or equivalent.
//!
//! Ordering: each frame is forwarded in stream order on a single mpsc, so
//! per-`id` frames retain their procedure-emitted order on the wire. Frames
//! across different `id`s are interleaved arbitrarily — that's the whole
//! point of multiplex.

#![cfg(feature = "ws")]

pub(crate) mod ws_route {
    use std::collections::HashMap;
    use std::sync::Arc;

    use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
    use axum::response::Response;
    use futures::stream::{AbortHandle, Abortable};
    use futures::{SinkExt, StreamExt};

    use crate::procedure::{ProcedureBody, ProcedureDescriptor, StreamFrame};
    use crate::router::ProcKindRuntime;

    /// Build the axum handler closure that upgrades an incoming HTTP request to
    /// a multiplexed taut-rpc WebSocket. The closure clones the descriptor
    /// table (cheap — descriptors are `Arc`-backed) into the upgrade future so
    /// the resulting service is `Clone + Send + Sync + 'static`, satisfying the
    /// shape `axum::routing::get` expects.
    pub fn ws_handler(
        descriptors: Arc<Vec<ProcedureDescriptor>>,
    ) -> impl Fn(WebSocketUpgrade) -> futures::future::BoxFuture<'static, Response>
           + Clone
           + Send
           + Sync
           + 'static {
        move |upgrade: WebSocketUpgrade| {
            let descriptors = descriptors.clone();
            Box::pin(
                async move { upgrade.on_upgrade(move |socket| handle_socket(socket, descriptors)) },
            )
        }
    }

    /// Drive one upgraded WebSocket through its full lifecycle.
    ///
    /// Maintains a map of `id` → [`AbortHandle`] for active subscriptions, a
    /// reader that dispatches each incoming text frame to Subscribe /
    /// Unsubscribe, and a writer task that pumps an mpsc into the outbound
    /// half of the split socket so multiple subscription tasks can write
    /// concurrently without contending on the sink.
    async fn handle_socket(socket: WebSocket, descriptors: Arc<Vec<ProcedureDescriptor>>) {
        let mut active: HashMap<u64, AbortHandle> = HashMap::new();
        let (mut tx, mut rx) = socket.split();
        let (frame_tx, mut frame_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();

        // Writer task: drains frame_rx into the WS sink in arrival order.
        // Bails out on send error (peer closed) so we don't pile up frames
        // for a dead socket.
        let writer_handle = tokio::spawn(async move {
            while let Some(msg) = frame_rx.recv().await {
                if tx.send(msg).await.is_err() {
                    break;
                }
            }
        });

        while let Some(Ok(msg)) = rx.next().await {
            // axum 0.7: Message::Text carries an owned String. Binary frames
            // are not part of the SPEC §4.2 contract; we ignore them rather
            // than error so future binary extensions (e.g. CBOR multiplex)
            // can be layered without breaking older servers.
            let text = match msg {
                Message::Text(t) => t,
                Message::Close(_) => break,
                _ => continue,
            };

            // Decode the inbound frame into a generic WsMessage. Server only
            // honours Subscribe / Unsubscribe; everything else from the
            // client side (Data / Error / End) is silently ignored — those
            // variants exist on the same enum so the same type can be used by
            // server and client codecs, not because the server is obligated
            // to handle them inbound.
            let parsed: Result<crate::wire::WsMessage<serde_json::Value, serde_json::Value>, _> =
                serde_json::from_str(&text);
            let parsed = match parsed {
                Ok(p) => p,
                Err(e) => {
                    // No `id` is recoverable from a frame that didn't even
                    // parse, so we surface the decode error against id=0 and
                    // let the client correlate by transcript timing. Clients
                    // SHOULD treat id=0 as "global / no subscription".
                    let _ = frame_tx.send(Message::Text(
                        serde_json::to_string(&serde_json::json!({
                            "type": "error",
                            "payload": {
                                "id": 0u64,
                                "err": {
                                    "code": "decode_error",
                                    "payload": { "message": e.to_string() }
                                }
                            }
                        }))
                        .unwrap(),
                    ));
                    continue;
                }
            };

            match parsed {
                crate::wire::WsMessage::Subscribe {
                    id,
                    procedure,
                    input,
                } => {
                    let desc = descriptors.iter().find(|d| d.name == procedure);
                    let Some(desc) = desc else {
                        // Unknown procedure: error then end so the client can
                        // tear down per-id state symmetrically with normal
                        // completion. SPEC §4.1's `not_found` envelope shape
                        // is reused for parity with the HTTP transport.
                        let _ = frame_tx.send(Message::Text(
                            serde_json::to_string(&serde_json::json!({
                                "type": "error",
                                "payload": {
                                    "id": id,
                                    "err": {
                                        "code": "not_found",
                                        "payload": { "procedure": procedure }
                                    }
                                }
                            }))
                            .unwrap(),
                        ));
                        let _ = frame_tx.send(Message::Text(
                            serde_json::to_string(&serde_json::json!({
                                "type": "end",
                                "payload": { "id": id }
                            }))
                            .unwrap(),
                        ));
                        continue;
                    };

                    // Per SPEC §4.2 only subscriptions are reachable on WS in
                    // v0.1. Reject query/mutation procedures with a stable
                    // `not_subscription` code so clients can distinguish
                    // "wrong transport" from "wrong procedure". The kind tag
                    // and the body variant should always agree here, but we
                    // pattern-match on `body` directly so this stays correct
                    // even if a future macro emission drifts the two apart.
                    let stream_handler = match &desc.body {
                        ProcedureBody::Stream(h) => h.clone(),
                        ProcedureBody::Unary(_) => {
                            debug_assert_ne!(
                                desc.kind,
                                ProcKindRuntime::Subscription,
                                "subscription kind paired with unary body"
                            );
                            let _ = frame_tx.send(Message::Text(
                                serde_json::to_string(&serde_json::json!({
                                    "type": "error",
                                    "payload": {
                                        "id": id,
                                        "err": {
                                            "code": "not_subscription",
                                            "payload": serde_json::Value::Null
                                        }
                                    }
                                }))
                                .unwrap(),
                            ));
                            let _ = frame_tx.send(Message::Text(
                                serde_json::to_string(&serde_json::json!({
                                    "type": "end",
                                    "payload": { "id": id }
                                }))
                                .unwrap(),
                            ));
                            continue;
                        }
                    };

                    // Spawn the per-subscription driver task. Each task owns:
                    //  - its own clone of `frame_tx` so it can write
                    //    concurrently with peers,
                    //  - an `Abortable` wrapper so `Unsubscribe` and
                    //    socket-disconnect can short-circuit the stream
                    //    without the user's stream having to be cancellation-
                    //    aware itself.
                    let stream = stream_handler(input);
                    let (abort_handle, abort_reg) = AbortHandle::new_pair();
                    active.insert(id, abort_handle);
                    let frame_tx = frame_tx.clone();
                    let abortable = Abortable::new(stream, abort_reg);
                    tokio::spawn(async move {
                        futures::pin_mut!(abortable);
                        while let Some(frame) = abortable.next().await {
                            let envelope = match frame {
                                StreamFrame::Data(value) => serde_json::json!({
                                    "type": "data",
                                    "payload": { "id": id, "value": value }
                                }),
                                StreamFrame::Error { code, payload } => serde_json::json!({
                                    "type": "error",
                                    "payload": {
                                        "id": id,
                                        "err": { "code": code, "payload": payload }
                                    }
                                }),
                            };
                            if frame_tx
                                .send(Message::Text(serde_json::to_string(&envelope).unwrap()))
                                .is_err()
                            {
                                // Writer is gone (socket closed); abandon the
                                // task. The AbortHandle in `active` will be
                                // dropped on socket teardown — there's no
                                // need for us to clean it up here.
                                return;
                            }
                        }
                        // Abortable returns None either on stream completion
                        // or on cancellation — in both cases we send `End`
                        // so the client can release per-id state. Sending
                        // End after a cancellation is harmless; the client
                        // already knows it asked to unsubscribe and will
                        // ignore the duplicate signal.
                        let _ = frame_tx.send(Message::Text(
                            serde_json::to_string(&serde_json::json!({
                                "type": "end",
                                "payload": { "id": id }
                            }))
                            .unwrap(),
                        ));
                    });
                }
                crate::wire::WsMessage::Unsubscribe { id } => {
                    if let Some(handle) = active.remove(&id) {
                        handle.abort();
                    }
                }
                // Server ignores Data / Error / End / V from the client —
                // those variants are server-to-client. Re-emitting an error
                // here would punish clients that share a single WsMessage
                // codec across both directions.
                _ => {}
            }
        }

        // Reader loop ended (peer closed, IO error, or Close frame). Drop all
        // outstanding subscriptions so their tasks observe the abort and exit
        // promptly, then shut the writer down — its mpsc will drain when the
        // last frame_tx clone is dropped, but we abort it explicitly to bound
        // shutdown latency in the case where a procedure task is mid-frame.
        for (_, h) in active.drain() {
            h.abort();
        }
        writer_handle.abort();
    }
}

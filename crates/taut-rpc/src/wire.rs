//! Wire format types for taut-rpc (server <-> client JSON shapes).
//!
//! See SPEC §4 for the full contract. Summary:
//!
//! - **Query / mutation** (§4.1): `POST /rpc/<procedure>` with body `{ "input": ... }`,
//!   responding with either `{ "ok": ... }` (2xx) or `{ "err": { "code", "payload" } }`
//!   (4xx/5xx). `GET /rpc/<procedure>?input=...` is allowed for procedures explicitly
//!   opted in via `#[rpc(method = "GET")]`.
//! - **Subscription** (§4.2): SSE stream where each event is one [`SubFrame`]. The
//!   WebSocket transport carries the same logical frames inside [`WsMessage`]
//!   envelopes that multiplex multiple subscriptions over one socket.
//! - **Errors** (§3.3 / §4.1): every error is an [`ErrEnvelope`] of `{ code, payload }`,
//!   where `code` is a stable `&'static str` discriminant and `payload` is the
//!   error's serialised data.
//! - **Versioning** (§9): subscriptions may emit a leading `SubFrame::V { v }` marker;
//!   absence implies v0.
//!
//! These types are deliberately generic over input/output/error so the same
//! envelope works for every procedure: monomorphisation happens at the call site.

use serde::{Deserialize, Serialize};

/// Request body for queries and mutations: `POST /rpc/<procedure>` with body `{ "input": ... }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcRequest<I> {
    pub input: I,
}

/// Response body for queries and mutations.
///
/// Tagged so `{ "ok": ... }` and `{ "err": ... }` deserialise into the right variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcResponse<T, E> {
    Ok { ok: T },
    Err { err: E },
}

impl<T, E> RpcResponse<T, E> {
    /// Collapse the envelope into a plain `Result`.
    pub fn into_result(self) -> Result<T, E> {
        match self {
            RpcResponse::Ok { ok } => Ok(ok),
            RpcResponse::Err { err } => Err(err),
        }
    }
}

/// Error envelope per SPEC §3.3 / §4.1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrEnvelope<P> {
    pub code: String,
    pub payload: P,
}

impl<P> ErrEnvelope<P> {
    /// Construct a new error envelope.
    pub fn new(code: impl Into<String>, payload: P) -> Self {
        Self {
            code: code.into(),
            payload,
        }
    }
}

/// Subscription frame, transported as one SSE event each (SPEC §4.2).
///
/// `End` is modelled as a unit variant. With `#[serde(tag = "type", content = "payload")]`
/// serde emits `{"type":"end","payload":null}` for unit variants — the explicit `null`
/// is acceptable per SPEC and is locked in by a test below. Decoders that need a
/// payload-less form on the wire should special-case the `end` event before parsing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum SubFrame<T, E> {
    Data(T),
    Error(ErrEnvelope<E>),
    End,
    /// Optional protocol-version marker (subscription only, per SPEC §9).
    V {
        v: u32,
    },
}

/// WebSocket envelope (alternative transport). Same `{type, payload}` shape, different framing.
///
/// Each WebSocket carries multiple logical streams keyed by `id`, so unlike SSE
/// frames every variant carries its subscription identifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum WsMessage<T, E> {
    /// Client -> server: subscribe to a procedure.
    Subscribe {
        id: u64,
        procedure: String,
        input: serde_json::Value,
    },
    /// Client -> server: cancel.
    Unsubscribe { id: u64 },
    /// Server -> client: data frame.
    Data { id: u64, value: T },
    /// Server -> client.
    Error { id: u64, err: ErrEnvelope<E> },
    /// Server -> client: stream ended normally.
    End { id: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rpc_request_serialises_with_input_field() {
        let req = RpcRequest { input: 42 };
        let s = serde_json::to_string(&req).unwrap();
        assert_eq!(s, r#"{"input":42}"#);
    }

    #[test]
    fn rpc_response_ok_roundtrips() {
        let resp: RpcResponse<u32, String> = RpcResponse::Ok { ok: 7 };
        let s = serde_json::to_string(&resp).unwrap();
        assert_eq!(s, r#"{"ok":7}"#);

        let parsed: RpcResponse<u32, String> = serde_json::from_str(&s).unwrap();
        match parsed {
            RpcResponse::Ok { ok } => assert_eq!(ok, 7),
            RpcResponse::Err { .. } => panic!("expected Ok"),
        }
    }

    #[test]
    fn rpc_response_err_roundtrips() {
        let resp: RpcResponse<u32, String> = RpcResponse::Err {
            err: "boom".to_string(),
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert_eq!(s, r#"{"err":"boom"}"#);

        let parsed: RpcResponse<u32, String> = serde_json::from_str(&s).unwrap();
        match parsed {
            RpcResponse::Err { err } => assert_eq!(err, "boom"),
            RpcResponse::Ok { .. } => panic!("expected Err"),
        }
    }

    #[test]
    fn rpc_response_into_result_collapses() {
        let ok: RpcResponse<u32, String> = RpcResponse::Ok { ok: 1 };
        assert_eq!(ok.into_result(), Ok(1));
        let err: RpcResponse<u32, String> = RpcResponse::Err {
            err: "x".to_string(),
        };
        assert_eq!(err.into_result(), Err("x".to_string()));
    }

    #[test]
    fn err_envelope_new_builds_via_into() {
        let e: ErrEnvelope<u32> = ErrEnvelope::new("not_found", 404);
        assert_eq!(e.code, "not_found");
        assert_eq!(e.payload, 404);
    }

    #[test]
    fn sub_frame_data_serialises_with_payload() {
        let f: SubFrame<u32, String> = SubFrame::Data(5);
        let s = serde_json::to_string(&f).unwrap();
        assert_eq!(s, r#"{"type":"data","payload":5}"#);
    }

    #[test]
    fn sub_frame_error_serialises_envelope() {
        let f: SubFrame<u32, String> = SubFrame::Error(ErrEnvelope::new("bad", "details".into()));
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "error",
                "payload": { "code": "bad", "payload": "details" }
            })
        );
    }

    /// Pin the on-the-wire shape of `SubFrame::End` so downstream decoders aren't surprised.
    /// serde emits unit variants under `tag/content` without a `payload` field — i.e.
    /// `{"type":"end"}`, NOT `{"type":"end","payload":null}`. Decoders MUST tolerate both.
    #[test]
    fn sub_frame_end_serialises_without_payload_field() {
        let f: SubFrame<u32, String> = SubFrame::End;
        let s = serde_json::to_string(&f).unwrap();
        assert_eq!(s, r#"{"type":"end"}"#);

        // Roundtrips from the canonical form.
        let parsed: SubFrame<u32, String> = serde_json::from_str(&s).unwrap();
        assert!(matches!(parsed, SubFrame::End));

        // Also accepts the explicit-null form for tolerance.
        let parsed: SubFrame<u32, String> =
            serde_json::from_str(r#"{"type":"end","payload":null}"#).unwrap();
        assert!(matches!(parsed, SubFrame::End));
    }

    #[test]
    fn sub_frame_version_marker_roundtrips() {
        let f: SubFrame<u32, String> = SubFrame::V { v: 1 };
        let s = serde_json::to_string(&f).unwrap();
        assert_eq!(s, r#"{"type":"v","payload":{"v":1}}"#);

        let parsed: SubFrame<u32, String> = serde_json::from_str(&s).unwrap();
        match parsed {
            SubFrame::V { v } => assert_eq!(v, 1),
            _ => panic!("expected V"),
        }
    }

    #[test]
    fn ws_message_subscribe_roundtrips() {
        let m: WsMessage<u32, String> = WsMessage::Subscribe {
            id: 42,
            procedure: "user.events".to_string(),
            input: json!({ "userId": 1 }),
        };
        let s = serde_json::to_string(&m).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "subscribe",
                "payload": {
                    "id": 42,
                    "procedure": "user.events",
                    "input": { "userId": 1 }
                }
            })
        );

        let parsed: WsMessage<u32, String> = serde_json::from_value(v).unwrap();
        match parsed {
            WsMessage::Subscribe {
                id,
                procedure,
                input,
            } => {
                assert_eq!(id, 42);
                assert_eq!(procedure, "user.events");
                assert_eq!(input, json!({ "userId": 1 }));
            }
            _ => panic!("expected Subscribe"),
        }
    }

    #[test]
    fn ws_message_data_roundtrips() {
        let m: WsMessage<u32, String> = WsMessage::Data { id: 1, value: 99 };
        let s = serde_json::to_string(&m).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "data",
                "payload": { "id": 1, "value": 99 }
            })
        );

        let parsed: WsMessage<u32, String> = serde_json::from_value(v).unwrap();
        match parsed {
            WsMessage::Data { id, value } => {
                assert_eq!(id, 1);
                assert_eq!(value, 99);
            }
            _ => panic!("expected Data"),
        }
    }
}

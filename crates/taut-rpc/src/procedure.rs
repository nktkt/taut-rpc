//! Type-erased procedure contract used by `#[rpc]`-emitted code to register
//! a procedure with the [`crate::router::Router`].
//!
//! The `#[rpc]` proc-macro emits a [`ProcedureDescriptor`] per annotated
//! function: a static name, a runtime kind tag, the IR fragment and reachable
//! [`crate::ir::TypeDef`]s for codegen, and a type-erased async body in the
//! form of a [`ProcedureBody`].
//!
//! The body is one of two shapes:
//!
//! - [`ProcedureBody::Unary`] — for queries and mutations. A future-returning
//!   closure shaped like SPEC §4.1's request/response cycle: take the JSON
//!   `input`, return a single [`ProcedureResult`].
//! - [`ProcedureBody::Stream`] — for subscriptions (Phase 3). A stream-returning
//!   closure: take the JSON `input`, yield a sequence of [`StreamFrame`]s,
//!   each mapping to one SSE frame per SPEC §4.2 (`event: data` or
//!   `event: error`). End-of-stream is implicit when the stream finishes —
//!   the router emits the closing `event: end\ndata:\n\n` frame itself.
//!
//! Both shapes are wrapped in `Arc<dyn Fn>` so descriptors are cheap to clone
//! and trivially `Send + Sync` — exactly what a shared `Router` needs to
//! dispatch concurrent requests across procedures. The deserialize → call user
//! fn → serialize cycle is owned entirely by the macro emission: the body
//! closure already accepts `serde_json::Value` for the input and produces
//! pre-serialized payloads. The [`crate::router::Router`] knows nothing about
//! input/output types — its job is purely HTTP framing.

use std::sync::Arc;

use futures::future::BoxFuture;
use futures::stream::BoxStream;

/// Type-erased async unary handler — used for queries and mutations.
///
/// Takes a JSON `Value` (the already-extracted `input` field of the §4.1
/// request envelope) and returns a future resolving to a single
/// [`ProcedureResult`]. Wrapped in `Arc<dyn Fn>` so the descriptor is
/// `Clone + Send + Sync` while keeping the closure type erased.
pub type UnaryHandler =
    Arc<dyn Fn(serde_json::Value) -> BoxFuture<'static, ProcedureResult> + Send + Sync>;

/// Type-erased async streaming handler — used for subscriptions (SPEC §4.2).
///
/// Takes a JSON `Value` (the request input) and returns a `BoxStream` of
/// [`StreamFrame`]s. Each yielded frame maps to one SSE event per SPEC §4.2:
/// [`StreamFrame::Data`] becomes `event: data`, [`StreamFrame::Error`]
/// becomes `event: error`. End-of-stream is implicit — when the stream
/// finishes, the router emits the closing `event: end\ndata:\n\n` frame.
pub type StreamHandler =
    Arc<dyn Fn(serde_json::Value) -> BoxStream<'static, StreamFrame> + Send + Sync>;

/// Backwards-compatible alias. Older code (and the Phase 1/2 macro emission)
/// referred to a single `ProcedureHandler` type that was implicitly unary;
/// keep the name pointed at [`UnaryHandler`] so unrelated call sites compile
/// unchanged across the Phase 3 split.
pub type ProcedureHandler = UnaryHandler;

/// Outcome of invoking a [`UnaryHandler`].
///
/// Maps directly to the SPEC §4.1 wire envelope: [`Self::Ok`] becomes
/// `200 { "ok": <payload> }`; [`Self::Err`] becomes
/// `<http_status> { "err": { "code", "payload" } }`.
pub enum ProcedureResult {
    /// Successful response — the JSON value sent back as `{"ok": ...}`.
    Ok(serde_json::Value),
    /// Failure response — sent back as `{"err": {"code", "payload"}}` with
    /// the given HTTP status.
    Err {
        /// HTTP status code returned to the caller.
        http_status: u16,
        /// Stable, machine-readable error code.
        code: String,
        /// Error payload serialized into the wire envelope.
        payload: serde_json::Value,
    },
}

impl ProcedureResult {
    /// Serialize a value into [`ProcedureResult::Ok`].
    ///
    /// On serialization failure, falls back to a 500 `serialization_error`
    /// with a `null` payload — there's no useful structured payload to emit
    /// when serde itself failed, and surfacing the raw `serde_json::Error`
    /// would leak Rust-internal type names to the wire.
    pub fn ok(value: impl serde::Serialize) -> Self {
        match serde_json::to_value(&value) {
            Ok(v) => ProcedureResult::Ok(v),
            Err(_) => ProcedureResult::Err {
                http_status: 500,
                code: "serialization_error".to_string(),
                payload: serde_json::Value::Null,
            },
        }
    }

    /// Build [`ProcedureResult::Err`] from a status, stable code, and
    /// serializable payload. Same fallback semantics as [`Self::ok`] when the
    /// payload fails to serialize.
    pub fn err(http_status: u16, code: impl Into<String>, payload: impl serde::Serialize) -> Self {
        match serde_json::to_value(&payload) {
            Ok(payload) => ProcedureResult::Err {
                http_status,
                code: code.into(),
                payload,
            },
            Err(_) => ProcedureResult::Err {
                http_status: 500,
                code: "serialization_error".to_string(),
                payload: serde_json::Value::Null,
            },
        }
    }

    /// Build [`ProcedureResult::Err`] from a [`crate::TautError`]. The payload
    /// is `serde_json::to_value(&e)`; if that fails the payload becomes
    /// `null` but `code` and `http_status` are still taken from the error.
    #[allow(clippy::needless_pass_by_value)] // owned `e` matches macro-emitted call sites
    pub fn from_taut_error<E: crate::TautError>(e: E) -> Self {
        let code = e.code().to_string();
        let http_status = e.http_status();
        let payload = serde_json::to_value(&e).unwrap_or(serde_json::Value::Null);
        ProcedureResult::Err {
            http_status,
            code,
            payload,
        }
    }

    /// Convenience helper for macro-emitted code: maps a `serde_json::Error`
    /// (typically from output serialization in the handler wrapper) to a
    /// uniform 500 `serialization_error` response.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)] // owned arg matches macro-emitted call sites
    pub fn from_serialization(_e: serde_json::Error) -> Self {
        ProcedureResult::Err {
            http_status: 500,
            code: "serialization_error".to_string(),
            payload: serde_json::Value::Null,
        }
    }
}

/// One frame yielded by a [`StreamHandler`].
///
/// Mirrors the SPEC §4.2 SSE event shapes:
///
/// - [`Self::Data`] → `event: data\ndata: <json>\n\n`
/// - [`Self::Error`] → `event: error\ndata: <{code,payload}>\n\n`
///
/// The terminal `event: end\ndata:\n\n` frame is implicit — when the
/// underlying stream finishes, the router emits it. Stream handlers should
/// just stop yielding rather than try to encode the end frame themselves.
///
/// `StreamFrame` is intentionally runtime-only: it carries pre-serialized
/// `serde_json::Value`s so the router can splat them into SSE bodies without
/// re-running user `Serialize` impls. It does **not** implement
/// `serde::Serialize`/`Deserialize` itself — there's no wire shape to round
/// trip.
#[derive(Debug, Clone)]
pub enum StreamFrame {
    /// A successful payload frame. Becomes `event: data\ndata: <json>\n\n`
    /// on the SSE wire.
    Data(serde_json::Value),
    /// An error frame. Becomes `event: error\ndata: {"code","payload"}\n\n`
    /// on the SSE wire. Streaming errors do **not** terminate the connection
    /// at the SPEC level — the user's stream chooses whether to keep yielding
    /// after an `Error` frame or stop. (The HTTP response is already
    /// committed by the time SSE frames flow, so there's no status code to
    /// flip.)
    Error {
        /// Stable error code emitted with the SSE error frame.
        code: String,
        /// Error payload serialized into the SSE error frame.
        payload: serde_json::Value,
    },
}

impl StreamFrame {
    /// Serialize a value into [`StreamFrame::Data`].
    ///
    /// On serialization failure, falls back to a [`StreamFrame::Error`] with
    /// `code = "serialization_error"` and a `null` payload — same fallback
    /// shape as [`ProcedureResult::ok`], for consistency between the unary
    /// and streaming paths.
    pub fn data(value: impl serde::Serialize) -> Self {
        match serde_json::to_value(&value) {
            Ok(v) => StreamFrame::Data(v),
            Err(_) => StreamFrame::Error {
                code: "serialization_error".to_string(),
                payload: serde_json::Value::Null,
            },
        }
    }

    /// Build [`StreamFrame::Error`] from a stable code and serializable
    /// payload. Same fallback semantics as [`Self::data`] when the payload
    /// fails to serialize.
    pub fn err(code: impl Into<String>, payload: impl serde::Serialize) -> Self {
        match serde_json::to_value(&payload) {
            Ok(payload) => StreamFrame::Error {
                code: code.into(),
                payload,
            },
            Err(_) => StreamFrame::Error {
                code: "serialization_error".to_string(),
                payload: serde_json::Value::Null,
            },
        }
    }

    /// Build [`StreamFrame::Error`] from a [`crate::TautError`]. The payload
    /// is `serde_json::to_value(&e)`; if that fails the payload becomes
    /// `null` but `code` is still taken from the error.
    ///
    /// Note that, unlike the unary [`ProcedureResult::from_taut_error`], the
    /// `http_status` of the error is intentionally dropped: SSE frames flow
    /// after the HTTP status line is already committed, so per-frame status
    /// codes don't fit. Callers wanting status-mapping semantics should use a
    /// unary procedure instead.
    #[allow(clippy::needless_pass_by_value)] // owned `e` matches macro-emitted call sites
    pub fn from_taut_error<E: crate::TautError>(e: E) -> Self {
        let code = e.code().to_string();
        let payload = serde_json::to_value(&e).unwrap_or(serde_json::Value::Null);
        StreamFrame::Error { code, payload }
    }
}

/// Body of a [`ProcedureDescriptor`] — either a unary handler (queries and
/// mutations, SPEC §4.1) or a streaming handler (subscriptions, SPEC §4.2).
///
/// `Clone` because [`UnaryHandler`] / [`StreamHandler`] are themselves `Arc`s
/// — cloning a `ProcedureBody` just bumps refcounts.
///
/// # Examples
///
/// Pattern-match a descriptor's body to dispatch on the procedure flavor.
/// This is the same shape the router itself uses internally:
///
/// ```rust,ignore
/// use taut_rpc::procedure::{ProcedureBody, ProcedureDescriptor};
///
/// fn describe(desc: &ProcedureDescriptor) -> &'static str {
///     match &desc.body {
///         ProcedureBody::Unary(_) => "query or mutation",
///         ProcedureBody::Stream(_) => "subscription",
///     }
/// }
/// ```
#[derive(Clone)]
pub enum ProcedureBody {
    /// Unary handler — used by queries and mutations (SPEC §4.1).
    Unary(UnaryHandler),
    /// Streaming handler — used by subscriptions (SPEC §4.2).
    Stream(StreamHandler),
}

/// Runtime descriptor for a single `#[rpc]` procedure.
///
/// Built by the `#[rpc]` macro at compile time and registered with
/// [`crate::router::Router`] at startup. Carries everything the router needs
/// to dispatch a request (`name`, `kind`, `body`) plus everything the IR
/// document needs to describe this procedure to the TypeScript codegen
/// (`ir`, `type_defs`).
#[derive(Clone)]
pub struct ProcedureDescriptor {
    /// Procedure name. Matches the underlying Rust function name and is the
    /// path segment in `/rpc/<name>`.
    pub name: &'static str,
    /// Runtime tag distinguishing query / mutation / subscription dispatch.
    pub kind: crate::router::ProcKindRuntime,
    /// IR fragment (input/output types, HTTP method, doc) for this procedure.
    pub ir: crate::ir::Procedure,
    /// All [`crate::ir::TypeDef`]s reachable from this procedure's signature.
    /// Router-level IR assembly deduplicates across procedures.
    pub type_defs: Vec<crate::ir::TypeDef>,
    /// Type-erased async body — unary for query/mutation, streaming for
    /// subscriptions. Phase 3 replaces the Phase 1/2 single `handler` field
    /// with this two-variant body.
    pub body: ProcedureBody,
}

impl std::fmt::Debug for ProcedureDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Skip the actual handler closure (no useful Debug for `dyn Fn`); just
        // print which variant of `ProcedureBody` we're holding so logs show
        // the unary-vs-stream split. IR input/output refs round out the
        // procedure shape.
        let body_kind = match &self.body {
            ProcedureBody::Unary(_) => "Unary",
            ProcedureBody::Stream(_) => "Stream",
        };
        f.debug_struct("ProcedureDescriptor")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("body", &body_kind)
            .field("input", &self.ir.input)
            .field("output", &self.ir.output)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_serializes_to_expected_json_value() {
        let r = ProcedureResult::ok(42u32);
        match r {
            ProcedureResult::Ok(v) => assert_eq!(v, serde_json::json!(42)),
            ProcedureResult::Err { .. } => panic!("expected Ok"),
        }
    }

    #[test]
    fn err_builds_envelope_with_supplied_fields() {
        let r = ProcedureResult::err(404, "not_found", serde_json::Value::Null);
        match r {
            ProcedureResult::Err {
                http_status,
                code,
                payload,
            } => {
                assert_eq!(http_status, 404);
                assert_eq!(code, "not_found");
                assert_eq!(payload, serde_json::Value::Null);
            }
            ProcedureResult::Ok(_) => panic!("expected Err"),
        }
    }

    #[test]
    fn from_taut_error_preserves_code_and_status() {
        let r = ProcedureResult::from_taut_error(crate::error::StandardError::Unauthenticated);
        match r {
            ProcedureResult::Err {
                http_status, code, ..
            } => {
                assert_eq!(code, "unauthenticated");
                assert_eq!(http_status, 401);
            }
            ProcedureResult::Ok(_) => panic!("expected Err"),
        }
    }

    #[test]
    fn ok_payload_roundtrips_through_serde_json_string() {
        let value = serde_json::json!({ "id": 7, "name": "ada" });
        let r = ProcedureResult::Ok(value.clone());
        let encoded = match r {
            ProcedureResult::Ok(v) => serde_json::to_string(&v).expect("encode"),
            ProcedureResult::Err { .. } => panic!("expected Ok"),
        };
        let decoded: serde_json::Value = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded, value);
    }

    // ---- Phase 3: ProcedureBody / StreamFrame -------------------------------

    /// Smallest possible IR fragment for tests — fields the router/IR loop
    /// don't care about for a closure-dispatch test, but that we still need
    /// to construct a `ProcedureDescriptor`.
    fn dummy_procedure_ir(name: &str) -> crate::ir::Procedure {
        use crate::ir::{HttpMethod, Primitive, ProcKind, TypeRef};
        crate::ir::Procedure {
            name: name.to_string(),
            kind: ProcKind::Query,
            input: TypeRef::Primitive(Primitive::Unit),
            output: TypeRef::Primitive(Primitive::Unit),
            errors: vec![],
            http_method: HttpMethod::Post,
            doc: None,
        }
    }

    #[tokio::test]
    async fn unary_body_dispatches_through_handler() {
        // Construct a `ProcedureBody::Unary` directly (i.e. without going
        // through the macro emission), call its handler with a JSON value,
        // and assert the result echoes back.
        let handler: UnaryHandler = Arc::new(|input: serde_json::Value| {
            Box::pin(async move { ProcedureResult::Ok(input) })
        });
        let desc = ProcedureDescriptor {
            name: "echo",
            kind: crate::router::ProcKindRuntime::Query,
            ir: dummy_procedure_ir("echo"),
            type_defs: vec![],
            body: ProcedureBody::Unary(handler),
        };

        let h = match &desc.body {
            ProcedureBody::Unary(h) => h.clone(),
            ProcedureBody::Stream(_) => panic!("expected Unary body"),
        };
        let result = h(serde_json::json!({"hello": "world"})).await;
        match result {
            ProcedureResult::Ok(v) => assert_eq!(v, serde_json::json!({"hello": "world"})),
            ProcedureResult::Err { .. } => panic!("expected Ok"),
        }
    }

    #[tokio::test]
    async fn stream_body_emits_collected_frames() {
        use futures::stream::{self, StreamExt};

        // Yield three `StreamFrame::Data` items — proves the descriptor's
        // streaming side compiles, runs, and produces the expected sequence.
        let handler: StreamHandler = Arc::new(|_input: serde_json::Value| {
            let frames = vec![
                StreamFrame::Data(serde_json::json!(1)),
                StreamFrame::Data(serde_json::json!(2)),
                StreamFrame::Data(serde_json::json!(3)),
            ];
            stream::iter(frames).boxed()
        });
        let desc = ProcedureDescriptor {
            name: "counter",
            kind: crate::router::ProcKindRuntime::Subscription,
            ir: dummy_procedure_ir("counter"),
            type_defs: vec![],
            body: ProcedureBody::Stream(handler),
        };

        let s = match &desc.body {
            ProcedureBody::Stream(s) => s.clone(),
            ProcedureBody::Unary(_) => panic!("expected Stream body"),
        };
        let frames: Vec<StreamFrame> = s(serde_json::Value::Null).collect().await;
        assert_eq!(frames.len(), 3);
        let values: Vec<serde_json::Value> = frames
            .into_iter()
            .map(|f| match f {
                StreamFrame::Data(v) => v,
                StreamFrame::Error { .. } => panic!("expected Data frame"),
            })
            .collect();
        assert_eq!(
            values,
            vec![
                serde_json::json!(1),
                serde_json::json!(2),
                serde_json::json!(3),
            ]
        );
    }

    #[test]
    fn stream_frame_data_serializes_payload_in_place() {
        // `StreamFrame` is runtime-only — it doesn't implement Serialize /
        // Deserialize, so there's no JSON round-trip to assert. Instead,
        // verify that `StreamFrame::data` serializes its argument *into* the
        // variant payload (so the router doesn't need to re-serialize).
        let f = StreamFrame::data(42u32);
        match f {
            StreamFrame::Data(v) => assert_eq!(v, serde_json::json!(42)),
            StreamFrame::Error { .. } => panic!("expected Data variant"),
        }
    }

    #[test]
    fn stream_frame_err_builds_error_variant() {
        let f = StreamFrame::err("rate_limited", serde_json::json!({"retry_after": 5}));
        match f {
            StreamFrame::Error { code, payload } => {
                assert_eq!(code, "rate_limited");
                assert_eq!(payload, serde_json::json!({"retry_after": 5}));
            }
            StreamFrame::Data(_) => panic!("expected Error variant"),
        }
    }

    #[test]
    fn stream_frame_from_taut_error_preserves_code() {
        let f = StreamFrame::from_taut_error(crate::error::StandardError::Unauthenticated);
        match f {
            StreamFrame::Error { code, .. } => assert_eq!(code, "unauthenticated"),
            StreamFrame::Data(_) => panic!("expected Error variant"),
        }
    }
}

//! Type-erased procedure contract used by `#[rpc]`-emitted code to register
//! a procedure with the [`crate::router::Router`].
//!
//! The `#[rpc]` proc-macro emits a [`ProcedureDescriptor`] per annotated
//! function: a static name, a runtime kind tag, the IR fragment and reachable
//! [`crate::ir::TypeDef`]s for codegen, and a type-erased async [`ProcedureHandler`].
//! The handler is `Arc<dyn Fn(...) -> BoxFuture<...> + Send + Sync>` so
//! descriptors are cheap to clone and trivially `Send + Sync` — exactly what a
//! shared `Router` needs to dispatch concurrent requests across procedures.
//!
//! The deserialize → call user fn → serialize cycle is owned entirely by the
//! macro emission: the handler closure already accepts `serde_json::Value` for
//! the input and produces a [`ProcedureResult`] carrying a pre-serialized
//! payload. The [`crate::router::Router`] knows nothing about input/output
//! types — its job is purely HTTP framing (route the request to the named
//! handler, translate [`ProcedureResult`] into the SPEC §4.1 wire envelope).
//! This split is what keeps the runtime and macro halves orthogonal.

use std::sync::Arc;

use futures::future::BoxFuture;

/// Type-erased async procedure handler.
///
/// Takes a JSON `Value` (the already-extracted `input` field of the §4.1
/// request envelope) and returns a future resolving to a [`ProcedureResult`].
/// Wrapped in `Arc<dyn Fn>` so the descriptor is `Clone + Send + Sync` while
/// keeping the closure type erased.
pub type ProcedureHandler =
    Arc<dyn Fn(serde_json::Value) -> BoxFuture<'static, ProcedureResult> + Send + Sync>;

/// Outcome of invoking a [`ProcedureHandler`].
///
/// Maps directly to the SPEC §4.1 wire envelope: [`Self::Ok`] becomes
/// `200 { "ok": <payload> }`; [`Self::Err`] becomes
/// `<http_status> { "err": { "code", "payload" } }`.
pub enum ProcedureResult {
    Ok(serde_json::Value),
    Err {
        http_status: u16,
        code: String,
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
    pub fn from_serialization(_e: serde_json::Error) -> Self {
        ProcedureResult::Err {
            http_status: 500,
            code: "serialization_error".to_string(),
            payload: serde_json::Value::Null,
        }
    }
}

/// Runtime descriptor for a single `#[rpc]` procedure.
///
/// Built by the `#[rpc]` macro at compile time and registered with
/// [`crate::router::Router`] at startup. Carries everything the router needs
/// to dispatch a request (`name`, `kind`, `handler`) plus everything the IR
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
    /// Type-erased async handler. The closure does decode → call → encode;
    /// the router only does HTTP framing.
    pub handler: ProcedureHandler,
}

impl std::fmt::Debug for ProcedureDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Skip `handler` (it's a closure, no useful Debug). Print the IR's
        // input/output type refs so logs make procedure shape obvious.
        f.debug_struct("ProcedureDescriptor")
            .field("name", &self.name)
            .field("kind", &self.kind)
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
}

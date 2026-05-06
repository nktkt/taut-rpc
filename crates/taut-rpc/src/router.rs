//! Server-side procedure router for taut-rpc. See SPEC §5.
//!
//! Phase 1: this module owns the runtime registration table for `#[rpc]`
//! procedures and converts that table into a real `axum::Router` that
//! dispatches `POST /rpc/<name>` against type-erased
//! [`crate::procedure::ProcedureHandler`]s.
//!
//! Wire-format obligations come from SPEC §4.1: success → `{"ok": <Output>}`,
//! errors → `{"err": {"code": ..., "payload": ...}}`. The success/error split
//! is communicated by the [`crate::procedure::ProcedureResult`] returned from
//! each handler; this module is responsible for shaping that into a real
//! HTTP response and for funneling decode failures and unknown-procedure
//! requests through the same envelope.
//!
//! # Phase boundaries
//!
//! - Phase 1 (this module): query + mutation dispatch via JSON over POST,
//!   debug introspection endpoints, the SPEC-shaped `not_found` fallback for
//!   unknown procedures, and a custom extractor that maps decode failures to
//!   the `decode_error` envelope.
//! - Phase 3 will add subscription dispatch (SSE/WebSocket per SPEC §4.2).
//!   For now, subscription procedures are accepted by `procedure(...)` so
//!   their IR is captured, but no HTTP route is mounted for them.
//! - Per-procedure `#[rpc(method = "GET")]` opt-in (SPEC §4.1) is also
//!   deferred — every Phase 1 procedure routes as POST.

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Router as AxumRouter;

use crate::procedure::{ProcedureDescriptor, ProcedureResult};
use crate::wire::RpcRequest;

/// Runtime tag for a registered procedure's flavor.
///
/// Mirrors [`crate::ir::ProcKind`] but kept distinct so the runtime side can
/// carry dispatch concerns (e.g. Phase 3 streaming) without leaking them into
/// the IR schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcKindRuntime {
    Query,
    Mutation,
    Subscription,
}

/// Registration table for `#[rpc]` procedures, mountable as an `axum::Router`.
///
/// The router is intentionally stateless in Phase 1 — Phase 2+ will reintroduce
/// a `with_state(...)` builder that threads an `S: Clone + Send + Sync` through
/// to handlers. Adding it later is non-breaking for callers that use the
/// no-state form documented in SPEC §5.
#[derive(Default)]
pub struct Router {
    procedures: Vec<ProcedureDescriptor>,
}

impl Router {
    /// Construct an empty router.
    #[must_use]
    pub fn new() -> Self {
        Self {
            procedures: Vec::new(),
        }
    }

    /// Register a procedure (typically from a `#[rpc]`-emitted
    /// `__taut_proc_<name>()` call).
    ///
    /// # Panics
    ///
    /// Panics if a procedure with this name is already registered. Duplicate
    /// procedure names would silently shadow each other on the `/rpc/<name>`
    /// path, so we surface this as a startup-time programmer error rather
    /// than letting it become a runtime mystery.
    #[must_use]
    pub fn procedure(mut self, desc: ProcedureDescriptor) -> Self {
        if self.procedures.iter().any(|p| p.name == desc.name) {
            panic!(
                "taut-rpc: procedure `{}` is already registered on this Router",
                desc.name
            );
        }
        self.procedures.push(desc);
        self
    }

    /// Snapshot the current IR document.
    ///
    /// Used by codegen and by the optional `/rpc/_ir` debug endpoint. Type
    /// defs are deduplicated by name across all registered procedures
    /// (procedures often share input/error types; emitting each one once
    /// keeps the IR stable for codegen).
    pub fn ir(&self) -> crate::ir::Ir {
        let mut procedures = Vec::with_capacity(self.procedures.len());
        let mut types: Vec<crate::ir::TypeDef> = Vec::new();
        let mut seen_type_names: std::collections::HashSet<String> = std::collections::HashSet::new();

        for desc in &self.procedures {
            procedures.push(desc.ir.clone());
            for td in &desc.type_defs {
                if seen_type_names.insert(td.name.clone()) {
                    types.push(td.clone());
                }
            }
        }

        crate::ir::Ir {
            ir_version: crate::ir::Ir::CURRENT_VERSION,
            procedures,
            types,
        }
    }

    /// Convert into an `axum::Router`.
    ///
    /// Mounts:
    /// - `GET  /rpc/_health` → `200 "ok"` (liveness probe).
    /// - `GET  /rpc/_procedures` → JSON array of registered procedure names.
    /// - `GET  /rpc/_ir` → full IR JSON, gated behind the `ir-export` feature.
    /// - `POST /rpc/<name>` for every registered query/mutation procedure.
    ///   Subscription procedures are skipped pending Phase 3 (SSE/WS, SPEC §4.2).
    ///
    /// Unknown routes fall through to a SPEC §4.1 envelope:
    /// `404 {"err": {"code": "not_found", "payload": {"procedure": "<name>"}}}`.
    #[must_use]
    pub fn into_axum(self) -> AxumRouter {
        // Capture the IR once up front so the `/rpc/_ir` handler can hand out
        // cheap clones of an `Arc<Ir>` rather than recomputing per-request.
        // The binding is only consumed under `cfg(feature = "ir-export")`, so
        // we suppress the unused-warning by leading-underscoring the name —
        // no `let _ = ...;` cargo-cult required.
        #[cfg_attr(not(feature = "ir-export"), allow(unused_variables))]
        let _ir: Arc<crate::ir::Ir> = Arc::new(self.ir());

        // Pre-collect procedure names for the `/rpc/_procedures` listing so
        // we can move the descriptor vec wholesale into the per-procedure
        // route loop below without juggling borrows.
        let names: Arc<Vec<String>> = Arc::new(
            self.procedures
                .iter()
                .map(|p| p.name.to_string())
                .collect(),
        );

        let mut app = AxumRouter::new()
            .route(
                "/rpc/_health",
                axum::routing::get(|| async { "ok" }),
            )
            .route(
                "/rpc/_procedures",
                axum::routing::get(move || {
                    let names = names.clone();
                    async move { axum::Json((*names).clone()) }
                }),
            );

        #[cfg(feature = "ir-export")]
        {
            let ir_for_route = _ir.clone();
            app = app.route(
                "/rpc/_ir",
                axum::routing::get(move || {
                    let ir = ir_for_route.clone();
                    async move { axum::Json((*ir).clone()) }
                }),
            );
        }

        for desc in self.procedures {
            match desc.kind {
                ProcKindRuntime::Query | ProcKindRuntime::Mutation => {
                    let handler = desc.handler.clone();
                    let path = format!("/rpc/{}", desc.name);
                    app = app.route(
                        &path,
                        axum::routing::post(move |input: RpcInput| {
                            let handler = handler.clone();
                            async move {
                                let RpcInput(value) = input;
                                let result = handler(value).await;
                                procedure_result_into_response(result)
                            }
                        }),
                    );
                }
                ProcKindRuntime::Subscription => {
                    // TODO(Phase 3): mount SSE GET / WebSocket route per SPEC §4.2.
                    // The descriptor's IR is still surfaced via `/rpc/_procedures`
                    // and `/rpc/_ir` so codegen can see subscriptions today.
                }
            }
        }

        app.fallback(not_found_fallback)
    }
}

/// Custom extractor wrapping `axum::Json<RpcRequest<serde_json::Value>>`.
///
/// Per SPEC §4.1 a malformed body must surface as the canonical envelope
/// `{"err": {"code": "decode_error", "payload": {"message": "..."}}}` with
/// status 400, not as axum's default plain-text rejection. We achieve that by
/// delegating to axum's `Json` extractor, then converting any [`JsonRejection`]
/// into a wire-shaped response.
struct RpcInput(serde_json::Value);

#[axum::async_trait]
impl<S> FromRequest<S> for RpcInput
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<RpcRequest<serde_json::Value>>::from_request(req, state).await {
            Ok(axum::Json(RpcRequest { input })) => Ok(RpcInput(input)),
            Err(rej) => Err(decode_error_response(&rej)),
        }
    }
}

/// Build the SPEC-shaped `decode_error` response from an axum JSON rejection.
fn decode_error_response(rej: &JsonRejection) -> Response {
    let body = serde_json::json!({
        "err": {
            "code": "decode_error",
            "payload": { "message": rej.body_text() },
        }
    });
    (StatusCode::BAD_REQUEST, axum::Json(body)).into_response()
}

/// Map a [`ProcedureResult`] to an HTTP response per SPEC §4.1.
fn procedure_result_into_response(result: ProcedureResult) -> Response {
    match result {
        ProcedureResult::Ok(value) => {
            let body = serde_json::json!({ "ok": value });
            (StatusCode::OK, axum::Json(body)).into_response()
        }
        ProcedureResult::Err {
            http_status,
            code,
            payload,
        } => {
            let status = StatusCode::from_u16(http_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body = serde_json::json!({
                "err": { "code": code, "payload": payload }
            });
            (status, axum::Json(body)).into_response()
        }
    }
}

/// Fallback handler that returns the SPEC §4.1 `not_found` envelope for any
/// path that didn't match a registered route. The procedure name is taken from
/// the request URI so clients can correlate it with their generated client's
/// path, even though no such procedure exists.
async fn not_found_fallback(req: Request) -> Response {
    // Strip the `/rpc/` prefix when present so the payload contains just
    // `"procedure": "<name>"`. For any other path (e.g. `/foo`) we surface
    // the raw path — there's no useful procedure name to extract there, but
    // returning the path keeps the envelope informative.
    let path = req.uri().path();
    let procedure = path.strip_prefix("/rpc/").unwrap_or(path).to_string();

    let body = serde_json::json!({
        "err": {
            "code": "not_found",
            "payload": { "procedure": procedure },
        }
    });
    (StatusCode::NOT_FOUND, axum::Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{HttpMethod, ProcKind, Primitive, Procedure, TypeRef};
    use crate::procedure::ProcedureHandler;
    use axum::body::Body;
    use futures::future::BoxFuture;
    use http::Request as HttpRequest;
    use tower::ServiceExt;

    fn make_descriptor(
        name: &'static str,
        kind: ProcKindRuntime,
        handler: ProcedureHandler,
    ) -> ProcedureDescriptor {
        let ir_kind = match kind {
            ProcKindRuntime::Query => ProcKind::Query,
            ProcKindRuntime::Mutation => ProcKind::Mutation,
            ProcKindRuntime::Subscription => ProcKind::Subscription,
        };
        ProcedureDescriptor {
            name,
            kind,
            ir: Procedure {
                name: name.to_string(),
                kind: ir_kind,
                input: TypeRef::Primitive(Primitive::Unit),
                output: TypeRef::Primitive(Primitive::Unit),
                errors: vec![],
                http_method: HttpMethod::Post,
                doc: None,
            },
            type_defs: vec![],
            handler,
        }
    }

    fn echo_handler() -> ProcedureHandler {
        Arc::new(|input: serde_json::Value| -> BoxFuture<'static, ProcedureResult> {
            Box::pin(async move { ProcedureResult::Ok(input) })
        })
    }

    fn not_found_handler() -> ProcedureHandler {
        Arc::new(|_input: serde_json::Value| -> BoxFuture<'static, ProcedureResult> {
            Box::pin(async move {
                ProcedureResult::Err {
                    http_status: 404,
                    code: "not_found".to_string(),
                    payload: serde_json::Value::Null,
                }
            })
        })
    }

    #[test]
    fn empty_router_builds() {
        // Smoke test: an empty router converts to an axum::Router without panic.
        let _: AxumRouter = Router::new().into_axum();
    }

    #[tokio::test]
    async fn health_endpoint_returns_ok() {
        let app = Router::new().into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/rpc/_health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"ok");
    }

    #[tokio::test]
    async fn procedures_endpoint_lists_registered_names() {
        let app = Router::new()
            .procedure(make_descriptor("alpha", ProcKindRuntime::Query, echo_handler()))
            .procedure(make_descriptor("beta", ProcKindRuntime::Mutation, echo_handler()))
            .into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/rpc/_procedures")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let names: Vec<String> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[tokio::test]
    async fn registered_query_dispatches_through_handler() {
        let app = Router::new()
            .procedure(make_descriptor(
                "echo",
                ProcKindRuntime::Query,
                echo_handler(),
            ))
            .into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/rpc/echo")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"input":42}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v, serde_json::json!({"ok": 42}));
    }

    #[tokio::test]
    async fn handler_error_surfaces_with_envelope_and_status() {
        let app = Router::new()
            .procedure(make_descriptor(
                "echo",
                ProcKindRuntime::Query,
                not_found_handler(),
            ))
            .into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/rpc/echo")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"input":null}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "err": { "code": "not_found", "payload": null }
            })
        );
    }

    #[tokio::test]
    async fn malformed_json_returns_decode_error_envelope() {
        let app = Router::new()
            .procedure(make_descriptor(
                "echo",
                ProcKindRuntime::Query,
                echo_handler(),
            ))
            .into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/rpc/echo")
                    .header("content-type", "application/json")
                    .body(Body::from("not json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["err"]["code"], serde_json::json!("decode_error"));
        // The exact rejection text is axum-version-dependent; assert structure
        // (a non-empty `message` string) rather than the precise wording.
        assert!(v["err"]["payload"]["message"].is_string());
        assert!(!v["err"]["payload"]["message"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_procedure_returns_not_found_envelope() {
        let app = Router::new().into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/rpc/nonexistent")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"input":null}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "err": {
                    "code": "not_found",
                    "payload": { "procedure": "nonexistent" }
                }
            })
        );
    }

    #[test]
    #[should_panic(expected = "already registered")]
    fn duplicate_procedure_name_panics() {
        let _ = Router::new()
            .procedure(make_descriptor("dup", ProcKindRuntime::Query, echo_handler()))
            .procedure(make_descriptor("dup", ProcKindRuntime::Query, echo_handler()));
    }

    #[tokio::test]
    async fn subscription_procedure_does_not_mount_an_http_route() {
        // Subscriptions are accepted (so their IR is captured) but not yet
        // routed — Phase 3 will add SSE/WS dispatch. Until then, hitting the
        // path falls through to the not_found fallback.
        let app = Router::new()
            .procedure(make_descriptor(
                "stream",
                ProcKindRuntime::Subscription,
                echo_handler(),
            ))
            .into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/rpc/stream")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"input":null}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn ir_snapshot_contains_registered_procedures() {
        let router = Router::new()
            .procedure(make_descriptor("a", ProcKindRuntime::Query, echo_handler()))
            .procedure(make_descriptor("b", ProcKindRuntime::Mutation, echo_handler()));

        let ir = router.ir();
        assert_eq!(ir.ir_version, crate::ir::Ir::CURRENT_VERSION);
        let names: Vec<&str> = ir.procedures.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }
}

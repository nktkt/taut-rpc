//! Server-side procedure router for taut-rpc. See SPEC §5.
//!
//! # Phase 1 TODO
//!
//! This file is a Day-0 stub. Phase 1 (see ROADMAP) needs to add:
//!
//! - Per-procedure routes derived from registered handlers (the `#[rpc]` macro
//!   will emit a richer `ProcedureDescriptor` carrying the boxed handler fn,
//!   input/output type info, and the chosen HTTP method per SPEC §4.1).
//! - JSON deserialization of the `{ "input": <Input> }` request body into the
//!   procedure's input type, including the `GET ?input=<urlencoded-json>` form
//!   for procedures marked `#[rpc(method = "GET")]`.
//! - Error envelope serialization: `Result<T, E: TautError>` → either
//!   `200 { "ok": <Output> }` or `4xx/5xx { "err": { "code", "payload" } }`
//!   per SPEC §4.1, with the status code derived from `TautError`.
//! - SSE (and WebSocket) framing for subscription procedures per SPEC §4.2:
//!   `event: data|error|end` with JSON `data:` payloads, plus the WS variant
//!   that uses `{ type, payload }` JSON messages.

use std::sync::Arc;

/// A collection of registered procedures, mountable as an `axum::Router`.
///
/// In Phase 1 (see ROADMAP) procedures are registered via the `#[rpc]` macro;
/// for now this is a stub that builds an empty `axum::Router` with `/rpc/_health`.
#[derive(Default)]
pub struct Router<S = ()> {
    procedures: Vec<ProcedureEntry>,
    _state: std::marker::PhantomData<S>,
}

#[derive(Debug)]
struct ProcedureEntry {
    name: String,
    #[allow(dead_code)] // populated by `__register`; consumed once handler routing lands in Phase 1.
    kind: ProcKindRuntime,
}

/// Runtime tag for a registered procedure's flavor.
///
/// Kept `pub(crate)` for now — the `#[rpc]` macro is in-tree and will be the
/// only caller of [`Router::__register`] until Phase 1 stabilizes the descriptor
/// shape; promoting to `pub` is fine once external callers need it.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum ProcKindRuntime {
    Query,
    Mutation,
    Subscription,
}

impl<S: Clone + Send + Sync + 'static> Router<S> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            procedures: vec![],
            _state: std::marker::PhantomData,
        }
    }

    /// Register a procedure. Stub — the macro-emitted form will pass a richer descriptor in Phase 1.
    #[doc(hidden)]
    #[must_use]
    pub fn __register(mut self, name: impl Into<String>, kind: ProcKindRuntime) -> Self {
        self.procedures.push(ProcedureEntry {
            name: name.into(),
            kind,
        });
        self
    }

    /// Convert to an `axum::Router`. Currently mounts only `/rpc/_health` returning 200 "ok"
    /// and `/rpc/_procedures` listing the names of every registered procedure.
    #[must_use]
    pub fn into_axum(self) -> axum::Router<S> {
        let registered = Arc::new(self.procedures);
        axum::Router::new()
            .route("/rpc/_health", axum::routing::get(|| async { "ok" }))
            .route(
                "/rpc/_procedures",
                axum::routing::get({
                    let regs = registered.clone();
                    move || {
                        let regs = regs.clone();
                        async move {
                            let names: Vec<String> =
                                regs.iter().map(|p| p.name.clone()).collect();
                            axum::Json(names)
                        }
                    }
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn empty_router_builds() {
        // Smoke test: the unit-state default router constructs without panic.
        let _: axum::Router = Router::<()>::new().into_axum();
    }

    #[tokio::test]
    async fn procedures_route_lists_registered_names() {
        let router: axum::Router = Router::<()>::new()
            .__register("alpha", ProcKindRuntime::Query)
            .__register("beta", ProcKindRuntime::Mutation)
            .into_axum();

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/rpc/_procedures")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let names: Vec<String> = serde_json::from_slice(&body_bytes).unwrap();

        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
    }
}

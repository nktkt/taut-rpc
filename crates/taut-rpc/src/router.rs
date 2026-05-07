//! Server-side procedure router for taut-rpc. See SPEC §5.
//!
//! This module owns the runtime registration table for `#[rpc]` procedures
//! and converts that table into a real `axum::Router`. Queries and mutations
//! dispatch as `POST /rpc/<name>` against [`crate::procedure::UnaryHandler`]s
//! returning JSON envelopes; subscriptions dispatch as
//! `GET /rpc/<name>?input=<urlencoded-json>` against
//! [`crate::procedure::StreamHandler`]s returning a `text/event-stream` body.
//!
//! Wire-format obligations come from SPEC §4. For unary procedures (§4.1):
//! success → `{"ok": <Output>}`, errors → `{"err": {"code": ..., "payload": ...}}`,
//! communicated by [`crate::procedure::ProcedureResult`]. For subscriptions
//! (§4.2): each [`crate::procedure::StreamFrame`] becomes an SSE event
//! (`event: data` / `event: error`), and the router appends a closing
//! `event: end` frame when the underlying stream completes.
//!
//! # Phase boundaries
//!
//! - Phase 1/2: query + mutation dispatch via JSON over POST, debug
//!   introspection endpoints, the SPEC-shaped `not_found` fallback for
//!   unknown procedures, a custom extractor that maps decode failures to the
//!   `decode_error` envelope, and `tower::Layer` integration via
//!   [`Router::layer`].
//! - Phase 3 (this revision): subscription dispatch via SSE per SPEC §4.2.
//!   Subscription procedures registered through [`Router::procedure`] mount a
//!   `GET /rpc/<name>?input=<urlencoded-json>` route that streams
//!   `event: data` / `event: error` frames terminated by `event: end`. Bad
//!   JSON in `?input=` produces a single in-band `event: error` frame whose
//!   `code` is `decode_error`, then the closing `event: end` — the SSE body
//!   has already been framed by the time we know the input is bad, so an
//!   HTTP 4xx isn't appropriate.
//! - Phase 3 (this revision, gated): WebSocket transport per SPEC §4.2.
//!   When the `ws` feature is enabled, [`Router::into_axum`] mounts a single
//!   `GET /rpc/_ws` route that multiplexes subscriptions over one connection
//!   using [`crate::wire::WsMessage`] frames. Implementation lives in
//!   `crate::ws::ws_route`. Queries and mutations remain on POST; only
//!   subscription procedures are addressable over WS in v0.1.
//! - Per-procedure `#[rpc(method = "GET")]` opt-in for queries (SPEC §4.1)
//!   is still deferred — every query/mutation routes as POST.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Query, Request};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, KeepAliveStream, Sse};
use axum::response::{IntoResponse, Response};
use axum::Router as AxumRouter;
use futures::stream::StreamExt;

use crate::procedure::{ProcedureBody, ProcedureDescriptor, ProcedureResult, StreamFrame};
use crate::wire::RpcRequest;

// WebSocket transport (SPEC §4.2). Mounted under `cfg(feature = "ws")` only;
// the source file lives at `src/ws.rs` rather than `src/router/ws.rs`, so we
// point `mod` at it explicitly via `#[path]`. Keeping the file alongside the
// other top-level transport modules (wire.rs, dump.rs, ...) matches the
// crate's existing layout, while still scoping the module under `router::` so
// the registration site below can write `crate::router::ws::ws_route::...`.
#[cfg(feature = "ws")]
#[path = "ws.rs"]
mod ws;

/// Runtime tag for a registered procedure's flavor.
///
/// Mirrors [`crate::ir::ProcKind`] but kept distinct so the runtime side can
/// carry dispatch concerns (e.g. Phase 3 streaming) without leaking them into
/// the IR schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcKindRuntime {
    /// Read-only request/response procedure.
    Query,
    /// State-mutating request/response procedure.
    Mutation,
    /// Long-lived stream of values pushed to the client.
    Subscription,
}

/// Type-erased adapter for a pending `tower::Layer`.
///
/// We can't store the `L` type on `Router` directly without making `Router`
/// generic over every layer the user composes — which would also make
/// `.layer()` change the type at every call (`Router<A> → Router<(B, A)>`),
/// killing the chainable builder ergonomics SPEC §5 wants.
///
/// Instead we capture the layer in a closure that knows how to apply itself
/// to an `axum::Router`, and erase its concrete type behind a trait object.
/// We use `FnMut` rather than `FnOnce` because `Box<dyn FnOnce>` is awkward
/// to call through (you can't move out of a `Box<dyn FnOnce>` on stable
/// without `unsized_fn_params` gymnastics). Each closure is invoked exactly
/// once at `into_axum()` time via [`Vec::drain`], so the FnMut/FnOnce
/// distinction is purely a calling-convention workaround.
type LayerApply = Box<dyn FnMut(AxumRouter) -> AxumRouter + Send + Sync>;

/// Registration table for `#[rpc]` procedures, mountable as an `axum::Router`.
///
/// The router is intentionally stateless in Phase 1 — Phase 2+ will reintroduce
/// a `with_state(...)` builder that threads an `S: Clone + Send + Sync` through
/// to handlers. Adding it later is non-breaking for callers that use the
/// no-state form documented in SPEC §5.
///
/// # Examples
///
/// Build a router with a single procedure and convert it into an
/// `axum::Router` ready to serve:
///
/// ```rust,ignore
/// use taut_rpc::Router;
///
/// #[taut_rpc::rpc]
/// async fn ping() -> &'static str { "pong" }
///
/// let app = Router::new()
///     .procedure(__taut_proc_ping())
///     .into_axum();
/// // mount `app` under your axum server, e.g. axum::serve(...).
/// ```
#[derive(Default)]
pub struct Router {
    procedures: Vec<ProcedureDescriptor>,
    /// Pending `tower::Layer` applications, recorded by [`Router::layer`] and
    /// folded over the built `axum::Router` at [`Router::into_axum`] time.
    /// Stored in call order — see `layer`'s docs for composition semantics.
    layers: Vec<LayerApply>,
}

impl Router {
    /// Construct an empty router.
    #[must_use]
    pub fn new() -> Self {
        Self {
            procedures: Vec::new(),
            layers: Vec::new(),
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
        assert!(
            !self.procedures.iter().any(|p| p.name == desc.name),
            "taut-rpc: procedure `{}` is already registered on this Router",
            desc.name
        );
        self.procedures.push(desc);
        self
    }

    /// Wrap the resulting `axum::Router` with a `tower::Layer`.
    ///
    /// Recorded layers are folded over the built router at [`Router::into_axum`]
    /// time, so you can keep registering procedures after a `.layer(...)` call —
    /// the layer applies to *all* routes mounted by this `Router`, regardless of
    /// the order in which procedures and layers were chained.
    ///
    /// # Composition order
    ///
    /// Layers compose in onion order: the **last** `.layer()` call is the
    /// **outermost** wrap, matching `axum::Router::layer` and `tower`'s
    /// convention. So:
    ///
    /// ```text
    /// Router::new().layer(Inner).layer(Outer)
    /// ```
    ///
    /// produces a stack where `Outer` sees the request first and the response
    /// last — equivalent to `axum_router.layer(Inner).layer(Outer)`.
    ///
    /// See SPEC §5 — middleware reuses axum's ecosystem, so any
    /// `tower::Layer` that works with `axum::Router::layer` works here.
    #[must_use]
    pub fn layer<L>(mut self, layer: L) -> Self
    where
        L: tower::Layer<axum::routing::Route> + Clone + Send + Sync + 'static,
        L::Service: tower::Service<axum::http::Request<axum::body::Body>, Error = std::convert::Infallible>
            + Clone
            + Send
            + Sync
            + 'static,
        <L::Service as tower::Service<axum::http::Request<axum::body::Body>>>::Response:
            axum::response::IntoResponse + 'static,
        <L::Service as tower::Service<axum::http::Request<axum::body::Body>>>::Future:
            Send + 'static,
    {
        // The closure is wrapped in `Option<L>` so we can `take()` the layer
        // out of it on first call — `axum::Router::layer` consumes the layer
        // by value, but our adapter is `FnMut`, not `FnOnce`. In practice the
        // closure is only ever called once (via `Vec::drain` in `into_axum`),
        // so this is a no-op safety net rather than load-bearing logic.
        let mut slot = Some(layer);
        self.layers.push(Box::new(move |r: AxumRouter| {
            let layer = slot
                .take()
                .expect("taut-rpc: layer adapter invoked more than once");
            r.layer(layer)
        }));
        self
    }

    /// Snapshot the current IR document.
    ///
    /// Used by codegen and by the optional `/rpc/_ir` debug endpoint. Type
    /// defs are deduplicated by name across all registered procedures
    /// (procedures often share input/error types; emitting each one once
    /// keeps the IR stable for codegen).
    #[must_use]
    pub fn ir(&self) -> crate::ir::Ir {
        let mut procedures = Vec::with_capacity(self.procedures.len());
        let mut types: Vec<crate::ir::TypeDef> = Vec::new();
        let mut seen_type_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();

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
    /// - `GET  /rpc/_version` → JSON `{ "taut_rpc": "<crate-version>",
    ///   "ir_version": <u32> }` for build/version introspection. Kept
    ///   separate from `_health` for backwards compatibility — `_health`'s
    ///   text/plain body is a feature.
    /// - `GET  /rpc/_procedures` → JSON array of registered procedure names.
    /// - `GET  /rpc/_ir` → full IR JSON, gated behind the `ir-export` feature.
    /// - `POST /rpc/<name>` for every registered query/mutation procedure.
    /// - `GET  /rpc/<name>?input=<urlencoded-json>` for every registered
    ///   subscription procedure, returning a `text/event-stream` body shaped
    ///   per SPEC §4.2 (`event: data | error | end`).
    /// - `GET  /rpc/_ws` (gated behind the `ws` feature) — a single
    ///   WebSocket that multiplexes subscriptions via [`crate::wire::WsMessage`].
    ///   Only subscription procedures are reachable over WS in v0.1; queries
    ///   and mutations remain on POST per SPEC §4.1.
    ///
    /// Unknown routes fall through to a SPEC §4.1 envelope:
    /// `404 {"err": {"code": "not_found", "payload": {"procedure": "<name>"}}}`.
    pub fn into_axum(mut self) -> AxumRouter {
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
        let names: Arc<Vec<String>> =
            Arc::new(self.procedures.iter().map(|p| p.name.to_string()).collect());

        let mut app = AxumRouter::new()
            .route("/rpc/_health", axum::routing::get(|| async { "ok" }))
            .route(
                "/rpc/_version",
                // SPEC §8: `_version` is intentionally separate from
                // `_health` so monitoring tools that scrape the latter as
                // text/plain don't see their parser change. `taut_rpc` is
                // pulled from `CARGO_PKG_VERSION` at compile time so it
                // tracks the crate version automatically; `ir_version`
                // mirrors the IR schema version emitted under `/rpc/_ir`.
                axum::routing::get(|| async {
                    axum::Json(serde_json::json!({
                        "taut_rpc": env!("CARGO_PKG_VERSION"),
                        "ir_version": crate::IR_VERSION,
                    }))
                }),
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

        // SPEC §4.2 WebSocket transport. Multiplexes subscriptions over a
        // single connection at `/rpc/_ws`; SSE remains available per-procedure
        // at `GET /rpc/<name>` and is the default. Cloning the descriptor vec
        // once into an `Arc` lets the WS handler share it across upgrades
        // without re-walking `self.procedures` (which we drain immediately
        // below to mount per-procedure routes). `ProcedureDescriptor` is
        // `Clone` (the body is an `Arc`-wrapped closure), so the clone is
        // cheap.
        #[cfg(feature = "ws")]
        {
            let descriptors_arc: Arc<Vec<ProcedureDescriptor>> = Arc::new(self.procedures.clone());
            app = app.route(
                "/rpc/_ws",
                axum::routing::get(crate::router::ws::ws_route::ws_handler(descriptors_arc)),
            );
        }

        for desc in std::mem::take(&mut self.procedures) {
            let path = format!("/rpc/{}", desc.name);
            match desc.kind {
                ProcKindRuntime::Query | ProcKindRuntime::Mutation => {
                    // Phase 3: dispatch through `ProcedureBody`. Query and
                    // mutation procedures are always `ProcedureBody::Unary` —
                    // the macro emission enforces this pairing. We
                    // destructure here so the Stream arm becomes a loud
                    // startup panic rather than a silent runtime fall-through
                    // if a malformed descriptor ever leaks past the macro.
                    let handler = match desc.body {
                        ProcedureBody::Unary(h) => h,
                        ProcedureBody::Stream(_) => {
                            unreachable!(
                                "taut-rpc: query/mutation `{}` was registered with a streaming body",
                                desc.name
                            )
                        }
                    };
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
                    // Symmetric to the unary branch: subscriptions must carry
                    // a `Stream` body. A `Unary` body here is a macro-side
                    // bug, so panic at startup with the offending name —
                    // bypassing this lazily would surface as a confusing
                    // 404 or 500 the first time the subscription is hit.
                    let handler = match desc.body {
                        ProcedureBody::Stream(h) => h,
                        ProcedureBody::Unary(_) => {
                            unreachable!(
                                "taut-rpc: subscription `{}` was registered with a unary body",
                                desc.name
                            )
                        }
                    };
                    app = app.route(
                        &path,
                        axum::routing::get(move |Query(params): Query<HashMap<String, String>>| {
                            let handler = handler.clone();
                            async move {
                                sse_response_for(handler, params.get("input").map(String::as_str))
                            }
                        }),
                    );
                }
            }
        }

        // Install the SPEC §4.1 not_found fallback before applying user layers
        // so middleware (logging, tracing, auth headers, ...) wraps the
        // fallback path too — clients hitting unknown procedures should see
        // the same observability as clients hitting real ones.
        let mut router = app.fallback(not_found_fallback);

        // Fold pending `tower::Layer`s in registration order. Because each
        // `axum::Router::layer` call wraps the entire router as the new
        // innermost service, applying layers in registration order means the
        // last `.layer()` call ends up outermost — matching axum's own
        // composition semantics, documented on `Router::layer` above.
        //
        // We `drain(..)` rather than iterate by reference so each closure can
        // move its captured layer out on its single invocation.
        for mut apply in self.layers.drain(..) {
            router = apply(router);
        }
        router
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
            let status =
                StatusCode::from_u16(http_status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let body = serde_json::json!({
                "err": { "code": code, "payload": payload }
            });
            (status, axum::Json(body)).into_response()
        }
    }
}

/// Decode the `?input=` query parameter for a subscription into a JSON value.
///
/// Per SPEC §4.2 the subscription input is URL-encoded JSON. Two flavors of
/// "missing" collapse to JSON `null`:
///
/// - the parameter is absent entirely (`/rpc/<name>` with no query string);
/// - the parameter is present but empty (`/rpc/<name>?input=`).
///
/// Both are treated as `null` so unit-input subscriptions ergonomically
/// dispatch on `?input=` or no query at all. Any other value must be valid
/// JSON; on `serde_json::Error` we surface the error to the caller so the
/// SSE handler can emit an in-band `decode_error` frame instead of an HTTP
/// 4xx — the connection is event-stream-framed by the time the handler
/// produces a body, so structured errors must ride the same channel.
fn parse_input_param(raw: Option<&str>) -> Result<serde_json::Value, serde_json::Error> {
    match raw {
        None | Some("") => Ok(serde_json::Value::Null),
        Some(s) => serde_json::from_str(s),
    }
}

/// Build the SSE response body for a subscription procedure (SPEC §4.2).
///
/// Two paths feed into one event stream:
///
/// 1. **Decode failure** of the `?input=` parameter: emit a single
///    `event: error\ndata: {"code":"decode_error",...}\n\n` frame followed by
///    `event: end`. Per SPEC §4.2 a malformed subscription input does not
///    produce a 400 HTTP status — by the time the client is reading the body
///    it expects event-stream framing, so we surface decode failures as an
///    in-band error frame whose envelope mirrors the unary `decode_error`
///    shape (clients can share a parser between transports).
///
/// 2. **Successful decode**: invoke the [`crate::procedure::StreamHandler`],
///    map each emitted [`StreamFrame`] to the corresponding SSE event, then
///    append a closing `event: end` frame after the user stream completes.
///    The `end` frame is generated here, not by the handler — handlers
///    signal completion by terminating their `BoxStream`.
///
/// `Sse::new` requires an item type of `Result<Event, E>` where `E:
/// std::error::Error`; we use `Infallible` because every event we produce is
/// hand-shaped from already-deserialized `serde_json::Value`s and a
/// `json!({...})` literal, neither of which can fail JSON serialization.
#[allow(clippy::needless_pass_by_value)] // handler is `Arc<…>`; moving avoids a clone at the call site
fn sse_response_for(
    handler: crate::procedure::StreamHandler,
    raw_input: Option<&str>,
) -> Sse<
    KeepAliveStream<futures::stream::BoxStream<'static, Result<Event, std::convert::Infallible>>>,
> {
    use futures::stream;

    // Both arms produce *different* concrete stream types, so erase to a
    // `BoxStream` to give the function a single return type. The cost is
    // one allocation per request, negligible next to the SSE socket
    // lifetime.
    let event_stream: futures::stream::BoxStream<'static, Result<Event, std::convert::Infallible>> =
        match parse_input_param(raw_input) {
            Err(e) => {
                // Synthesize a single `event: error` frame describing the decode
                // failure. The payload mirrors the unary `decode_error` envelope
                // shape so clients can share a parser between transports.
                let event = Event::default()
                    .event("error")
                    .json_data(serde_json::json!({
                        "code": "decode_error",
                        "payload": { "message": e.to_string() },
                    }))
                    .expect("valid json");
                stream::once(async move { Ok(event) }).boxed()
            }
            Ok(input_json) => {
                // Map each handler frame to an SSE event. `Event::json_data`
                // only fails when its argument isn't serializable to JSON — but
                // we feed it already-deserialized `serde_json::Value`s and a
                // hand-rolled `json!({...})` literal, both of which are
                // guaranteed JSON. The `expect`s therefore can never fire at
                // runtime.
                let frames = handler(input_json);
                frames
                    .map(|frame| {
                        let event = match frame {
                            StreamFrame::Data(v) => Event::default()
                                .event("data")
                                .json_data(v)
                                .expect("valid json"),
                            StreamFrame::Error { code, payload } => Event::default()
                                .event("error")
                                .json_data(serde_json::json!({
                                    "code": code,
                                    "payload": payload,
                                }))
                                .expect("valid json"),
                        };
                        Ok::<Event, std::convert::Infallible>(event)
                    })
                    .boxed()
            }
        };

    // SPEC §4.2 mandates a trailing `event: end` frame; the router emits it
    // (handlers just terminate their stream). We append it via `chain` so
    // every code path — decode-error, normal completion, empty stream —
    // closes with the same frame.
    let end = stream::once(async {
        Ok::<Event, std::convert::Infallible>(Event::default().event("end").data(""))
    });

    // axum 0.8's `Sse::keep_alive` wraps the inner stream in `KeepAliveStream`,
    // changing the response's static type from `Sse<S>` to `Sse<KeepAliveStream<S>>`.
    // Reflecting that in the return signature lets us keep emitting keep-alive
    // comments on idle, which is required for HTTP-aware proxies (e.g. nginx's
    // default 60s idle timeout) that would otherwise cut the SSE connection.
    Sse::new(event_stream.chain(end).boxed()).keep_alive(KeepAlive::default())
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
    use crate::ir::{HttpMethod, Primitive, ProcKind, Procedure, TypeRef};
    use crate::procedure::{StreamHandler, UnaryHandler};
    use axum::body::Body;
    use futures::future::BoxFuture;
    use futures::stream::{self, BoxStream};
    use http::Request as HttpRequest;
    use tower::ServiceExt;

    fn make_descriptor(
        name: &'static str,
        kind: ProcKindRuntime,
        handler: UnaryHandler,
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
            // Phase 3: the descriptor's body is now an enum; the existing
            // router tests all exercise unary dispatch so we wrap in
            // `ProcedureBody::Unary` here. Streaming-side tests live in
            // `procedure.rs` for now; Agent 2 will add HTTP-level streaming
            // tests when the SSE route lands.
            body: ProcedureBody::Unary(handler),
        }
    }

    fn echo_handler() -> UnaryHandler {
        Arc::new(
            |input: serde_json::Value| -> BoxFuture<'static, ProcedureResult> {
                Box::pin(async move { ProcedureResult::Ok(input) })
            },
        )
    }

    fn not_found_handler() -> UnaryHandler {
        Arc::new(
            |_input: serde_json::Value| -> BoxFuture<'static, ProcedureResult> {
                Box::pin(async move {
                    ProcedureResult::Err {
                        http_status: 404,
                        code: "not_found".to_string(),
                        payload: serde_json::Value::Null,
                    }
                })
            },
        )
    }

    /// Build a [`ProcedureDescriptor`] wrapping a streaming body.
    ///
    /// Mirrors `make_descriptor` for the streaming side: same IR shape, but
    /// `kind = ProcKindRuntime::Subscription` and `body =
    /// ProcedureBody::Stream(handler)`. Used by the SPEC §4.2 SSE tests
    /// below.
    fn make_stream_descriptor(name: &'static str, handler: StreamHandler) -> ProcedureDescriptor {
        ProcedureDescriptor {
            name,
            kind: ProcKindRuntime::Subscription,
            ir: Procedure {
                name: name.to_string(),
                kind: ProcKind::Subscription,
                input: TypeRef::Primitive(Primitive::Unit),
                output: TypeRef::Primitive(Primitive::Unit),
                errors: vec![],
                http_method: HttpMethod::Get,
                doc: None,
            },
            type_defs: vec![],
            body: ProcedureBody::Stream(handler),
        }
    }

    /// Build a [`StreamHandler`] that yields `n` `StreamFrame::Data(json!(i))`
    /// items for `i` in `0..n`. Lets the subscription tests assert on exact
    /// frame contents without baking the stream construction into each test.
    fn counting_stream_handler(n: usize) -> StreamHandler {
        Arc::new(
            move |_input: serde_json::Value| -> BoxStream<'static, StreamFrame> {
                stream::iter((0..n).map(|i| StreamFrame::Data(serde_json::json!(i)))).boxed()
            },
        )
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
    async fn health_endpoint_unchanged() {
        // `_version` was added alongside `_health` in Phase 5 — pin the
        // contract that `_health` itself didn't change. Per SPEC §8 its
        // text/plain `ok` body is a feature; monitoring tools that scrape
        // it should continue to see the same wire shape after the
        // `_version` route lands. Mirrors `health_endpoint_returns_ok`
        // above; the duplication is deliberate so this guarantee survives
        // independent edits to either test.
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
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.starts_with("text/plain"),
            "expected text/plain content-type, got {ct:?}"
        );
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], b"ok");
    }

    #[tokio::test]
    async fn version_endpoint_returns_json_with_version_and_ir_version() {
        // `_version` returns a JSON object with both `taut_rpc` (the crate
        // version, pulled from `CARGO_PKG_VERSION` at compile time) and
        // `ir_version` (mirroring `crate::IR_VERSION`). We assert structure
        // rather than exact strings so this test doesn't churn every time
        // we bump the crate version — the contract is "two fields, right
        // types, ir_version matches the constant", not "the crate version
        // is literally 0.1.0".
        let app = Router::new().into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/rpc/_version")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("body must parse as JSON");
        assert!(
            v["taut_rpc"].is_string(),
            "taut_rpc field must be a string; got {v}"
        );
        assert_eq!(
            v["taut_rpc"].as_str().unwrap(),
            env!("CARGO_PKG_VERSION"),
            "taut_rpc must match CARGO_PKG_VERSION",
        );
        assert_eq!(
            v["ir_version"].as_u64(),
            Some(u64::from(crate::IR_VERSION)),
            "ir_version must match crate::IR_VERSION",
        );
    }

    #[tokio::test]
    async fn procedures_endpoint_lists_registered_names() {
        let app = Router::new()
            .procedure(make_descriptor(
                "alpha",
                ProcKindRuntime::Query,
                echo_handler(),
            ))
            .procedure(make_descriptor(
                "beta",
                ProcKindRuntime::Mutation,
                echo_handler(),
            ))
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
            .procedure(make_descriptor(
                "dup",
                ProcKindRuntime::Query,
                echo_handler(),
            ))
            .procedure(make_descriptor(
                "dup",
                ProcKindRuntime::Query,
                echo_handler(),
            ));
    }

    #[test]
    fn ir_snapshot_contains_registered_procedures() {
        let router = Router::new()
            .procedure(make_descriptor("a", ProcKindRuntime::Query, echo_handler()))
            .procedure(make_descriptor(
                "b",
                ProcKindRuntime::Mutation,
                echo_handler(),
            ));

        let ir = router.ir();
        assert_eq!(ir.ir_version, crate::ir::Ir::CURRENT_VERSION);
        let names: Vec<&str> = ir.procedures.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    // ---- tower::Layer integration (Phase 2 — SPEC §5) ------------------

    /// Apply a marker-stamping middleware to a [`Router`] under test.
    ///
    /// Stamps `x-taut-test: <marker>` on the response so the layer tests
    /// below can prove (a) the layer was invoked at all, and (b) which layer
    /// won when several stamp the same header. We pass `marker` as a `'static
    /// str` to dodge naming the closure-and-future types that `from_fn`
    /// bakes into its `FromFnLayer<...>` return type — those types are
    /// unnameable in stable Rust without `impl Trait` in type aliases.
    fn with_marker_layer(router: Router, marker: &'static str) -> Router {
        router.layer(axum::middleware::from_fn(
            move |req: axum::extract::Request, next: axum::middleware::Next| async move {
                let mut resp = next.run(req).await;
                resp.headers_mut()
                    .insert("x-taut-test", marker.parse().unwrap());
                resp
            },
        ))
    }

    #[tokio::test]
    async fn router_with_no_layers_builds_unchanged() {
        // Smoke: a router with zero `.layer()` calls still produces a working
        // axum router — the layer machinery shouldn't perturb the no-middleware
        // path. We hit `/rpc/_health` because it's the cheapest live route.
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
        // No layers were registered, so no middleware-stamped headers should
        // appear. This is what "unchanged" means in the test name.
        assert!(response.headers().get("x-taut-test").is_none());
    }

    #[tokio::test]
    async fn router_with_a_simple_layer_applies_it() {
        // Exercises the happy path: register one procedure, wrap with one
        // middleware that stamps a header, confirm the header survives the
        // round trip. Proves `.layer()` actually plumbs through to the built
        // axum::Router rather than being silently dropped.
        let router = Router::new().procedure(make_descriptor(
            "ping",
            ProcKindRuntime::Query,
            echo_handler(),
        ));
        let app = with_marker_layer(router, "hit").into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/rpc/ping")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"input":null}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-taut-test")
                .map(|v| v.to_str().unwrap()),
            Some("hit"),
        );
    }

    #[tokio::test]
    async fn multiple_layers_compose_in_outer_first_order() {
        // axum's `Router::layer` makes the *last* applied layer the outermost
        // wrap, so its post-handler logic runs *last* on the response path.
        // Two layers both setting the same response header therefore lets the
        // outer layer overwrite the inner one — and that "winner" tells us
        // which layer ran outermost.
        //
        // Registration:   .layer(inner).layer(outer)
        // Onion:          outer( inner( handler ) )
        // Header writes:  inner first, outer last  → outer wins.
        let router = Router::new().procedure(make_descriptor(
            "ping",
            ProcKindRuntime::Query,
            echo_handler(),
        ));
        let router = with_marker_layer(router, "inner");
        let router = with_marker_layer(router, "outer");
        let app = router.into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("POST")
                    .uri("/rpc/ping")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"input":null}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("x-taut-test")
                .map(|v| v.to_str().unwrap()),
            // The last `.layer()` call wins — that's the SPEC §5 contract,
            // matching `axum::Router::layer` and `tower`'s own ordering.
            Some("outer"),
        );
    }

    // ---- Subscription dispatch (Phase 3 — SPEC §4.2) ------------------

    #[tokio::test]
    async fn subscription_route_emits_three_data_frames_then_end() {
        // Register a subscription whose handler yields three Data frames,
        // hit the SSE route, and verify the body contains the three frames
        // in order followed by the closing `event: end` frame the router
        // appends. Keepalive comments aren't asserted on here — they're
        // emitted on a timer and don't appear during the brief synchronous
        // lifetime of `oneshot`, which keeps this test deterministic.
        let app = Router::new()
            .procedure(make_stream_descriptor("ticks", counting_stream_handler(3)))
            .into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/rpc/ticks?input=null")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        // Content-Type must be `text/event-stream`; axum's `Sse` sets this
        // automatically. Pinning it here pins the SPEC §4.2 transport.
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.starts_with("text/event-stream"),
            "expected text/event-stream content-type, got {ct:?}"
        );

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = std::str::from_utf8(&bytes).expect("utf8 body");

        // Each Data frame should appear in order, followed by `event: end`.
        // We string-match the SSE wire bytes rather than parse — the framing
        // is small and stable per SPEC §4.2, and a structural parse would
        // only re-derive these same assertions.
        for i in 0..3 {
            let needle = format!("event: data\ndata: {i}\n\n");
            assert!(
                body.contains(&needle),
                "missing data frame {i}; body was:\n{body}"
            );
        }
        assert!(
            body.contains("event: end"),
            "missing terminating event:end frame; body was:\n{body}"
        );

        // Order check: `event: end` must come after the last data frame.
        // Without this we could be matching an interleaved stream and miss
        // a bug where the router emits `end` before the handler stream
        // drains.
        let last_data_idx = body.rfind("event: data").expect("at least one data frame");
        let end_idx = body.find("event: end").expect("end frame present");
        assert!(
            end_idx > last_data_idx,
            "event:end must follow the last data frame; body was:\n{body}"
        );
    }

    #[tokio::test]
    async fn subscription_decode_error_emits_error_frame_then_end() {
        // Per SPEC §4.2 a malformed `?input=` doesn't get a 400 status — by
        // the time the body is being read the response is already framed
        // text/event-stream. We synthesize a single in-band error frame
        // followed by `event: end`. This pins both pieces of that contract.
        let app = Router::new()
            .procedure(make_stream_descriptor(
                "ticks",
                // The handler is never invoked when input fails to decode,
                // so any stream handler suffices here.
                counting_stream_handler(0),
            ))
            .into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/rpc/ticks?input=not-json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = std::str::from_utf8(&bytes).expect("utf8 body");

        // The error frame must (a) be tagged `event: error`, (b) carry a
        // JSON body whose `code` is `decode_error`, and (c) precede
        // `event: end`. We assert structurally on the JSON instead of
        // pinning the exact serde_json error message — that text drifts
        // between serde_json releases.
        assert!(
            body.contains("event: error"),
            "missing event:error frame; body was:\n{body}"
        );

        // Find the data line that lives under the `event: error` block and
        // parse it as JSON. A `split_once`-then-`lines` walk is simpler
        // than a full SSE parser for one frame.
        let error_section = body
            .split("event: error\n")
            .nth(1)
            .expect("error event present");
        let data_line = error_section
            .lines()
            .find(|l| l.starts_with("data: "))
            .expect("data line under error event");
        let json_str = data_line.strip_prefix("data: ").unwrap();
        let v: serde_json::Value =
            serde_json::from_str(json_str).expect("valid json in error frame");
        assert_eq!(v["code"], serde_json::json!("decode_error"));
        assert!(
            v["payload"]["message"].is_string(),
            "payload.message should be a string; got {v}"
        );

        let error_idx = body.find("event: error").unwrap();
        let end_idx = body
            .find("event: end")
            .expect("missing event:end after event:error");
        assert!(
            end_idx > error_idx,
            "event:end must follow event:error; body was:\n{body}"
        );
    }

    #[tokio::test]
    async fn subscription_with_no_input_param_decodes_as_null() {
        // SPEC §4.2 doesn't require `?input=` — a unit-input subscription
        // should be reachable as bare `/rpc/<name>`. The router decodes a
        // missing param to JSON `null`, so the handler runs normally and
        // emits its frames (here: a single Data(0)) plus the closing
        // `event: end`. Without this contract every unit-input subscription
        // would need a redundant `?input=null` on its URL.
        let app = Router::new()
            .procedure(make_stream_descriptor("ticks", counting_stream_handler(1)))
            .into_axum();

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/rpc/ticks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = std::str::from_utf8(&bytes).expect("utf8 body");

        assert!(
            body.contains("event: data\ndata: 0\n\n"),
            "expected single data frame for null input; body was:\n{body}"
        );
        assert!(
            body.contains("event: end"),
            "missing event:end; body was:\n{body}"
        );
    }
}

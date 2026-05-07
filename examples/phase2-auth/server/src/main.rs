//! Phase 2 example server — `tower::Layer` + `#[derive(TautError)]` end-to-end.
//!
//! Demonstrates two complementary failure modes a production app needs:
//!
//! 1. **Layer-level rejection.** An `axum::middleware::from_fn` runs *before*
//!    the procedure handler, inspects `Authorization: Bearer <token>`, and
//!    short-circuits unauthenticated requests to protected procedures with the
//!    SPEC §4.1 envelope `{"err":{"code":"unauthenticated","payload":null}}`
//!    and HTTP 401. The procedure handler never runs.
//! 2. **Procedure-level typed error.** A protected procedure that *is* reached
//!    can still fail with a domain-specific `AuthError::Forbidden { ... }`
//!    that flows through `#[derive(TautError)]` and serialises with the same
//!    SPEC envelope, but with HTTP 403 and a structured payload.
//!
//! Both shapes are caught on the TS side via the same `TautError` runtime
//! type, narrowed on `e.code` — `"unauthenticated"` vs `"forbidden"`.
//!
//! ## State plumbing is out-of-scope here
//!
//! Phase 2 does NOT add `State<S>` extractor support to `#[rpc]`; that lands
//! with Phase 3. The "current user" is therefore faked at the layer level —
//! the layer makes the access-control decision and short-circuits, while
//! procedures themselves return canned responses. The point of the example is
//! to show the **layer + typed error** pipeline works, not full state plumbing.

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use taut_rpc::{dump_if_requested, rpc, Router, TautError, Type};
use tower_http::cors::CorsLayer;

// --- Domain types -----------------------------------------------------------

#[derive(Serialize, Deserialize, Type, Clone, Debug)]
pub struct User {
    pub id: u64,
    pub role: String,
}

/// Procedure-level errors. The wire shape is driven by serde
/// (`tag = "code", content = "payload"`), and the `TautError` derive supplies
/// `code()` + `http_status()` per variant — see SPEC §3.3.
#[derive(Serialize, Type, TautError, thiserror::Error, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum AuthError {
    #[taut(status = 401)]
    #[error("unauthenticated")]
    Unauthenticated,
    #[taut(status = 403)]
    #[error("forbidden: {required_role}")]
    Forbidden { required_role: String },
}

// --- Procedures -------------------------------------------------------------
//
// Phase 4 forward-compatibility: Phase 4 makes `#[rpc]` emit a `Validate` call
// for every non-unit input. All three procedures below take zero args, so the
// macro emits no validation calls and no `Validate` derive is required on any
// type in this file. `User` is returned (not input) and `AuthError` is an
// error type — neither needs `Validate`. No constraint changes are needed for
// this example to keep compiling under Phase 4.

/// Public health-check style query. The auth layer lets requests to this path
/// through unconditionally.
#[rpc]
async fn ping() -> &'static str {
    "pong"
}

/// Gated by the auth layer: unauthenticated callers never reach this fn.
/// In a real app this would consult the request extensions for the user the
/// layer attached; see the module-level note on Phase 2 state plumbing.
#[rpc]
async fn whoami() -> Result<User, AuthError> {
    // The layer has already guaranteed the caller is authenticated.
    // For this demo we return a stub user — Phase 3 will plumb the real one.
    Ok(User {
        id: 1,
        role: "user".to_string(),
    })
}

/// Gated by the auth layer for `unauthenticated`, AND short-circuited by the
/// layer for non-admin callers. The `Forbidden` variant is therefore produced
/// at the layer in this demo — the procedure body returns the secret on the
/// happy path. See SPEC §3.3 + §5: `tower::Layer` does access control,
/// `TautError` provides the typed wire shape on the way back out.
#[rpc]
async fn get_secret() -> Result<String, AuthError> {
    Ok("the cake is a lie".to_string())
}

// --- Auth layer -------------------------------------------------------------

/// Resolve a bearer token to a user, hardcoded for the demo.
///
/// `"alpha"` → a regular user, `"admin"` → an admin, anything else → no user.
/// In production this would hit a session store / JWT verifier / etc.
fn authenticate(token: &str) -> Option<User> {
    match token {
        "alpha" => Some(User {
            id: 1,
            role: "user".to_string(),
        }),
        "admin" => Some(User {
            id: 2,
            role: "admin".to_string(),
        }),
        _ => None,
    }
}

/// Build the SPEC §4.1 error envelope `{"err":{"code":..., "payload":...}}`
/// as a fully-rendered axum response with the given status.
fn envelope(status: StatusCode, code: &str, payload: serde_json::Value) -> Response {
    let body = serde_json::json!({
        "err": { "code": code, "payload": payload },
    });
    (status, axum::Json(body)).into_response()
}

/// Auth middleware. Inspects `Authorization: Bearer <token>` and:
///
/// - Lets `/rpc/_health`, `/rpc/_procedures`, `/rpc/_ir`, and `/rpc/ping`
///   through without authentication (public surface).
/// - Requires a valid token for `/rpc/whoami` and `/rpc/get_secret`. Missing
///   or unrecognized tokens short-circuit with 401 + `unauthenticated`
///   envelope, matching SPEC §3.3 / §4.1.
/// - For `/rpc/get_secret`, additionally requires the resolved user's role
///   to be `"admin"`. Non-admins short-circuit with 403 + `forbidden`
///   envelope (`payload = { "required_role": "admin" }`).
/// - Inserts the resolved `User` into request extensions for downstream
///   handlers — Phase 3 will let `#[rpc]` extract it directly.
async fn auth_layer(mut request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();

    // Public bypass: built-in debug routes plus the `ping` procedure.
    let is_public = path == "/rpc/_health"
        || path == "/rpc/_procedures"
        || path == "/rpc/_ir"
        || path == "/rpc/ping";

    if is_public {
        return next.run(request).await;
    }

    // Only `/rpc/whoami` and `/rpc/get_secret` are gated in this demo. Any
    // other path falls through to the router's own `not_found` envelope.
    let is_protected = path == "/rpc/whoami" || path == "/rpc/get_secret";
    if !is_protected {
        return next.run(request).await;
    }

    // Extract the bearer token.
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim);

    let user = match token.and_then(authenticate) {
        Some(u) => u,
        None => {
            return envelope(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                serde_json::Value::Null,
            );
        }
    };

    // Per-procedure role check. The procedure body is untouched — the layer
    // makes the policy decision and the procedure stays declarative.
    if path == "/rpc/get_secret" && user.role != "admin" {
        return envelope(
            StatusCode::FORBIDDEN,
            "forbidden",
            serde_json::json!({ "required_role": "admin" }),
        );
    }

    // Attach the user so future Phase 3 handlers can read it. Phase 2 doesn't
    // wire `#[rpc]` to extensions yet; the insert is here for forward-compat.
    request.extensions_mut().insert(user);
    next.run(request).await
}

// --- main -------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let router = Router::new()
        .procedure(__taut_proc_ping())
        .procedure(__taut_proc_whoami())
        .procedure(__taut_proc_get_secret());

    // If `TAUT_DUMP_IR` is set, write the IR and exit before binding any port.
    // `cargo taut gen --from-binary` relies on this to extract the IR without
    // needing a working network or filesystem outside the IR target path.
    dump_if_requested(&router);

    let app = router
        .into_axum()
        // Order matters: the auth layer must wrap the procedure routes. We
        // apply CORS *outside* auth so preflight `OPTIONS` requests don't get
        // bounced by the auth check — `tower-http`'s permissive CORS handles
        // them before auth ever sees them.
        .layer(axum::middleware::from_fn(auth_layer))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:7702")
        .await
        .expect("bind 0.0.0.0:7702");

    println!("phase2-auth-server listening on http://127.0.0.1:7702");

    axum::serve(listener, app).await.expect("server crashed");
}

//! Phase 4 example server — `#[derive(Validate)]` + Valibot bridge end-to-end.
//!
//! Demonstrates the validation contract from SPEC §7: per-field constraints
//! attached to an input type via `#[taut(...)]` attributes are recorded into
//! the IR, lowered to a Valibot schema by codegen, and enforced *both*
//! client-side (before the request leaves the browser/Node) and server-side
//! (before the procedure body runs).
//!
//! The headline procedure is `create_user(CreateUser) -> Result<User,
//! CreateUserError>`. The input type uses every constraint kind v0.1 supports
//! — `length`, `email`, `min`/`max`, `pattern`, `url` — so the generated
//! schema exercises the full lowering surface.
//!
//! `CreateUserError` is a serde-tagged enum that derives `taut_rpc::TautError`
//! for the Phase 2 error contract: it carries a single `UsernameTaken` variant
//! so the example can demonstrate the three-way distinction between client-
//! side validation rejection, server-side validation rejection, and
//! application-layer rejection — all of which surface to the TS caller as a
//! `TautError` distinguishable by `.code`.

use serde::{Deserialize, Serialize};
use taut_rpc::{dump_if_requested, rpc, Router, Type, Validate};
use tower_http::cors::CorsLayer;

// --- CreateUser --------------------------------------------------------------

/// Input for `create_user`. Every field carries at least one Phase 4
/// constraint so the codegen lowering exercises all five v0.1 forms.
///
/// Constraints emit into the IR as `field -> Vec<Constraint>` and are lowered
/// in `taut-rpc-cli` to Valibot pipe stages (`v.string([v.minLength(3),
/// v.maxLength(32)])`, etc). Server-side, the same constraints drive the
/// `Validate::validate` impl; if any field fails, the macro-generated handler
/// short-circuits to the standard `validation_error` envelope before the
/// procedure body runs.
#[derive(Serialize, Deserialize, Type, Validate)]
pub struct CreateUser {
    /// 3..=32 characters. The classic "username" length window.
    #[taut(length(min = 3, max = 32))]
    pub username: String,
    /// Plausibly an email; the regex Valibot uses is the same one Rust's
    /// `Validate` impl checks against.
    #[taut(email)]
    pub email: String,
    /// 18..=120; demonstrates the `min`/`max` numeric form on a `u8`.
    #[taut(min = 18, max = 120)]
    pub age: u8,
    /// Lowercase ASCII alnum + underscore; demonstrates the `pattern`
    /// arbitrary-regex form. The leading `^` and trailing `$` are required —
    /// Valibot's `regex` is unanchored by default and we want a full-string
    /// match.
    #[taut(pattern = "^[a-z0-9_]+$")]
    pub handle: String,
    /// A well-formed URL; demonstrates the `url` form. The Valibot lowering
    /// uses `v.url()`, which accepts anything `new URL(...)` accepts.
    #[taut(url)]
    pub homepage: String,
}

// --- User --------------------------------------------------------------------

/// Output of `create_user`. Just enough fields to confirm the round-trip;
/// no validation needed on output types in v0.1 (the client-side schema
/// covers the recv path).
#[derive(Serialize, Deserialize, Type)]
pub struct User {
    /// `u32` rather than `u64` to keep the demo simple — the codegen emits
    /// `v.bigint()` for `u64` outputs, but the wire JSON ships them as plain
    /// JS numbers, so validating on `bigint` fails on the recv path. v0.2
    /// will add a coercion adapter; until then `u32` skirts the issue.
    pub id: u32,
    pub username: String,
}

// --- CreateUserError ---------------------------------------------------------

/// Application-layer error for `create_user`. Distinct from validation
/// errors: this fires *after* the input passes `Validate` but the business
/// logic still says no. The client demonstrates narrowing on
/// `isTautError(e, "username_taken")` to distinguish it from the
/// `validation_error` envelope.
#[derive(Serialize, taut_rpc::Type, taut_rpc::TautError, thiserror::Error, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum CreateUserError {
    #[error("username taken")]
    UsernameTaken,
}

// --- procedures --------------------------------------------------------------

/// The headline mutation. Validation runs *before* this body executes — by
/// the time we get here, every field of `input` has already passed every
/// `#[taut(...)]` constraint. Anything we reject here is a true
/// application-layer rejection, not a malformed-input rejection.
#[rpc(mutation)]
async fn create_user(input: CreateUser) -> Result<User, CreateUserError> {
    if input.username == "taken" {
        return Err(CreateUserError::UsernameTaken);
    }
    Ok(User {
        id: 1,
        username: input.username,
    })
}

/// Sanity-check unary query. No input, no validation; confirms the
/// validation pipeline didn't regress the unconstrained path.
#[rpc]
async fn ping() -> &'static str {
    "pong"
}

// --- main --------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let router = Router::new()
        .procedure(__taut_proc_ping())
        .procedure(__taut_proc_create_user());

    // If `TAUT_DUMP_IR` is set, write the IR and exit before binding any port.
    // `cargo taut gen --from-binary` relies on this to extract the IR without
    // needing a working network or filesystem outside the IR target path.
    dump_if_requested(&router);

    let app = router.into_axum().layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:7705")
        .await
        .expect("bind 0.0.0.0:7705");

    println!("phase4-validate-server listening on http://127.0.0.1:7705");

    axum::serve(listener, app).await.expect("server crashed");
}

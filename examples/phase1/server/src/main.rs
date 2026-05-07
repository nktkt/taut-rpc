//! Phase 1 example server.
//!
//! Demonstrates the full taut-rpc Phase 1 pipeline: `#[rpc]` on async fns,
//! `#[derive(Type)]` on the input/output/error types, the `Router` wiring, and
//! `dump_if_requested` so `cargo taut gen --from-binary` can extract the IR.
//!
//! Per SPEC §3.3, errors are tagged JSON with a `code` discriminant and a
//! `payload` for variant-specific data. We use serde's `tag = "code", content
//! = "payload"` so the wire shape matches the envelope `taut-rpc` emits in
//! the `err` field on a 4xx/5xx response.

use serde::{Deserialize, Serialize};
use taut_rpc::{dump_if_requested, rpc, Router, Type};
use tower_http::cors::CorsLayer;

// --- ping --------------------------------------------------------------------

/// Health-check style query that takes no input and always succeeds.
#[rpc]
async fn ping() -> String {
    "pong".to_string()
}

// --- add ---------------------------------------------------------------------

#[derive(Serialize, Deserialize, Type, taut_rpc::Validate)]
pub struct AddInput {
    pub a: i32,
    pub b: i32,
}

#[derive(Serialize, Deserialize, Type, taut_rpc::TautError, thiserror::Error, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum AddError {
    /// `a + b` overflowed `i32`.
    #[error("overflow")]
    Overflow,
}

/// Adds two `i32`s, surfacing overflow as a typed error rather than a panic.
#[rpc]
async fn add(input: AddInput) -> Result<i32, AddError> {
    input.a.checked_add(input.b).ok_or(AddError::Overflow)
}

// --- get_user ----------------------------------------------------------------

#[derive(Serialize, Deserialize, Type, taut_rpc::Validate)]
pub struct GetUserInput {
    pub id: u64,
}

#[derive(Serialize, Deserialize, Type)]
pub struct User {
    pub id: u64,
    pub name: String,
}

#[derive(Serialize, Deserialize, Type, taut_rpc::TautError, thiserror::Error, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum GetUserError {
    /// No user exists with this id.
    #[error("user {id} not found")]
    NotFound { id: u64 },
}

/// Fetches a user by id; returns `NotFound { id }` for any id other than 1.
#[rpc]
async fn get_user(input: GetUserInput) -> Result<User, GetUserError> {
    if input.id == 1 {
        Ok(User {
            id: 1,
            name: "ada".to_string(),
        })
    } else {
        Err(GetUserError::NotFound { id: input.id })
    }
}

// --- get_status --------------------------------------------------------------

/// Demonstrates an enum return covering both unit variants and a struct
/// variant. The discriminant is `tag = "type"` (the SPEC §3.2 default), so the
/// wire shape is e.g. `{ "type": "online" }` or `{ "type": "away",
/// "since_ms": 1234 }` (struct variants inline their fields).
#[derive(Serialize, Deserialize, Type)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Status {
    Online,
    Offline,
    Away { since_ms: u64 },
}

#[rpc]
async fn get_status() -> Status {
    Status::Away { since_ms: 1234 }
}

// --- main --------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let router = Router::new()
        .procedure(__taut_proc_ping())
        .procedure(__taut_proc_add())
        .procedure(__taut_proc_get_user())
        .procedure(__taut_proc_get_status());

    // If `TAUT_DUMP_IR` is set, write the IR and exit before binding any port.
    // `cargo taut gen --from-binary` relies on this to extract the IR without
    // needing a working network or filesystem outside the IR target path.
    dump_if_requested(&router);

    let app = router.into_axum().layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:7701")
        .await
        .expect("bind 0.0.0.0:7701");

    println!("phase1-server listening on http://127.0.0.1:7701");

    axum::serve(listener, app).await.expect("server crashed");
}

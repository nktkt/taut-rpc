//! Phase 0 hand-written smoke server.
//!
//! No macros, no codegen — every route is written longhand to validate the
//! taut-rpc wire format from `SPEC.md` §4. Any future `#[rpc]` macro must
//! emit code shaped like the handlers below.

use std::{convert::Infallible, time::Duration};

use axum::{
    Json, Router,
    http::StatusCode,
    response::{
        IntoResponse,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

// --- Wire envelopes (SPEC §4.1) --------------------------------------------

#[derive(Deserialize)]
struct Input<T> {
    input: T,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Envelope<T> {
    Ok { ok: T },
    Err { err: ApiError },
}

#[derive(Serialize)]
struct ApiError {
    code: &'static str,
    payload: serde_json::Value,
}

impl<T: Serialize> Envelope<T> {
    fn ok(value: T) -> Self {
        Envelope::Ok { ok: value }
    }

    fn err(code: &'static str) -> Self {
        Envelope::Err {
            err: ApiError {
                code,
                payload: serde_json::Value::Null,
            },
        }
    }
}

// `#[serde(untagged)]` plus two variants with distinct named fields makes
// `Envelope::Ok { .. }` serialize to `{"ok": ...}` and `Envelope::Err { .. }`
// to `{"err": ...}`, matching SPEC §4.1 exactly.

// --- Procedure types -------------------------------------------------------
//
// In real taut-rpc, these would be derived/inferred. Here we write them by
// hand — newtypes around primitives serialize transparently with serde, so
// `Pong("pong")` encodes as `"pong"` and `AddOutput(5)` as `5`.

#[derive(Serialize)]
#[serde(transparent)]
struct Pong(&'static str);

#[derive(Deserialize)]
struct AddInput {
    a: i32,
    b: i32,
}

#[derive(Serialize)]
#[serde(transparent)]
struct AddOutput(i32);

#[derive(Deserialize)]
struct GetUserInput {
    id: u64,
}

#[derive(Serialize)]
struct User {
    id: u64,
    name: &'static str,
}

// --- Handlers --------------------------------------------------------------

async fn ping() -> Json<Envelope<Pong>> {
    Json(Envelope::ok(Pong("pong")))
}

async fn add(Json(body): Json<Input<AddInput>>) -> impl IntoResponse {
    match body.input.a.checked_add(body.input.b) {
        Some(sum) => (
            StatusCode::OK,
            Json(Envelope::<AddOutput>::ok(AddOutput(sum))),
        )
            .into_response(),
        None => (
            StatusCode::BAD_REQUEST,
            Json(Envelope::<AddOutput>::err("overflow")),
        )
            .into_response(),
    }
}

async fn get_user(Json(body): Json<Input<GetUserInput>>) -> impl IntoResponse {
    if body.input.id == 1 {
        let user = User { id: 1, name: "ada" };
        (StatusCode::OK, Json(Envelope::ok(user))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(Envelope::<User>::err("not_found")),
        )
            .into_response()
    }
}

async fn tick() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        for i in 0..5_i32 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            yield Ok(Event::default().event("data").data(i.to_string()));
        }
        yield Ok(Event::default().event("end").data(""));
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn health() -> &'static str {
    "ok"
}

// --- main ------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/rpc/ping", post(ping))
        .route("/rpc/add", post(add))
        .route("/rpc/get_user", post(get_user))
        .route("/rpc/tick", get(tick))
        .route("/rpc/_health", get(health))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:7700")
        .await
        .expect("bind 0.0.0.0:7700");

    println!("smoke-server listening on http://127.0.0.1:7700");

    axum::serve(listener, app).await.expect("server crashed");
}

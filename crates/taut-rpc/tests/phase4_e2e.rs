//! Phase 4 end-to-end tests for taut-rpc — server-side input validation.
//!
//! These tests pin the macro -> runtime -> wire pipeline against the SPEC §7
//! validation contract:
//!
//! - The `#[rpc]` macro injects a `<Input as Validate>::validate(&__input)`
//!   call after `serde_json::from_value` succeeds. On a non-empty
//!   `Vec<ValidationError>`, the unary path returns
//!   `ProcedureResult::Err { http_status: 400, code: "validation_error",
//!   payload: { "errors": [...] } }`, which the router then frames as the
//!   SPEC §4.1 envelope.
//! - The same hook fires for `#[rpc(stream)]` subscriptions, but surfaces as
//!   a single `event: error\ndata: { "code": "validation_error", ... }\n\n`
//!   SSE frame followed by the closing `event: end` (the HTTP status line is
//!   already 200 OK by the time the body streams, per SPEC §4.2).
//! - Validation runs *after* JSON decoding: a malformed body still surfaces
//!   as `decode_error`, never `validation_error`.
//!
//! The tests below intentionally read structurally — parsing the response
//! body as JSON / SSE rather than substring-matching on the wire bytes —
//! because the exact payload formatting is not part of the SPEC contract;
//! only the field names and codes are.
//!
//! NOTE on the procedure signature: the agent spec asks for `-> Result<u64,
//! std::convert::Infallible>`, but `Infallible` does not implement
//! `taut_rpc::TautError` / `taut_rpc::TautType`, so the unary `#[rpc]`
//! expansion would not compile with that error type and the task forbids
//! modifying other crate sources to add the missing impls. The validation
//! contract is independent of the success/error split — the hook fires
//! before the user fn runs — so we use `-> u64` directly. Test 5's expected
//! `{ "ok": 1 }` body is identical either way.

use axum::body::Body;
use http::Request as HttpRequest;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Shared procedures used across the validation tests.
//
// Defined at module scope so each test can reuse the macro-emitted
// `__taut_proc_*()` factory without re-declaring the input struct or the
// procedure body. Each test mounts a fresh `Router::new()` — that's cheap and
// keeps test isolation explicit.
// ---------------------------------------------------------------------------

/// Input for the `create` mutation. Carries enough constraints to drive both
/// the single-violation tests (test 1: `name` too short) and the
/// multi-violation test (test 2: `name` too short *and* `age` below `min`).
#[derive(taut_rpc::Validate, taut_rpc::Type, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
struct CreateInput {
    /// Length-bounded so that an empty / very short string fails.
    #[taut(length(min = 3, max = 32))]
    name: String,
    /// Numeric bounds so that `age < 0` or `age > 150` fails.
    #[taut(min = 0, max = 150)]
    age: i32,
}

/// Mutation procedure used by tests 1, 2, 3, and 5.
///
/// Returns a plain `u64` (not `Result<u64, _>`) for the reason in the module
/// docstring: there's no zero-cost stand-in for `Infallible` we can plug in
/// without touching crate sources, and the success path is unaffected.
#[taut_rpc::rpc(mutation)]
#[allow(clippy::unused_async)] // `#[rpc]` requires `async fn` signatures
async fn create(input: CreateInput) -> u64 {
    // The handler body is irrelevant for tests 1-3 (validation rejects the
    // input before we get here); for test 5 we return 1 so the body matches
    // the agent spec's `{"ok":1}` expectation.
    let _ = input;
    1
}

/// Input for the `ticks` subscription. Single numeric field with a `max`
/// bound, so test 4 can send `count = 9999` and watch validation reject it.
///
/// `count` is `u32` rather than the more obvious `u64`: `taut_rpc::validate`'s
/// numeric checkers are bounded `T: Into<f64>`, and `u64` does not implement
/// `Into<f64>` losslessly (only `u8`/`u16`/`u32` and the signed counterparts
/// up to `i32` do). `u32` is the widest unsigned type that compiles here and
/// is plenty for a `count` that's bounded by `max(100)` anyway.
#[derive(taut_rpc::Validate, taut_rpc::Type, serde::Serialize, serde::Deserialize)]
#[allow(dead_code)]
struct TicksInput {
    /// Range-bounded so that `count > 100` fails. `min = 0` is a no-op for
    /// the unsigned type but pins the lower bound explicitly for readability.
    #[taut(min = 0, max = 100)]
    count: u32,
}

/// Subscription procedure used by test 4. Yields nothing useful — the test
/// only inspects the `validation_error` frame, so the stream body never runs.
#[taut_rpc::rpc(stream)]
#[allow(clippy::unused_async)] // `#[rpc(stream)]` requires `async fn` signatures
async fn ticks(input: TicksInput) -> impl futures::Stream<Item = u32> + Send + 'static {
    async_stream::stream! {
        for i in 0..input.count {
            yield i;
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Drain the body of an axum `Response` into a `String`. All the bodies under
/// test (JSON envelopes for unary, SSE text for streams) are UTF-8 text, so
/// converting once up front lets every test do plain `serde_json::from_str` /
/// `str::contains` checks without wrestling with `Bytes`.
async fn body_string(response: axum::http::Response<Body>) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    String::from_utf8(bytes.to_vec()).expect("response body is utf-8")
}

// ---------------------------------------------------------------------------
// 1. A single failing constraint surfaces as the SPEC §7 envelope:
//    HTTP 400, `err.code == "validation_error"`, `err.payload.errors[]` of
//    `{ path, constraint, message }` objects.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validation_error_payload_includes_path_constraint_message() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_create())
        .into_axum();

    // `name = ""` violates `length(min = 3, ...)`, but `age = 30` is fine —
    // so we expect exactly one ValidationError on the `name` path.
    let body = serde_json::json!({
        "input": { "name": "", "age": 30 }
    })
    .to_string();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/create")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    // The router returns the validation envelope with HTTP 400 — that's the
    // SPEC §7 + §4.1 contract. We pin the status before parsing so a 200
    // (validation hook missing) fails loudly with a status mismatch instead
    // of a confusing JSON-shape error downstream.
    assert_eq!(
        response.status(),
        http::StatusCode::BAD_REQUEST,
        "validation failure must be a 400 BAD_REQUEST",
    );

    let body = body_string(response).await;
    let v: serde_json::Value = serde_json::from_str(&body).expect("body is JSON");

    // The envelope is `{ "err": { "code": ..., "payload": ... } }`. We assert
    // on `code` and the shape of `payload.errors` rather than the entire
    // response value — the message text isn't pinned by the SPEC and is
    // free to change between releases.
    assert_eq!(
        v["err"]["code"],
        serde_json::json!("validation_error"),
        "expected `err.code == \"validation_error\"`, got {v}",
    );

    let errors = v["err"]["payload"]["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("expected `err.payload.errors` to be an array, got {v}"));

    assert!(
        !errors.is_empty(),
        "expected at least one ValidationError in payload.errors, got {v}",
    );

    // SPEC §7 says each ValidationError carries `path`, `constraint`, and
    // `message` strings. Assert structurally on every entry — this catches
    // both missing fields and accidental type changes (e.g. a numeric `path`).
    for (i, err) in errors.iter().enumerate() {
        assert!(
            err["path"].is_string(),
            "errors[{i}].path must be a string, got {err}",
        );
        assert!(
            err["constraint"].is_string(),
            "errors[{i}].constraint must be a string, got {err}",
        );
        assert!(
            err["message"].is_string(),
            "errors[{i}].message must be a string, got {err}",
        );
    }

    // Pin the *single* error this input is expected to produce: a `length`
    // failure on `name`. This catches a future regression where unrelated
    // fields start emitting spurious errors.
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one error, got {errors:?}"
    );
    assert_eq!(errors[0]["path"], serde_json::json!("name"));
    assert_eq!(errors[0]["constraint"], serde_json::json!("length"));
}

// ---------------------------------------------------------------------------
// 2. When two distinct fields fail, both errors land in `payload.errors`.
//    SPEC §7 specifies that the `Validate` impl accumulates failures rather
//    than short-circuiting — this test pins that contract end-to-end.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_constraint_violations_all_reported() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_create())
        .into_axum();

    // `name = "ab"` fails `length(min = 3, ...)`; `age = -1` fails `min(0)`.
    // Two distinct failures → two distinct entries in `payload.errors`.
    let body = serde_json::json!({
        "input": { "name": "ab", "age": -1 }
    })
    .to_string();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/create")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
    let body = body_string(response).await;
    let v: serde_json::Value = serde_json::from_str(&body).expect("body is JSON");
    assert_eq!(v["err"]["code"], serde_json::json!("validation_error"));

    let errors = v["err"]["payload"]["errors"]
        .as_array()
        .expect("errors is array");

    // SPEC §7: at least the two failing fields must be reported. We use
    // `>=` rather than `==` to leave room for future extra checks (e.g. a
    // `format` constraint that fires alongside `length`) without breaking
    // the test.
    assert!(
        errors.len() >= 2,
        "expected at least two errors for two-field violation, got {errors:?}",
    );

    // Collect the reported paths and assert both failing fields appear.
    // We collect into a `HashSet` so the assertion is order-independent —
    // SPEC §7 doesn't pin emission order, only that every failure surfaces.
    let paths: std::collections::HashSet<&str> =
        errors.iter().filter_map(|e| e["path"].as_str()).collect();
    assert!(
        paths.contains("name"),
        "expected `name` in error paths, got {paths:?}",
    );
    assert!(
        paths.contains("age"),
        "expected `age` in error paths, got {paths:?}",
    );
}

// ---------------------------------------------------------------------------
// 3. Decode runs before validate: a malformed body returns `decode_error`,
//    NOT `validation_error`. SPEC §7 puts the validation hook *after*
//    `serde_json::from_value` succeeds; sending invalid JSON must short-circuit
//    at decode time and never reach the Validate impl.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validation_runs_after_decode_not_before() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_create())
        .into_axum();

    // Outright-malformed JSON: axum's `Json` extractor rejects this with a
    // `JsonRejection` long before the `#[rpc]` body sees a `Value`.
    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/create")
                .header("content-type", "application/json")
                .body(Body::from("this is not json"))
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    assert_eq!(
        response.status(),
        http::StatusCode::BAD_REQUEST,
        "malformed JSON must surface as 400 BAD_REQUEST",
    );

    let body = body_string(response).await;
    let v: serde_json::Value = serde_json::from_str(&body).expect("body is JSON");
    assert_eq!(
        v["err"]["code"],
        serde_json::json!("decode_error"),
        "expected `decode_error`, got {v}",
    );
    assert_ne!(
        v["err"]["code"],
        serde_json::json!("validation_error"),
        "decode failure must NOT surface as validation_error: {v}",
    );
}

// ---------------------------------------------------------------------------
// 4. Subscription path: validation failures surface as a single `event: error`
//    SSE frame whose JSON body has `code = "validation_error"`, followed by
//    the closing `event: end` frame. The HTTP status remains 200 because the
//    response is already framed `text/event-stream` by the time the validation
//    hook runs (SPEC §4.2).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_input_validation_emits_error_frame() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_ticks())
        .into_axum();

    // `count = 9999` violates `max(100)` — the input decodes fine but the
    // Validate impl rejects it. We URL-encode the JSON manually rather than
    // pull in `urlencoding` as a dev-dep: `{"count":9999}` contains no
    // characters that require percent-encoding when placed in a query string.
    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri("/rpc/ticks?input=%7B%22count%22%3A9999%7D")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    // 200 OK: SSE responses commit their status line before any frame flows,
    // so per-stream errors ride the body, not the status.
    assert_eq!(response.status(), http::StatusCode::OK);

    let body = body_string(response).await;

    // The first event must be `event: error`. Substring-matching is enough
    // here — the SSE framing is small, stable, and asserted on more
    // exhaustively in `tests/phase3.rs`.
    assert!(
        body.contains("event: error"),
        "expected `event: error` frame in body, got:\n{body}",
    );

    // Pull the `data: <json>` line out of the error frame and parse it. The
    // `code` field must be `validation_error` — that's the contract this
    // test exists to pin.
    let error_section = body
        .split("event: error\n")
        .nth(1)
        .expect("error frame present");
    let data_line = error_section
        .lines()
        .find(|l| l.starts_with("data: "))
        .expect("data line under error event");
    let json_str = data_line.strip_prefix("data: ").unwrap();
    let v: serde_json::Value = serde_json::from_str(json_str).expect("valid json in error frame");
    assert_eq!(
        v["code"],
        serde_json::json!("validation_error"),
        "expected validation_error code in SSE error frame, got {v}",
    );

    // The same `payload.errors` array shape as the unary path: an array of
    // `{ path, constraint, message }` objects. We assert the array exists
    // and is non-empty; the per-entry shape was already pinned in test 1.
    let errors = v["payload"]["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("expected payload.errors array, got {v}"));
    assert!(!errors.is_empty(), "expected at least one error, got {v}");
    // `count` is the only constrained field — pin that the failing path is
    // reported correctly here too.
    assert!(
        errors
            .iter()
            .any(|e| e["path"] == serde_json::json!("count")),
        "expected `count` in error paths, got {errors:?}",
    );

    // Stream must close with `event: end` after the error frame, per SPEC
    // §4.2 — and `end` must come after `error`, not before.
    let error_idx = body.find("event: error").expect("error frame present");
    let end_idx = body
        .find("event: end")
        .expect("missing event:end terminator");
    assert!(
        end_idx > error_idx,
        "event:end must follow event:error; body was:\n{body}",
    );
}

// ---------------------------------------------------------------------------
// 5. Happy path: a valid input passes validation and the handler runs.
//    Pins that the validation hook is *not* an unconditional reject — when
//    constraints are satisfied the success envelope flows normally.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validation_passes_when_input_satisfies_constraints() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_create())
        .into_axum();

    // `name = "alice"` satisfies `length(min = 3, max = 32)`; `age = 30`
    // satisfies `min(0), max(150)`. Both constraints pass, so the handler
    // runs and returns `1`.
    let body = serde_json::json!({
        "input": { "name": "alice", "age": 30 }
    })
    .to_string();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/create")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    assert_eq!(response.status(), http::StatusCode::OK);
    let body = body_string(response).await;
    let v: serde_json::Value = serde_json::from_str(&body).expect("body is JSON");
    assert_eq!(
        v,
        serde_json::json!({ "ok": 1 }),
        "expected `{{\"ok\":1}}` for valid input, got {v}",
    );
}

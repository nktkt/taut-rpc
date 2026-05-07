//! Phase 4 integration tests for taut-rpc — the validation pipeline end-to-end.
//!
//! These tests pin the macro -> runtime -> router pipeline against the Phase 4
//! shared contract:
//!
//! - `#[derive(taut_rpc::Validate)]` produces a `Validate` impl from per-field
//!   `#[taut(min, max, length, pattern, email, url, custom)]` attributes.
//! - `#[derive(taut_rpc::Type)]` records the same constraints into the IR
//!   (`ir::Field.constraints`) so codegen can emit a matching TS schema.
//! - `#[rpc]` inserts a validation call after deserializing the input; on
//!   `Err`, the router responds with HTTP 400 and the SPEC envelope
//!   `{"err":{"code":"validation_error","payload":{"errors":[...]}}}`.
//! - `IR_VERSION` has been bumped to 1.
//!
//! The tests are split into three groups:
//!
//! 1. Trait-level: directly invoking `Validate::validate()` on derived impls
//!    and asserting the returned `Result`.
//! 2. IR-level: asserting that `#[derive(Type)]` writes the field-level
//!    constraints into the IR `Field` shape.
//! 3. HTTP-level: building a `Router` over an `#[rpc]`-decorated mutation,
//!    submitting valid and invalid inputs, and asserting the wire envelope.

use axum::body::Body;
use http::Request as HttpRequest;
use tower::ServiceExt;

use taut_rpc::ir::{Constraint, TypeShape};
use taut_rpc::{TautType, Validate, IR_VERSION};

// ---------------------------------------------------------------------------
// 1. A struct with `#[derive(Validate)]` and zero `#[taut(...)]` attrs still
//    gets a working `Validate` impl whose `validate()` returns `Ok(())`.
//
//    The derive lowers to an empty `validate::run(|_| {})` body — no checks,
//    no errors — so the surface contract (always `Ok`) is the right thing
//    to pin.
// ---------------------------------------------------------------------------

#[test]
fn derive_validate_struct_with_no_constraints_returns_ok() {
    #[derive(taut_rpc::Validate)]
    #[allow(dead_code)]
    struct Empty {
        note: String,
        count: i32,
    }

    let v = Empty {
        note: "anything goes".to_string(),
        count: -42,
    };
    v.validate()
        .expect("a struct with no constraints must always validate Ok");
}

// ---------------------------------------------------------------------------
// 2. Numeric `min` / `max` on an integer field. Boundary values (`== min`
//    and `== max`) must pass; values outside the inclusive range must fail
//    with a `min` or `max` constraint code respectively.
// ---------------------------------------------------------------------------

#[test]
fn derive_validate_min_max_on_number() {
    #[derive(taut_rpc::Validate)]
    struct X {
        #[taut(min = 0, max = 100)]
        age: i32,
    }

    // In-range, including both inclusive boundaries.
    X { age: 0 }
        .validate()
        .expect("age == 0 (min boundary) is in range");
    X { age: 50 }.validate().expect("age 50 is in range");
    X { age: 100 }
        .validate()
        .expect("age == 100 (max boundary) is in range");

    // Below min.
    let errs = X { age: -1 }
        .validate()
        .expect_err("age == -1 must fail min(0)");
    assert_eq!(
        errs.len(),
        1,
        "exactly one violation expected, got {errs:?}"
    );
    assert_eq!(errs[0].constraint, "min");
    assert_eq!(errs[0].path, "age");

    // Above max.
    let errs = X { age: 101 }
        .validate()
        .expect_err("age == 101 must fail max(100)");
    assert_eq!(
        errs.len(),
        1,
        "exactly one violation expected, got {errs:?}"
    );
    assert_eq!(errs[0].constraint, "max");
    assert_eq!(errs[0].path, "age");
}

// ---------------------------------------------------------------------------
// 3. String `length(min, max)`. Bounds are character counts (inclusive both
//    sides). Boundary values pass; below-min and above-max fail with a
//    `length` constraint code.
// ---------------------------------------------------------------------------

#[test]
fn derive_validate_length_on_string() {
    #[derive(taut_rpc::Validate)]
    struct X {
        #[taut(length(min = 3, max = 8))]
        name: String,
    }

    // In-range, including both boundaries.
    X {
        name: "abc".to_string(),
    }
    .validate()
    .expect("len 3 == min boundary is in range");
    X {
        name: "abcdef".to_string(),
    }
    .validate()
    .expect("len 6 is in range");
    X {
        name: "abcdefgh".to_string(),
    }
    .validate()
    .expect("len 8 == max boundary is in range");

    // Too short.
    let errs = X {
        name: "ab".to_string(),
    }
    .validate()
    .expect_err("len 2 < min(3) must fail");
    assert_eq!(
        errs.len(),
        1,
        "exactly one violation expected, got {errs:?}"
    );
    assert_eq!(errs[0].constraint, "length");
    assert_eq!(errs[0].path, "name");

    // Too long.
    let errs = X {
        name: "abcdefghi".to_string(),
    }
    .validate()
    .expect_err("len 9 > max(8) must fail");
    assert_eq!(
        errs.len(),
        1,
        "exactly one violation expected, got {errs:?}"
    );
    assert_eq!(errs[0].constraint, "length");
    assert_eq!(errs[0].path, "name");
}

// ---------------------------------------------------------------------------
// 4. `email` constraint. Permissive RFC-5322-ish check: must contain `@` with
//    at least one character before, and a `.` somewhere after with at least
//    one character after the dot. The empty string and obvious malformations
//    (no `@`, no `.` after `@`) must fail.
// ---------------------------------------------------------------------------

#[test]
fn derive_validate_email() {
    #[derive(taut_rpc::Validate)]
    struct X {
        #[taut(email)]
        email: String,
    }

    // Valid.
    X {
        email: "alice@example.com".to_string(),
    }
    .validate()
    .expect("`alice@example.com` is a valid email");

    // No `@`.
    let errs = X {
        email: "no-at-sign.com".to_string(),
    }
    .validate()
    .expect_err("missing `@` must fail email");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].constraint, "email");
    assert_eq!(errs[0].path, "email");

    // No `.` after `@`.
    let errs = X {
        email: "alice@bare".to_string(),
    }
    .validate()
    .expect_err("missing `.` after `@` must fail email");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].constraint, "email");

    // Empty.
    let errs = X {
        email: String::new(),
    }
    .validate()
    .expect_err("empty string must fail email");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].constraint, "email");
}

// ---------------------------------------------------------------------------
// 5. `pattern = "<regex>"`. The regex is compiled at validation time and
//    `is_match` is used (not fully anchored) — but since the test pattern
//    `^[a-z]+$` is fully anchored, only all-lowercase strings of length >= 1
//    match.
// ---------------------------------------------------------------------------

#[test]
fn derive_validate_pattern() {
    #[derive(taut_rpc::Validate)]
    struct X {
        #[taut(pattern = "^[a-z]+$")]
        handle: String,
    }

    // Matching.
    X {
        handle: "alice".to_string(),
    }
    .validate()
    .expect("`alice` matches `^[a-z]+$`");
    X {
        handle: "z".to_string(),
    }
    .validate()
    .expect("single char `z` matches `^[a-z]+$`");

    // Non-matching: contains a digit.
    let errs = X {
        handle: "alice1".to_string(),
    }
    .validate()
    .expect_err("`alice1` contains a digit, must fail pattern");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].constraint, "pattern");
    assert_eq!(errs[0].path, "handle");

    // Non-matching: uppercase.
    let errs = X {
        handle: "Alice".to_string(),
    }
    .validate()
    .expect_err("`Alice` has uppercase, must fail pattern");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].constraint, "pattern");

    // Non-matching: empty (the `+` requires at least one char).
    let errs = X {
        handle: String::new(),
    }
    .validate()
    .expect_err("empty string fails `^[a-z]+$`");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].constraint, "pattern");
}

// ---------------------------------------------------------------------------
// 6. `url` constraint. Must start with `http://` or `https://`; anything else
//    (other schemes, no scheme, empty) fails.
// ---------------------------------------------------------------------------

#[test]
fn derive_validate_url() {
    #[derive(taut_rpc::Validate)]
    struct X {
        #[taut(url)]
        homepage: String,
    }

    // Valid.
    X {
        homepage: "https://example.com".to_string(),
    }
    .validate()
    .expect("https URL is valid");
    X {
        homepage: "http://localhost:8080/path".to_string(),
    }
    .validate()
    .expect("http URL is valid");

    // Missing scheme.
    let errs = X {
        homepage: "example.com".to_string(),
    }
    .validate()
    .expect_err("missing scheme must fail url");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].constraint, "url");
    assert_eq!(errs[0].path, "homepage");

    // Wrong scheme.
    let errs = X {
        homepage: "ftp://example.com".to_string(),
    }
    .validate()
    .expect_err("ftp scheme must fail url");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].constraint, "url");

    // Empty.
    let errs = X {
        homepage: String::new(),
    }
    .validate()
    .expect_err("empty string must fail url");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].constraint, "url");
}

// ---------------------------------------------------------------------------
// 7. The derive accumulates errors across fields (and across multiple checks
//    on a single field) rather than short-circuiting on the first failure.
//
//    Two failing fields must produce two `ValidationError` entries in the
//    returned vec — codegen relies on this so the TS client can render every
//    field error at once.
// ---------------------------------------------------------------------------

#[test]
fn derive_validate_collects_multiple_errors() {
    #[derive(taut_rpc::Validate)]
    struct X {
        #[taut(min = 0)]
        age: i32,
        #[taut(length(min = 3))]
        name: String,
    }

    let errs = X {
        age: -5,
        name: "ab".to_string(),
    }
    .validate()
    .expect_err("two failing fields must yield two errors");

    assert_eq!(errs.len(), 2, "expected exactly two errors, got {errs:?}");

    // Order follows the field declaration order.
    let codes: Vec<&str> = errs.iter().map(|e| e.constraint.as_str()).collect();
    let paths: Vec<&str> = errs.iter().map(|e| e.path.as_str()).collect();
    assert!(
        codes.contains(&"min"),
        "expected a `min` violation in {codes:?}",
    );
    assert!(
        codes.contains(&"length"),
        "expected a `length` violation in {codes:?}",
    );
    assert!(paths.contains(&"age"), "expected `age` path in {paths:?}");
    assert!(paths.contains(&"name"), "expected `name` path in {paths:?}");
}

// ---------------------------------------------------------------------------
// 8. The `path` field on a `ValidationError` produced by the derive equals
//    the struct field's name. This pins the contract that codegen on the
//    TypeScript side keys errors by the same field name the developer wrote.
// ---------------------------------------------------------------------------

#[test]
fn derive_validate_path_in_error_uses_field_name() {
    #[derive(taut_rpc::Validate)]
    struct CreateUser {
        #[taut(length(min = 3))]
        username: String,
    }

    let errs = CreateUser {
        username: "ab".to_string(),
    }
    .validate()
    .expect_err("len 2 < min(3) must fail");
    assert_eq!(errs.len(), 1);
    assert_eq!(
        errs[0].path, "username",
        "the derive must use the struct field name as the error path",
    );
}

// ---------------------------------------------------------------------------
// 9. `#[derive(Type)]` records `#[taut(min, max, ...)]` constraints into the
//    IR `Field.constraints` vec. Codegen reads this to emit Valibot/Zod
//    schema fragments on the TypeScript side.
// ---------------------------------------------------------------------------

#[test]
fn derive_type_records_constraints_in_ir_field() {
    #[derive(taut_rpc::Type)]
    #[allow(dead_code)]
    struct X {
        #[taut(min = 0, max = 100)]
        age: i32,
    }

    let def = <X as TautType>::ir_type_def().expect("X must produce a TypeDef");
    let fields = match &def.shape {
        TypeShape::Struct(f) => f,
        other => panic!("expected struct shape, got {other:?}"),
    };
    assert_eq!(fields.len(), 1);
    let age = &fields[0];
    assert_eq!(age.name, "age");
    assert_eq!(
        age.constraints,
        vec![Constraint::Min(0.0), Constraint::Max(100.0)],
        "expected `Min(0.0)` then `Max(100.0)` in declaration order, got {:?}",
        age.constraints,
    );
}

// ---------------------------------------------------------------------------
// 10. End-to-end: an `#[rpc(mutation)]` with a `#[derive(Validate)]` input
//     rejects bad input with HTTP 400 and the SPEC validation envelope.
//
//     The `#[rpc]` macro inserts a `Validate::validate()` call after
//     deserializing the input and before invoking the handler; on `Err`, the
//     router emits `{"err":{"code":"validation_error","payload":{"errors":[
//     {"path":"username", ...} ]}}}` per the Phase 4 contract.
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize, taut_rpc::Type, taut_rpc::Validate)]
struct CreateUser {
    #[taut(length(min = 3))]
    username: String,
}

#[taut_rpc::rpc(mutation)]
#[allow(clippy::unused_async)] // `#[rpc]` requires `async fn` signatures
async fn create_user(input: CreateUser) -> u64 {
    // The handler shouldn't run for invalid inputs — the router rejects
    // before getting here. For valid inputs we simply hand back a stub id so
    // the success-path test (test 11) has a known body to assert against.
    let _ = input;
    1
}

#[tokio::test]
async fn router_rejects_invalid_input_with_validation_error_envelope() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_create_user())
        .into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/create_user")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"input":{"username":"ab"}}"#))
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    // SPEC §7: validation failures are reported as HTTP 400 with the
    // validation_error envelope.
    assert_eq!(
        response.status(),
        http::StatusCode::BAD_REQUEST,
        "validation failures must surface as HTTP 400",
    );

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("body must be valid JSON");

    // Top-level shape: `{"err": {"code": "validation_error", "payload": ...}}`.
    assert_eq!(
        v["err"]["code"],
        serde_json::json!("validation_error"),
        "expected `validation_error` envelope, got {v}",
    );

    // Payload shape: `{"errors": [{"path": "username", ...}, ...]}`.
    let errors = v["err"]["payload"]["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("expected `errors` array in payload, got {v}"));
    assert!(!errors.is_empty(), "errors array must be non-empty: {v}");
    assert_eq!(
        errors[0]["path"],
        serde_json::json!("username"),
        "first error must point at `username`, got {v}",
    );
}

// ---------------------------------------------------------------------------
// 11. The success path of the same procedure: a valid input flows through
//     the validation gate and reaches the handler, returning HTTP 200 and
//     the canonical `{"ok": <output>}` envelope.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn router_passes_valid_input_through() {
    let app = taut_rpc::Router::new()
        .procedure(__taut_proc_create_user())
        .into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/create_user")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"input":{"username":"alice"}}"#))
                .unwrap(),
        )
        .await
        .expect("oneshot dispatch");

    assert_eq!(response.status(), http::StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("body must be valid JSON");

    assert_eq!(
        v,
        serde_json::json!({ "ok": 1 }),
        "valid input must produce the canonical `{{ok: <output>}}` envelope",
    );
}

// ---------------------------------------------------------------------------
// 12. SPEC §9: the IR schema version is bumped to 1 in Phase 4 because the
//     `Field.constraints` field is a (backward-compatible at the JSON layer
//     but semantically new) addition to the IR. Codegen pins on this constant
//     to refuse mismatched IR documents.
// ---------------------------------------------------------------------------

#[test]
fn ir_version_bumped_to_one() {
    assert_eq!(IR_VERSION, 1, "Phase 4 bumps the IR schema version to 1");
}

//! End-to-end integration tests for taut-rpc Phase 1.
//!
//! These tests exercise the full macro -> runtime pipeline:
//!
//! - `#[derive(Type)]` lowering a Rust type to an IR `TypeDef` (and its
//!   transitive dependents).
//! - `#[rpc]` emitting a `__taut_proc_<name>()` `ProcedureDescriptor` whose
//!   IR fragment matches the function's signature.
//! - `Router::into_axum()` serving those descriptors over HTTP per SPEC §4.1
//!   (success / error / `decode_error` / `not_found` envelopes).
//!
//! The codegen side (TypeScript output) is covered separately in
//! `codegen_snapshot.rs`; these tests stay Rust-only.

use serde::{Deserialize, Serialize};
use taut_rpc::{rpc, Router, Type, IR_VERSION};

use axum::body::Body;
use http::Request as HttpRequest;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// (a) #[derive(Type)] on a struct emits a Struct TypeDef.
// ---------------------------------------------------------------------------

#[test]
fn derive_type_struct_emits_typedef() {
    use taut_rpc::ir::{Primitive, TypeRef, TypeShape};
    use taut_rpc::TautType;

    #[derive(Type, Serialize, Deserialize)]
    #[allow(dead_code)]
    struct User {
        id: u64,
        name: String,
    }

    let def = <User as TautType>::ir_type_def().expect("User must produce a TypeDef");
    assert_eq!(def.name, "User");

    let fields = match &def.shape {
        TypeShape::Struct(f) => f,
        other => panic!("expected struct shape, got {other:?}"),
    };
    assert_eq!(fields.len(), 2, "User should have two fields");
    assert_eq!(fields[0].name, "id");
    assert_eq!(fields[0].ty, TypeRef::Primitive(Primitive::U64));
    assert_eq!(fields[1].name, "name");
    assert_eq!(fields[1].ty, TypeRef::Primitive(Primitive::String));

    // ir_type_ref points to the named type by name.
    assert_eq!(
        <User as TautType>::ir_type_ref(),
        TypeRef::Named("User".to_string())
    );
}

// ---------------------------------------------------------------------------
// (b) #[derive(Type)] on an enum emits a discriminated-union TypeDef
//     covering unit, tuple, and struct variants.
// ---------------------------------------------------------------------------

#[test]
fn derive_type_enum_emits_discriminated_union() {
    use taut_rpc::ir::{Primitive, TypeRef, TypeShape, VariantPayload};
    use taut_rpc::TautType;

    #[derive(Type, Serialize, Deserialize)]
    #[allow(dead_code)]
    enum Event {
        Ping,
        Message(String),
        Login { user_id: u64, name: String },
    }

    let def = <Event as TautType>::ir_type_def().expect("Event must produce a TypeDef");
    assert_eq!(def.name, "Event");

    let enum_def = match &def.shape {
        TypeShape::Enum(e) => e,
        other => panic!("expected enum shape, got {other:?}"),
    };
    // Default tag per SPEC §3.2 / derive_type docs is "type".
    assert_eq!(enum_def.tag, "type");
    assert_eq!(enum_def.variants.len(), 3);

    // Unit variant.
    assert_eq!(enum_def.variants[0].name, "Ping");
    assert!(matches!(enum_def.variants[0].payload, VariantPayload::Unit));

    // Tuple variant.
    assert_eq!(enum_def.variants[1].name, "Message");
    let tuple = match &enum_def.variants[1].payload {
        VariantPayload::Tuple(t) => t,
        other => panic!("expected tuple variant, got {other:?}"),
    };
    assert_eq!(tuple.len(), 1);
    assert_eq!(tuple[0], TypeRef::Primitive(Primitive::String));

    // Struct variant.
    assert_eq!(enum_def.variants[2].name, "Login");
    let fields = match &enum_def.variants[2].payload {
        VariantPayload::Struct(f) => f,
        other => panic!("expected struct variant, got {other:?}"),
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "user_id");
    assert_eq!(fields[0].ty, TypeRef::Primitive(Primitive::U64));
    assert_eq!(fields[1].name, "name");
    assert_eq!(fields[1].ty, TypeRef::Primitive(Primitive::String));
}

// ---------------------------------------------------------------------------
// (c) collect_type_defs walks into reachable user-defined field types.
// ---------------------------------------------------------------------------

#[test]
fn derive_type_collect_recurses_into_field_types() {
    use taut_rpc::TautType;

    #[derive(Type, Serialize, Deserialize)]
    #[allow(dead_code)]
    struct Address {
        street: String,
        zip: String,
    }

    #[derive(Type, Serialize, Deserialize)]
    #[allow(dead_code)]
    struct Person {
        name: String,
        addr: Address,
    }

    let mut out = Vec::new();
    <Person as TautType>::collect_type_defs(&mut out);
    let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
    assert!(
        names.contains(&"Person"),
        "expected Person in collected defs, got {names:?}"
    );
    assert!(
        names.contains(&"Address"),
        "expected Address in collected defs, got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// (d) #[rpc] emits a ProcedureDescriptor matching the function signature.
// ---------------------------------------------------------------------------

#[rpc]
#[allow(clippy::unused_async)] // `#[rpc]` requires `async fn` signatures
async fn ping() -> String {
    "pong".to_string()
}

#[test]
fn rpc_macro_emits_descriptor() {
    use taut_rpc::ir::{Primitive, TypeRef};
    use taut_rpc::ProcKindRuntime;

    let desc = __taut_proc_ping();
    assert_eq!(desc.name, "ping");
    assert_eq!(desc.kind, ProcKindRuntime::Query);
    assert_eq!(desc.ir.input, TypeRef::Primitive(Primitive::Unit));
    assert_eq!(desc.ir.output, TypeRef::Primitive(Primitive::String));
    assert!(desc.ir.errors.is_empty());
}

// ---------------------------------------------------------------------------
// (e) Router serves a typed query end-to-end over HTTP.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn router_serves_typed_query() {
    let app = Router::new().procedure(__taut_proc_ping()).into_axum();

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

    assert_eq!(response.status(), http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v, serde_json::json!({"ok": "pong"}));
}

// ---------------------------------------------------------------------------
// (f) Router serves a typed mutation taking an input struct.
// ---------------------------------------------------------------------------

#[derive(Type, taut_rpc::Validate, Serialize, Deserialize)]
struct AddInput {
    a: i32,
    b: i32,
}

#[rpc(mutation)]
#[allow(clippy::unused_async)] // `#[rpc]` requires `async fn` signatures
async fn add(input: AddInput) -> i32 {
    input.a + input.b
}

#[tokio::test]
async fn router_serves_typed_mutation_with_input_struct() {
    let app = Router::new().procedure(__taut_proc_add()).into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/add")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"input":{"a":2,"b":3}}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v, serde_json::json!({"ok": 5}));
}

// ---------------------------------------------------------------------------
// (g) Handler errors surface as a SPEC §4.1 error envelope.
// ---------------------------------------------------------------------------

/// A user-defined error type. With `#[derive(TautError)]` the macro picks the
/// per-variant `code` (`snake_case`'d) and the default HTTP status (400).
#[derive(serde::Serialize, taut_rpc::Type, taut_rpc::TautError, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
#[allow(dead_code)]
enum MyErr {
    Boom { reason: String },
}

#[rpc]
#[allow(clippy::unnecessary_wraps, clippy::unused_async)] // `#[rpc]` requires `async fn` signatures
async fn fails() -> Result<i32, MyErr> {
    Err(MyErr::Boom {
        reason: "test".into(),
    })
}

#[tokio::test]
async fn router_returns_error_envelope_on_handler_error() {
    let app = Router::new().procedure(__taut_proc_fails()).into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/fails")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"input":null}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Default HTTP status from `TautError::http_status` is 400.
    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    // The wire envelope's `code` comes from `<MyErr as TautError>::code()`.
    // The `payload` is the full serde serialization of the error value, so
    // when the user opts into `#[serde(tag = "code", content = "payload")]`
    // the payload itself nests `{code, payload}`. This is documented; users
    // who don't want nesting should serialize their error types without that
    // serde shape.
    assert_eq!(
        v,
        serde_json::json!({
            "err": {
                "code": "boom",
                "payload": { "code": "boom", "payload": { "reason": "test" } }
            }
        })
    );
}

// ---------------------------------------------------------------------------
// (g2) Per-variant `#[taut(status = ...)]` overrides flow through to the HTTP
//      response status, not just to the body.
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, taut_rpc::Type, taut_rpc::TautError, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
#[allow(dead_code)]
enum AuthErr {
    #[taut(status = 401)]
    Unauthenticated,
}

#[rpc]
#[allow(clippy::unnecessary_wraps, clippy::unused_async)] // `#[rpc]` requires `async fn` signatures
async fn protected() -> Result<(), AuthErr> {
    Err(AuthErr::Unauthenticated)
}

#[tokio::test]
async fn router_uses_taut_error_http_status_override() {
    let app = Router::new().procedure(__taut_proc_protected()).into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/protected")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"input":null}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // The `#[taut(status = 401)]` annotation on the variant must propagate
    // all the way out to the HTTP status — not just appear in the body.
    assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["err"]["code"], serde_json::json!("unauthenticated"));
}

// ---------------------------------------------------------------------------
// (h) Malformed JSON request bodies surface as a `decode_error` envelope.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn router_decodes_bad_json_to_envelope() {
    let app = Router::new().procedure(__taut_proc_ping()).into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/rpc/ping")
                .header("content-type", "application/json")
                .body(Body::from("not json at all"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["err"]["code"], serde_json::json!("decode_error"));
}

// ---------------------------------------------------------------------------
// (i) Unknown procedures surface as a `not_found` envelope.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn router_returns_404_envelope_for_unknown_procedure() {
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

    assert_eq!(response.status(), http::StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["err"]["code"], serde_json::json!("not_found"));
}

// ---------------------------------------------------------------------------
// (j) `/rpc/_ir` returns a valid Ir document when the feature is enabled.
// ---------------------------------------------------------------------------

#[cfg(feature = "ir-export")]
#[tokio::test]
async fn ir_endpoint_returns_full_ir_when_feature_enabled() {
    use taut_rpc::ir::Ir;

    let app = Router::new().procedure(__taut_proc_ping()).into_axum();

    let response = app
        .oneshot(
            HttpRequest::builder()
                .method("GET")
                .uri("/rpc/_ir")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ir: Ir = serde_json::from_slice(&bytes).expect("valid Ir JSON");
    assert_eq!(ir.ir_version, Ir::CURRENT_VERSION);
    let names: Vec<&str> = ir.procedures.iter().map(|p| p.name.as_str()).collect();
    assert!(
        names.contains(&"ping"),
        "expected `ping` in IR procedures, got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// (k) IR_VERSION tracks the current schema version (bumped to 1 in Phase 4).
// ---------------------------------------------------------------------------

#[test]
fn ir_version_is_current() {
    assert_eq!(IR_VERSION, 1);
}

//! Integration test for the MCP manifest emitter (`taut_rpc_cli::mcp`).
//!
//! Exercises two paths:
//!
//! 1. A hand-built IR that mirrors what `#[derive(Type)]` + `#[rpc]` would
//!    produce for a typical query, run through `render_manifest`, and
//!    asserted against MCP `tools/list` shape (spec 2025-06-18).
//! 2. A live macro pipeline: a real `#[rpc]` function whose
//!    `__taut_proc_*()` descriptor is mounted on a `Router`, the IR pulled
//!    via `Router::ir()`, and the resulting manifest validated.
//!
//! The unit-level matrix (every `TypeRef` variant, primitive, edge case) lives
//! inline in `crates/taut-rpc-cli/src/mcp.rs`. This file is the integration
//! glue — proving the macro→IR→manifest pipeline holds end-to-end.

use serde::{Deserialize, Serialize};
use serde_json::json;
use taut_rpc::ir::{
    Field, HttpMethod, Ir, Primitive, ProcKind, Procedure, TypeDef, TypeRef, TypeShape,
};
use taut_rpc::{rpc, Router, Type};
use taut_rpc_cli::mcp::{render_manifest, McpOptions};

#[test]
fn hand_built_ir_round_trips_through_manifest_shape() {
    let ir = Ir {
        ir_version: Ir::CURRENT_VERSION,
        procedures: vec![Procedure {
            name: "get_user".into(),
            kind: ProcKind::Query,
            input: TypeRef::Named("UserId".into()),
            output: TypeRef::Named("User".into()),
            errors: vec![],
            http_method: HttpMethod::Post,
            doc: Some("Fetch a user by id.".into()),
        }],
        types: vec![
            TypeDef {
                name: "UserId".into(),
                doc: None,
                shape: TypeShape::Newtype(TypeRef::Primitive(Primitive::Uuid)),
            },
            TypeDef {
                name: "User".into(),
                doc: None,
                shape: TypeShape::Struct(vec![Field {
                    name: "id".into(),
                    ty: TypeRef::Named("UserId".into()),
                    optional: false,
                    undefined: false,
                    doc: None,
                    constraints: vec![],
                }]),
            },
        ],
    };

    let manifest = render_manifest(&ir, &McpOptions::default());
    let tools = manifest["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 1);

    let tool = &tools[0];
    assert_eq!(tool["name"], "get_user");
    assert_eq!(tool["description"], "Fetch a user by id.");

    let schema = &tool["inputSchema"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["input"]));
    assert_eq!(
        schema["properties"]["input"],
        json!({ "$ref": "#/$defs/UserId" })
    );

    // Newtype unwrapping pulls the primitive shape into UserId, so the input
    // remains validatable end to end without a wrapping object.
    let user_id = &schema["$defs"]["UserId"];
    assert_eq!(user_id["type"], "string");
    assert_eq!(user_id["format"], "uuid");

    // The output type is reachable from procedure output, but the manifest
    // intentionally only emits input schemas — output isn't part of MCP's
    // contract today, so `User` should NOT appear in $defs.
    assert!(schema["$defs"].get("User").is_none(), "got: {schema}");
}

// ---------------------------------------------------------------------------
// Live macro pipeline
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Type, taut_rpc::Validate)]
pub struct AddInput {
    /// Left operand.
    pub a: i32,
    /// Right operand.
    pub b: i32,
}

/// Adds two integers and returns the sum.
#[rpc]
#[allow(clippy::unused_async)] // `#[rpc]` requires `async fn` signatures
async fn add_numbers(input: AddInput) -> i32 {
    input.a + input.b
}

/// Heartbeat — takes no input, returns a fixed string.
#[rpc]
#[allow(clippy::unused_async)] // `#[rpc]` requires `async fn` signatures
async fn ping(_input: ()) -> String {
    "pong".into()
}

#[test]
fn manifest_from_live_macro_pipeline_matches_mcp_shape() {
    let router = Router::new()
        .procedure(__taut_proc_add_numbers())
        .procedure(__taut_proc_ping());
    let ir = router.ir();
    let manifest = render_manifest(&ir, &McpOptions::default());

    let tools = manifest["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 2);

    let by_name: std::collections::HashMap<&str, &serde_json::Value> = tools
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t))
        .collect();

    let add = by_name["add_numbers"];
    assert_eq!(
        add["description"], "Adds two integers and returns the sum.",
        "rustdoc must thread through to the MCP description field"
    );
    let add_schema = &add["inputSchema"];
    assert_eq!(add_schema["type"], "object");
    assert_eq!(add_schema["required"], json!(["input"]));
    assert_eq!(
        add_schema["properties"]["input"],
        json!({ "$ref": "#/$defs/AddInput" })
    );

    let add_input = &add_schema["$defs"]["AddInput"];
    assert_eq!(add_input["type"], "object");
    assert_eq!(add_input["additionalProperties"], json!(false));
    let mut required = add_input["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    required.sort();
    assert_eq!(required, vec!["a", "b"]);
    assert_eq!(add_input["properties"]["a"]["type"], "integer");
    assert_eq!(add_input["properties"]["a"]["description"], "Left operand.");
    assert_eq!(
        add_input["properties"]["b"]["description"],
        "Right operand."
    );

    let ping = by_name["ping"];
    let ping_schema = &ping["inputSchema"];
    // `()` lowers to Primitive::Unit → JSON null.
    assert_eq!(ping_schema["properties"]["input"]["type"], "null");
}

#[test]
fn manifest_is_serializable_as_pretty_json_without_loss() {
    let router = Router::new().procedure(__taut_proc_add_numbers());
    let ir = router.ir();
    let manifest = render_manifest(&ir, &McpOptions::default());
    let pretty = serde_json::to_string_pretty(&manifest).expect("serialize");
    let parsed: serde_json::Value = serde_json::from_str(&pretty).expect("re-parse");
    assert_eq!(parsed, manifest);
}

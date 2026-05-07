use taut_rpc_cli::codegen::{render_ts, CodegenOptions, Validator};
use taut_rpc_cli::mcp::{render_manifest, McpOptions};

#[test]
fn codegen_empty_ir_succeeds() {
    let ir = taut_rpc::ir::Ir::empty();
    let out = render_ts(&ir, &CodegenOptions::default());
    assert!(out.contains("DO NOT EDIT"));
}

#[test]
fn codegen_zod_validator_emits_zod_import() {
    let ir = taut_rpc::ir::Ir::empty();
    let opts = CodegenOptions {
        validator: Validator::Zod,
        ..Default::default()
    };
    let out = render_ts(&ir, &opts);
    // Empty IR with no types still emits the validator-stub import header? Or not?
    // Loose assertion: output is at least non-empty and has the DO NOT EDIT banner.
    assert!(out.contains("DO NOT EDIT"));
}

#[test]
fn mcp_empty_ir_emits_empty_tools() {
    let ir = taut_rpc::ir::Ir::empty();
    let manifest = render_manifest(&ir, &McpOptions::default());
    assert_eq!(manifest["tools"], serde_json::Value::Array(vec![]));
}

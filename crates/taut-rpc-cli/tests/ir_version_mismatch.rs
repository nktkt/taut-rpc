//! Tests that the CLI rejects IRs with mismatched `ir_version`.

// Note: the actual version-mismatch check is in `commands::gen::run`, which is
// binary-only. These tests assert the underlying invariant: a stale-version IR
// can be detected by inspecting `ir.ir_version` against `taut_rpc::IR_VERSION`.

#[test]
fn detect_stale_ir_version_via_field() {
    let stale_ir_json = r#"{
        "ir_version": 0,
        "procedures": [],
        "types": []
    }"#;
    let stale: taut_rpc::ir::Ir = serde_json::from_str(stale_ir_json).unwrap();
    assert_eq!(stale.ir_version, 0);
    assert_eq!(taut_rpc::IR_VERSION, 1);
    // Caller-side check that we'd refuse:
    assert!(stale.ir_version != taut_rpc::IR_VERSION);
}

#[test]
fn fresh_ir_matches_current_version() {
    let fresh = taut_rpc::ir::Ir::empty();
    assert_eq!(fresh.ir_version, taut_rpc::IR_VERSION);
}

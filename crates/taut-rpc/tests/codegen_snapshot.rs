//! Snapshot-style test for the Phase 1 codegen output.
//!
//! Builds an IR by hand that mimics what the macros would produce for an
//! `add(input: AddInput) -> i32` mutation, runs it through
//! `taut_rpc_cli::codegen::render_ts`, and asserts the rendered TypeScript
//! contains the load-bearing pieces of SPEC §6's client API.
//!
//! The test deliberately stops short of pinning the entire string; the
//! `taut-rpc-cli/src/codegen.rs` test module already does line-level
//! assertions. This test exists to prove the *integration* — specifically,
//! that the IR shape the macros emit lines up with what codegen consumes.

use taut_rpc::ir::{
    Field, HttpMethod, Ir, Primitive, ProcKind, Procedure, TypeDef, TypeRef, TypeShape,
};
use taut_rpc_cli::codegen::{render_ts, CodegenOptions};

#[test]
fn render_ts_for_phase1_example_contains_expected_anchors() {
    // What the macros produce for:
    //
    //   #[derive(Type, serde::Serialize, serde::Deserialize)]
    //   struct AddInput { a: i32, b: i32 }
    //
    //   #[rpc(mutation)]
    //   async fn add(input: AddInput) -> i32 { input.a + input.b }
    let ir = Ir {
        ir_version: Ir::CURRENT_VERSION,
        procedures: vec![Procedure {
            name: "add".to_string(),
            kind: ProcKind::Mutation,
            input: TypeRef::Named("AddInput".to_string()),
            output: TypeRef::Primitive(Primitive::I32),
            errors: vec![],
            http_method: HttpMethod::Post,
            doc: None,
        }],
        types: vec![TypeDef {
            name: "AddInput".to_string(),
            doc: None,
            shape: TypeShape::Struct(vec![
                Field {
                    name: "a".to_string(),
                    ty: TypeRef::Primitive(Primitive::I32),
                    optional: false,
                    undefined: false,
                    doc: None,
                },
                Field {
                    name: "b".to_string(),
                    ty: TypeRef::Primitive(Primitive::I32),
                    optional: false,
                    undefined: false,
                    doc: None,
                },
            ]),
        }],
    };

    let out = render_ts(&ir, &CodegenOptions::default());

    // Header banner — generated files must be obvious to reviewers.
    assert!(
        out.contains("DO NOT EDIT"),
        "header banner missing:\n{out}",
    );

    // Type definition — the input struct surfaces as a TS interface.
    assert!(
        out.contains("export interface AddInput"),
        "AddInput interface missing:\n{out}",
    );

    // Procedure alias — `add` maps to `Proc_add = ProcedureDef<...>`.
    assert!(
        out.contains("Proc_add = ProcedureDef<"),
        "Proc_add alias missing:\n{out}",
    );

    // Cheap format-bug guards — anything obviously broken should not be in
    // the output. `undefined undefined` would mean the field-mode code
    // double-applied; raw `<<` / `>>` typically means a templating glitch.
    assert!(
        !out.contains("undefined undefined"),
        "double-undefined slipped through:\n{out}",
    );
    assert!(
        !out.contains("<<") && !out.contains(">>"),
        "template-style placeholder leaked:\n{out}",
    );
}

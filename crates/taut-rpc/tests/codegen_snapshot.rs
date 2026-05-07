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
    Constraint, Field, HttpMethod, Ir, Primitive, ProcKind, Procedure, TypeDef, TypeRef, TypeShape,
};
use taut_rpc_cli::codegen::{render_ts, CodegenOptions, Validator};

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
                    constraints: vec![],
                },
                Field {
                    name: "b".to_string(),
                    ty: TypeRef::Primitive(Primitive::I32),
                    optional: false,
                    undefined: false,
                    doc: None,
                    constraints: vec![],
                },
            ]),
        }],
    };

    let out = render_ts(&ir, &CodegenOptions::default());

    // Header banner — generated files must be obvious to reviewers.
    assert!(out.contains("DO NOT EDIT"), "header banner missing:\n{out}",);

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
    // Catch leftover `{{placeholder}}`-style markers, but tolerate `<<` / `>>`
    // since nested generics like `v.BaseSchema<unknown, v.BaseIssue<unknown>>`
    // end with `>>` legitimately.
    assert!(
        !out.contains("{{") && !out.contains("}}"),
        "template-style placeholder leaked:\n{out}",
    );
}

// ---------------------------------------------------------------------------
// Phase 4: Valibot/Zod schema emission
// ---------------------------------------------------------------------------

/// Build the canonical `User` IR used by the Phase 4 schema tests.
///
/// Mirrors what the macros would produce for:
/// ```ignore
/// #[derive(Type, Validate)]
/// struct User {
///     id: u64,
///     name: String,
///     #[validate(email)]
///     email: String,
///     #[validate(min = 0, max = 120)]
///     age: u8,
/// }
/// ```
fn user_ir_with_constraints() -> Ir {
    Ir {
        ir_version: Ir::CURRENT_VERSION,
        procedures: vec![],
        types: vec![TypeDef {
            name: "User".to_string(),
            doc: None,
            shape: TypeShape::Struct(vec![
                Field {
                    name: "id".to_string(),
                    ty: TypeRef::Primitive(Primitive::U64),
                    optional: false,
                    undefined: false,
                    doc: None,
                    constraints: vec![],
                },
                Field {
                    name: "name".to_string(),
                    ty: TypeRef::Primitive(Primitive::String),
                    optional: false,
                    undefined: false,
                    doc: None,
                    constraints: vec![],
                },
                Field {
                    name: "email".to_string(),
                    ty: TypeRef::Primitive(Primitive::String),
                    optional: false,
                    undefined: false,
                    doc: None,
                    constraints: vec![Constraint::Email],
                },
                Field {
                    name: "age".to_string(),
                    ty: TypeRef::Primitive(Primitive::U8),
                    optional: false,
                    undefined: false,
                    doc: None,
                    constraints: vec![Constraint::Min(0.0), Constraint::Max(120.0)],
                },
            ]),
        }],
    }
}

/// Pull out the `<line>` containing the rendered schema for a named field of
/// `<TypeName>Schema`. Lets the assertions below scope to a single field
/// rather than relying on the order of substrings appearing somewhere in the
/// whole file.
///
/// Scopes the search to the slice of `out` starting at the first
/// `<TypeName>Schema = ` declaration so the matching field line on the TS
/// `interface <TypeName>` above it doesn't shadow the schema body.
fn schema_field_line<'a>(out: &'a str, type_name: &str, field: &str) -> &'a str {
    let anchor = format!("{type_name}Schema =");
    let start = out
        .find(&anchor)
        .unwrap_or_else(|| panic!("no `{anchor}` declaration in:\n{out}"));
    let body = &out[start..];
    let prefix = format!("{field}:");
    body.lines()
        .find(|line| line.trim_start().starts_with(&prefix))
        .unwrap_or_else(|| panic!("no schema line for field `{field}` in:\n{out}"))
}

#[test]
fn valibot_schema_emitted_for_struct_with_constraints() {
    let ir = user_ir_with_constraints();
    let opts = CodegenOptions {
        validator: Validator::Valibot,
        ..Default::default()
    };
    let out = render_ts(&ir, &opts);

    assert!(
        out.contains("import * as v from \"valibot\";"),
        "valibot import missing:\n{out}",
    );
    assert!(
        out.contains("export const UserSchema = v.object({"),
        "UserSchema header missing:\n{out}",
    );

    let age_line = schema_field_line(&out, "User", "age");
    assert!(
        age_line.contains("v.minValue(0"),
        "age field should carry v.minValue(0..) — got line: {age_line}\nfull:\n{out}",
    );
    assert!(
        age_line.contains("v.maxValue(120"),
        "age field should carry v.maxValue(120..) — got line: {age_line}\nfull:\n{out}",
    );

    let email_line = schema_field_line(&out, "User", "email");
    assert!(
        email_line.contains("v.email()"),
        "email field should carry v.email() — got line: {email_line}\nfull:\n{out}",
    );
}

#[test]
fn zod_schema_emitted_for_struct_with_constraints() {
    let ir = user_ir_with_constraints();
    let opts = CodegenOptions {
        validator: Validator::Zod,
        ..Default::default()
    };
    let out = render_ts(&ir, &opts);

    assert!(
        out.contains("import { z } from \"zod\";"),
        "zod import missing:\n{out}",
    );
    assert!(
        out.contains("export const UserSchema = z.object({"),
        "UserSchema header missing:\n{out}",
    );

    let email_line = schema_field_line(&out, "User", "email");
    assert!(
        email_line.contains("z.string().email()"),
        "email field should be `z.string().email()` — got line: {email_line}\nfull:\n{out}",
    );

    let age_line = schema_field_line(&out, "User", "age");
    assert!(
        age_line.contains(".min(0)"),
        "age field should chain .min(0) — got line: {age_line}\nfull:\n{out}",
    );
    assert!(
        age_line.contains(".max(120)"),
        "age field should chain .max(120) — got line: {age_line}\nfull:\n{out}",
    );
}

#[test]
fn none_validator_emits_no_schemas() {
    let ir = user_ir_with_constraints();
    let opts = CodegenOptions {
        validator: Validator::None,
        ..Default::default()
    };
    let out = render_ts(&ir, &opts);

    assert!(
        !out.contains("import * as v from \"valibot\""),
        "valibot import must not appear when validator is None:\n{out}",
    );
    assert!(
        !out.contains("import { z } from \"zod\""),
        "zod import must not appear when validator is None:\n{out}",
    );
    assert!(
        !out.contains("Schema = "),
        "no `<Name>Schema = ...` constants should be emitted when validator is None:\n{out}",
    );
}

#[test]
fn pattern_constraint_emits_regex_in_valibot() {
    let ir = Ir {
        ir_version: Ir::CURRENT_VERSION,
        procedures: vec![],
        types: vec![TypeDef {
            name: "Slug".to_string(),
            doc: None,
            shape: TypeShape::Struct(vec![Field {
                name: "value".to_string(),
                ty: TypeRef::Primitive(Primitive::String),
                optional: false,
                undefined: false,
                doc: None,
                constraints: vec![Constraint::Pattern("^[a-z]+$".to_string())],
            }]),
        }],
    };
    let opts = CodegenOptions {
        validator: Validator::Valibot,
        ..Default::default()
    };
    let out = render_ts(&ir, &opts);

    let value_line = schema_field_line(&out, "Slug", "value");
    assert!(
        value_line.contains("v.regex(/^[a-z]+$/)") || value_line.contains("v.regex(new RegExp("),
        "value field should carry a regex check — got line: {value_line}\nfull:\n{out}",
    );
}

#[test]
fn length_constraint_emits_min_max_length() {
    let ir = Ir {
        ir_version: Ir::CURRENT_VERSION,
        procedures: vec![],
        types: vec![TypeDef {
            name: "Name".to_string(),
            doc: None,
            shape: TypeShape::Struct(vec![Field {
                name: "value".to_string(),
                ty: TypeRef::Primitive(Primitive::String),
                optional: false,
                undefined: false,
                doc: None,
                constraints: vec![Constraint::Length {
                    min: Some(3),
                    max: Some(32),
                }],
            }]),
        }],
    };
    let opts = CodegenOptions {
        validator: Validator::Valibot,
        ..Default::default()
    };
    let out = render_ts(&ir, &opts);

    let value_line = schema_field_line(&out, "Name", "value");
    assert!(
        value_line.contains("v.minLength(3)"),
        "value field should carry v.minLength(3) — got line: {value_line}\nfull:\n{out}",
    );
    assert!(
        value_line.contains("v.maxLength(32)"),
        "value field should carry v.maxLength(32) — got line: {value_line}\nfull:\n{out}",
    );
}

#[test]
fn procedure_schemas_const_emitted_with_input_output() {
    let ir = Ir {
        ir_version: Ir::CURRENT_VERSION,
        procedures: vec![Procedure {
            name: "add".to_string(),
            kind: ProcKind::Query,
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
                    constraints: vec![],
                },
                Field {
                    name: "b".to_string(),
                    ty: TypeRef::Primitive(Primitive::I32),
                    optional: false,
                    undefined: false,
                    doc: None,
                    constraints: vec![],
                },
            ]),
        }],
    };
    let opts = CodegenOptions {
        validator: Validator::Valibot,
        ..Default::default()
    };
    let out = render_ts(&ir, &opts);

    assert!(
        out.contains("export const procedureSchemas = {"),
        "procedureSchemas const missing:\n{out}",
    );
    // The map's `add` entry references the Named-typed input via its Schema
    // alias, and an inline schema for the i32 output.
    assert!(
        out.contains(
            "\"add\": { input: __taut_wrap(Proc_add_inputSchema), output: __taut_wrap(Proc_add_outputSchema) }",
        ),
        "procedureSchemas entry for `add` missing:\n{out}",
    );
    assert!(
        out.contains("export const Proc_add_inputSchema = AddInputSchema;"),
        "Proc_add_inputSchema should alias AddInputSchema:\n{out}",
    );
    assert!(
        out.contains("export const Proc_add_outputSchema = v.number();"),
        "Proc_add_outputSchema should inline v.number():\n{out}",
    );
}

#[test]
fn primitive_input_emits_inline_schema() {
    let ir = Ir {
        ir_version: Ir::CURRENT_VERSION,
        procedures: vec![Procedure {
            name: "echo".to_string(),
            kind: ProcKind::Query,
            input: TypeRef::Primitive(Primitive::String),
            output: TypeRef::Primitive(Primitive::String),
            errors: vec![],
            http_method: HttpMethod::Post,
            doc: None,
        }],
        types: vec![],
    };
    let opts = CodegenOptions {
        validator: Validator::Valibot,
        ..Default::default()
    };
    let out = render_ts(&ir, &opts);

    // No TypeDef exists for `string`, so the input must be inlined as
    // `v.string()` — both via the per-procedure constant and (transitively)
    // via the `procedureSchemas` map entry.
    assert!(
        out.contains("export const Proc_echo_inputSchema = v.string();"),
        "Proc_echo_inputSchema should inline v.string():\n{out}",
    );
    assert!(
        out.contains(
            "\"echo\": { input: __taut_wrap(Proc_echo_inputSchema), output: __taut_wrap(Proc_echo_outputSchema) }",
        ),
        "procedureSchemas entry for `echo` missing:\n{out}",
    );
}

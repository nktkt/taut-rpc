//! Intermediate representation (IR) types for taut-rpc.
//!
//! The IR is the contract between proc-macro emission (compile-time) and
//! codegen (build-time / `cargo taut gen`). It is a JSON document, written to
//! `target/taut/ir.json`, that fully describes every `#[rpc]` procedure and
//! every type reachable from a procedure signature.
//!
//! See SPEC §2 for the architecture diagram: the proc-macro layer emits IR
//! entries which `cargo taut gen` consumes to produce a static TypeScript
//! client. The IR is schema-versioned via [`Ir::CURRENT_VERSION`]; codegen
//! refuses to operate on a mismatched version (SPEC §9).
//!
//! Every type in this module derives `Serialize`/`Deserialize`, and every
//! tagged enum uses an explicit `tag` (and where applicable `content`) so the
//! on-disk JSON shape is stable and self-describing.

use serde::{Deserialize, Serialize};

/// Re-export of [`crate::validate::Constraint`] so that the IR module is the
/// canonical home for the validation-constraint vocabulary recorded into a
/// [`Field`]. The actual definition lives in [`crate::validate`] (SPEC §7).
pub use crate::validate::Constraint;

/// Root document written to `target/taut/ir.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ir {
    pub ir_version: u32,
    pub procedures: Vec<Procedure>,
    pub types: Vec<TypeDef>,
}

impl Ir {
    /// Current IR schema version. See SPEC §9 — codegen refuses mismatches.
    pub const CURRENT_VERSION: u32 = 1;

    /// Construct an empty IR document at the current schema version.
    pub fn empty() -> Self {
        Self {
            ir_version: Self::CURRENT_VERSION,
            procedures: vec![],
            types: vec![],
        }
    }
}

/// A single `#[rpc]` procedure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Procedure {
    pub name: String,
    pub kind: ProcKind,
    pub input: TypeRef,
    pub output: TypeRef,
    pub errors: Vec<TypeRef>,
    pub http_method: HttpMethod,
    pub doc: Option<String>,
}

/// Procedure flavour. See SPEC §4 (wire format) and §6 (client API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcKind {
    Query,
    Mutation,
    Subscription,
}

/// HTTP method used to dispatch the procedure (SPEC §4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpMethod {
    Post,
    Get,
}

/// A user-defined type reachable from at least one procedure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeDef {
    pub name: String,
    pub doc: Option<String>,
    pub shape: TypeShape,
}

/// The structural shape of a [`TypeDef`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TypeShape {
    Struct(Vec<Field>),
    Enum(EnumDef),
    Tuple(Vec<TypeRef>),
    Newtype(TypeRef),
    Alias(TypeRef),
}

/// A named field of a struct or struct-variant.
///
/// `optional` is true when the Rust type is `Option<T>`. `undefined` is true
/// when the field is annotated `#[taut(undefined)]`, instructing codegen to
/// emit `T | undefined` rather than `T | null` (SPEC §3.1).
///
/// `constraints` carries the per-field validation vocabulary recorded by
/// `#[derive(Validate)]` (SPEC §7). The list is empty for fields that do not
/// participate in validation; older IR documents (pre-`v1`) that omit the
/// field deserialize as an empty vec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub ty: TypeRef,
    pub optional: bool,
    pub undefined: bool,
    pub doc: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
}

/// A discriminated-union enum (SPEC §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumDef {
    pub tag: String,
    pub variants: Vec<Variant>,
}

/// One variant of an [`EnumDef`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    pub payload: VariantPayload,
}

/// Payload shape of an enum variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum VariantPayload {
    Unit,
    Tuple(Vec<TypeRef>),
    Struct(Vec<Field>),
}

/// Reference to a type, either built-in or user-defined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TypeRef {
    Primitive(Primitive),
    Named(String),
    Option(Box<TypeRef>),
    Vec(Box<TypeRef>),
    Map {
        key: Box<TypeRef>,
        value: Box<TypeRef>,
    },
    Tuple(Vec<TypeRef>),
    FixedArray {
        elem: Box<TypeRef>,
        len: u64,
    },
}

/// Built-in primitive type. See SPEC §3.1 for the Rust → TypeScript mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primitive {
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    U128,
    I128,
    F32,
    F64,
    String,
    Bytes,
    Unit,
    DateTime,
    Uuid,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(ir: &Ir) -> Ir {
        let json = serde_json::to_string(ir).expect("serialize");
        serde_json::from_str(&json).expect("deserialize")
    }

    #[test]
    fn empty_ir_roundtrips() {
        let ir = Ir::empty();
        assert_eq!(ir.ir_version, Ir::CURRENT_VERSION);
        assert!(ir.procedures.is_empty());
        assert!(ir.types.is_empty());
        assert_eq!(roundtrip(&ir), ir);
    }

    #[test]
    fn query_with_string_input_and_user_output_roundtrips() {
        let ir = Ir {
            ir_version: Ir::CURRENT_VERSION,
            procedures: vec![Procedure {
                name: "get_user".to_string(),
                kind: ProcKind::Query,
                input: TypeRef::Primitive(Primitive::String),
                output: TypeRef::Named("User".to_string()),
                errors: vec![],
                http_method: HttpMethod::Post,
                doc: Some("Fetch a user by id.".to_string()),
            }],
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
                        name: "nickname".to_string(),
                        ty: TypeRef::Option(Box::new(TypeRef::Primitive(Primitive::String))),
                        optional: true,
                        undefined: false,
                        doc: None,
                        constraints: vec![],
                    },
                ]),
            }],
        };
        assert_eq!(roundtrip(&ir), ir);
    }

    #[test]
    fn discriminated_union_enum_roundtrips() {
        let ir = Ir {
            ir_version: Ir::CURRENT_VERSION,
            procedures: vec![],
            types: vec![TypeDef {
                name: "Event".to_string(),
                doc: Some("Server-emitted event variants.".to_string()),
                shape: TypeShape::Enum(EnumDef {
                    tag: "type".to_string(),
                    variants: vec![
                        Variant {
                            name: "Ping".to_string(),
                            payload: VariantPayload::Unit,
                        },
                        Variant {
                            name: "Message".to_string(),
                            payload: VariantPayload::Tuple(vec![TypeRef::Primitive(
                                Primitive::String,
                            )]),
                        },
                        Variant {
                            name: "User".to_string(),
                            payload: VariantPayload::Struct(vec![
                                Field {
                                    name: "id".to_string(),
                                    ty: TypeRef::Primitive(Primitive::Uuid),
                                    optional: false,
                                    undefined: false,
                                    doc: None,
                                    constraints: vec![],
                                },
                                Field {
                                    name: "name".to_string(),
                                    ty: TypeRef::Primitive(Primitive::String),
                                    optional: false,
                                    undefined: true,
                                    doc: None,
                                    constraints: vec![],
                                },
                            ]),
                        },
                    ],
                }),
            }],
        };
        assert_eq!(roundtrip(&ir), ir);
    }

    #[test]
    fn field_roundtrips_with_constraints() {
        // SPEC §7: a `#[derive(Validate)]` field records its per-field
        // constraint vocabulary into the IR. Verify that a non-empty
        // `constraints` vec survives a serde round-trip on the canonical
        // `Field` shape.
        let field = Field {
            name: "score".to_string(),
            ty: TypeRef::Primitive(Primitive::F64),
            optional: false,
            undefined: false,
            doc: None,
            constraints: vec![Constraint::Min(0.0), Constraint::Max(100.0)],
        };

        let json = serde_json::to_string(&field).expect("serialize Field");
        let back: Field = serde_json::from_str(&json).expect("deserialize Field");
        assert_eq!(back, field);
        assert_eq!(back.constraints.len(), 2);
        assert_eq!(back.constraints[0], Constraint::Min(0.0));
        assert_eq!(back.constraints[1], Constraint::Max(100.0));
    }
}

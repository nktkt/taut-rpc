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
    /// IR schema version. Codegen rejects mismatches against [`Ir::CURRENT_VERSION`].
    pub ir_version: u32,
    /// Every `#[rpc]` procedure visible to codegen.
    pub procedures: Vec<Procedure>,
    /// Every user-defined type reachable from a procedure signature.
    pub types: Vec<TypeDef>,
}

impl Ir {
    /// Current IR schema version. See SPEC §9 — codegen refuses mismatches.
    pub const CURRENT_VERSION: u32 = 1;

    /// Construct an empty IR document at the current schema version.
    #[must_use]
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
    /// Fully qualified procedure name as exposed to clients.
    pub name: String,
    /// Procedure flavour (query, mutation, or subscription).
    pub kind: ProcKind,
    /// Input argument type.
    pub input: TypeRef,
    /// Success output type.
    pub output: TypeRef,
    /// Error variants the procedure may return.
    pub errors: Vec<TypeRef>,
    /// HTTP method used to dispatch this procedure.
    pub http_method: HttpMethod,
    /// Doc comment harvested from the Rust source, if any.
    pub doc: Option<String>,
}

/// Procedure flavour. See SPEC §4 (wire format) and §6 (client API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcKind {
    /// Read-only request/response procedure.
    Query,
    /// State-mutating request/response procedure.
    Mutation,
    /// Long-lived stream of values pushed to the client.
    Subscription,
}

/// HTTP method used to dispatch the procedure (SPEC §4.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpMethod {
    /// `POST` — used for mutations and JSON-bodied queries.
    Post,
    /// `GET` — used for cacheable queries with query-string inputs.
    Get,
}

/// A user-defined type reachable from at least one procedure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeDef {
    /// Rust type name as it appears in the source.
    pub name: String,
    /// Doc comment harvested from the type definition, if any.
    pub doc: Option<String>,
    /// Structural shape of the type.
    pub shape: TypeShape,
}

/// The structural shape of a [`TypeDef`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TypeShape {
    /// Plain struct with named fields.
    Struct(Vec<Field>),
    /// Discriminated-union enum (SPEC §3.2).
    Enum(EnumDef),
    /// Tuple struct.
    Tuple(Vec<TypeRef>),
    /// Newtype wrapper around a single inner type.
    Newtype(TypeRef),
    /// Type alias to another type.
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
    /// Field name as written in Rust.
    pub name: String,
    /// Type of this field.
    pub ty: TypeRef,
    /// `true` if the Rust type is `Option<T>`.
    pub optional: bool,
    /// `true` if the field is annotated `#[taut(undefined)]` (emit `T | undefined`).
    pub undefined: bool,
    /// Doc comment harvested from the field, if any.
    pub doc: Option<String>,
    /// Per-field validation constraints recorded by `#[derive(Validate)]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
}

/// A discriminated-union enum (SPEC §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumDef {
    /// Discriminator field name used in the wire format (e.g. `"type"`).
    pub tag: String,
    /// Enum variants.
    pub variants: Vec<Variant>,
}

/// One variant of an [`EnumDef`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    /// Variant name as written in Rust.
    pub name: String,
    /// Payload shape carried by this variant.
    pub payload: VariantPayload,
}

/// Payload shape of an enum variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum VariantPayload {
    /// Unit variant — no payload.
    Unit,
    /// Tuple-style payload of positional types.
    Tuple(Vec<TypeRef>),
    /// Struct-style payload with named fields.
    Struct(Vec<Field>),
}

/// Reference to a type, either built-in or user-defined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TypeRef {
    /// Built-in primitive (numeric, string, etc.).
    Primitive(Primitive),
    /// Reference to a user-defined [`TypeDef`] by name.
    Named(String),
    /// `Option<T>`.
    Option(Box<TypeRef>),
    /// `Vec<T>`.
    Vec(Box<TypeRef>),
    /// Map with key and value types (e.g. `HashMap<K, V>`).
    Map {
        /// Key type.
        key: Box<TypeRef>,
        /// Value type.
        value: Box<TypeRef>,
    },
    /// Anonymous tuple type.
    Tuple(Vec<TypeRef>),
    /// Fixed-length array `[T; N]`.
    FixedArray {
        /// Element type.
        elem: Box<TypeRef>,
        /// Array length.
        len: u64,
    },
}

/// Built-in primitive type. See SPEC §3.1 for the Rust → TypeScript mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Primitive {
    /// `bool`.
    Bool,
    /// `u8`.
    U8,
    /// `u16`.
    U16,
    /// `u32`.
    U32,
    /// `u64`.
    U64,
    /// `i8`.
    I8,
    /// `i16`.
    I16,
    /// `i32`.
    I32,
    /// `i64`.
    I64,
    /// `u128`.
    U128,
    /// `i128`.
    I128,
    /// `f32`.
    F32,
    /// `f64`.
    F64,
    /// `String` / `&str`.
    String,
    /// Raw byte buffer (`Vec<u8>` or `Bytes`).
    Bytes,
    /// Unit type `()`.
    Unit,
    /// Date-time (e.g. `chrono::DateTime`).
    DateTime,
    /// `Uuid`.
    Uuid,
}

impl std::fmt::Display for Primitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Primitive::Bool => "bool",
            Primitive::U8 => "u8",
            Primitive::U16 => "u16",
            Primitive::U32 => "u32",
            Primitive::U64 => "u64",
            Primitive::I8 => "i8",
            Primitive::I16 => "i16",
            Primitive::I32 => "i32",
            Primitive::I64 => "i64",
            Primitive::U128 => "u128",
            Primitive::I128 => "i128",
            Primitive::F32 => "f32",
            Primitive::F64 => "f64",
            Primitive::String => "String",
            Primitive::Bytes => "Bytes",
            Primitive::Unit => "()",
            Primitive::DateTime => "DateTime",
            Primitive::Uuid => "Uuid",
        };
        f.write_str(s)
    }
}

impl std::fmt::Display for TypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeRef::Primitive(p) => write!(f, "{p}"),
            TypeRef::Named(name) => f.write_str(name),
            TypeRef::Vec(inner) => write!(f, "Vec<{inner}>"),
            TypeRef::Option(inner) => write!(f, "Option<{inner}>"),
            TypeRef::Map { key, value } => write!(f, "HashMap<{key}, {value}>"),
            TypeRef::Tuple(elems) => {
                f.write_str("(")?;
                for (i, t) in elems.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{t}")?;
                }
                f.write_str(")")
            }
            TypeRef::FixedArray { elem, len } => write!(f, "[{elem}; {len}]"),
        }
    }
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
    fn type_ref_display_renders_each_variant() {
        // Primitive
        assert_eq!(TypeRef::Primitive(Primitive::U64).to_string(), "u64");
        assert_eq!(TypeRef::Primitive(Primitive::Bool).to_string(), "bool");
        assert_eq!(TypeRef::Primitive(Primitive::String).to_string(), "String");
        assert_eq!(TypeRef::Primitive(Primitive::Unit).to_string(), "()");
        assert_eq!(TypeRef::Primitive(Primitive::Uuid).to_string(), "Uuid");

        // Named
        assert_eq!(TypeRef::Named("User".to_string()).to_string(), "User");

        // Vec
        assert_eq!(
            TypeRef::Vec(Box::new(TypeRef::Primitive(Primitive::U32))).to_string(),
            "Vec<u32>",
        );

        // Option
        assert_eq!(
            TypeRef::Option(Box::new(TypeRef::Named("User".to_string()))).to_string(),
            "Option<User>",
        );

        // Map
        assert_eq!(
            TypeRef::Map {
                key: Box::new(TypeRef::Primitive(Primitive::String)),
                value: Box::new(TypeRef::Primitive(Primitive::U64)),
            }
            .to_string(),
            "HashMap<String, u64>",
        );

        // Tuple
        assert_eq!(
            TypeRef::Tuple(vec![
                TypeRef::Primitive(Primitive::U64),
                TypeRef::Primitive(Primitive::String),
            ])
            .to_string(),
            "(u64, String)",
        );

        // FixedArray
        assert_eq!(
            TypeRef::FixedArray {
                elem: Box::new(TypeRef::Primitive(Primitive::U8)),
                len: 32,
            }
            .to_string(),
            "[u8; 32]",
        );

        // Nested: Vec<Option<HashMap<String, User>>>
        let nested = TypeRef::Vec(Box::new(TypeRef::Option(Box::new(TypeRef::Map {
            key: Box::new(TypeRef::Primitive(Primitive::String)),
            value: Box::new(TypeRef::Named("User".to_string())),
        }))));
        assert_eq!(nested.to_string(), "Vec<Option<HashMap<String, User>>>");
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

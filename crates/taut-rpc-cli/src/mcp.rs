//! MCP (Model Context Protocol) manifest emitter.
//!
//! Pure logic: takes an [`Ir`] document and renders a JSON manifest matching
//! the MCP `tools/list` response shape (spec 2025-06-18). The CLI's `mcp`
//! subcommand owns the I/O; this module owns the schema-building.
//!
//! Output shape:
//!
//! ```json
//! {
//!   "tools": [
//!     {
//!       "name": "<procedure-name>",
//!       "description": "<rustdoc string>",
//!       "inputSchema": {
//!         "type": "object",
//!         "properties": { "input": { ... } },
//!         "required": ["input"],
//!         "$defs": { ... }
//!       }
//!     }
//!   ]
//! }
//! ```
//!
//! The outer `inputSchema` is always wrapped as an object with a single
//! `input` field, mirroring taut-rpc's wire envelope (`{"input": <value>}`).
//! MCP requires `inputSchema.type == "object"`, so this wrapping is also
//! how we satisfy that constraint when a procedure's input is a primitive.
//!
//! Subscriptions are skipped by default — MCP tools are strictly
//! request/response. Pass `include_subscriptions: true` to surface them
//! anyway (their streaming nature is invisible at the manifest level).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};
use taut_rpc::ir::{
    EnumDef, Field, Ir, Primitive, ProcKind, Procedure, TypeDef, TypeRef, TypeShape, Variant,
    VariantPayload,
};
use taut_rpc::type_map::BigIntStrategy;

/// Per-invocation knobs for [`render_manifest`].
#[derive(Debug, Clone)]
pub struct McpOptions {
    /// How to render 64- and 128-bit integers in JSON Schema. `Native` emits
    /// `{"type": "integer"}`; `AsString` emits `{"type": "string"}`.
    pub bigint_strategy: BigIntStrategy,
    /// If true, also emit a tool entry per `Subscription` procedure. MCP
    /// itself does not model streaming results, so the manifest entry will
    /// look identical to a query — callers should be aware.
    pub include_subscriptions: bool,
}

impl Default for McpOptions {
    fn default() -> Self {
        Self {
            bigint_strategy: BigIntStrategy::Native,
            include_subscriptions: false,
        }
    }
}

/// Render an [`Ir`] into an MCP `tools/list` manifest as a JSON value.
///
/// The returned `serde_json::Value` is pretty-printable directly; callers
/// usually pipe it through [`serde_json::to_string_pretty`].
#[must_use]
pub fn render_manifest(ir: &Ir, opts: &McpOptions) -> Value {
    let type_index: BTreeMap<&str, &TypeDef> =
        ir.types.iter().map(|t| (t.name.as_str(), t)).collect();

    let mut tools: Vec<Value> = Vec::new();
    for proc in &ir.procedures {
        if matches!(proc.kind, ProcKind::Subscription) && !opts.include_subscriptions {
            continue;
        }
        tools.push(render_tool(proc, &type_index, opts));
    }

    json!({ "tools": tools })
}

fn render_tool(
    proc: &Procedure,
    type_index: &BTreeMap<&str, &TypeDef>,
    opts: &McpOptions,
) -> Value {
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    collect_named_refs(&proc.input, type_index, &mut reachable);

    let input_schema_inner = type_ref_to_schema(&proc.input, opts);

    let mut input_schema = Map::new();
    input_schema.insert("type".into(), Value::String("object".into()));

    let mut properties = Map::new();
    properties.insert("input".into(), input_schema_inner);
    input_schema.insert("properties".into(), Value::Object(properties));
    input_schema.insert(
        "required".into(),
        Value::Array(vec![Value::String("input".into())]),
    );

    if !reachable.is_empty() {
        let mut defs = Map::new();
        for name in &reachable {
            if let Some(td) = type_index.get(name.as_str()) {
                defs.insert(name.clone(), type_def_to_schema(td, opts));
            }
        }
        if !defs.is_empty() {
            input_schema.insert("$defs".into(), Value::Object(defs));
        }
    }

    let mut tool = Map::new();
    tool.insert("name".into(), Value::String(proc.name.clone()));
    if let Some(doc) = &proc.doc {
        tool.insert("description".into(), Value::String(doc.clone()));
    }
    tool.insert("inputSchema".into(), Value::Object(input_schema));
    Value::Object(tool)
}

// ---------------------------------------------------------------------------
// Reachability — walk a TypeRef collecting named types, transitively.
// ---------------------------------------------------------------------------

fn collect_named_refs(
    t: &TypeRef,
    type_index: &BTreeMap<&str, &TypeDef>,
    out: &mut BTreeSet<String>,
) {
    match t {
        TypeRef::Primitive(_) => {}
        TypeRef::Named(name) => {
            if !out.insert(name.clone()) {
                return;
            }
            if let Some(td) = type_index.get(name.as_str()) {
                collect_named_refs_in_shape(&td.shape, type_index, out);
            }
        }
        TypeRef::Option(inner) | TypeRef::Vec(inner) => {
            collect_named_refs(inner, type_index, out);
        }
        TypeRef::Map { key, value } => {
            collect_named_refs(key, type_index, out);
            collect_named_refs(value, type_index, out);
        }
        TypeRef::Tuple(elems) => {
            for e in elems {
                collect_named_refs(e, type_index, out);
            }
        }
        TypeRef::FixedArray { elem, .. } => {
            collect_named_refs(elem, type_index, out);
        }
    }
}

fn collect_named_refs_in_shape(
    shape: &TypeShape,
    type_index: &BTreeMap<&str, &TypeDef>,
    out: &mut BTreeSet<String>,
) {
    match shape {
        TypeShape::Struct(fields) => {
            for f in fields {
                collect_named_refs(&f.ty, type_index, out);
            }
        }
        TypeShape::Enum(EnumDef { variants, .. }) => {
            for v in variants {
                match &v.payload {
                    VariantPayload::Unit => {}
                    VariantPayload::Tuple(elems) => {
                        for e in elems {
                            collect_named_refs(e, type_index, out);
                        }
                    }
                    VariantPayload::Struct(fields) => {
                        for f in fields {
                            collect_named_refs(&f.ty, type_index, out);
                        }
                    }
                }
            }
        }
        TypeShape::Tuple(elems) => {
            for e in elems {
                collect_named_refs(e, type_index, out);
            }
        }
        TypeShape::Newtype(inner) | TypeShape::Alias(inner) => {
            collect_named_refs(inner, type_index, out);
        }
    }
}

// ---------------------------------------------------------------------------
// TypeRef → JSON Schema (Draft 2020-12 keywords).
// ---------------------------------------------------------------------------

fn type_ref_to_schema(t: &TypeRef, opts: &McpOptions) -> Value {
    match t {
        TypeRef::Primitive(p) => primitive_to_schema(*p, opts),
        TypeRef::Named(name) => json!({ "$ref": format!("#/$defs/{name}") }),
        TypeRef::Option(inner) => {
            // JSON Schema Draft 2020-12 allows `type` to be an array; for a
            // ref-typed inner we fall back to oneOf.
            let inner_schema = type_ref_to_schema(inner, opts);
            match inner_schema {
                Value::Object(mut map) => {
                    if let Some(Value::String(t)) = map.remove("type") {
                        map.insert(
                            "type".into(),
                            Value::Array(vec![Value::String(t), Value::String("null".into())]),
                        );
                        Value::Object(map)
                    } else {
                        // Re-insert what we took out; emit oneOf wrapper.
                        let inner = Value::Object(map);
                        json!({ "oneOf": [inner, { "type": "null" }] })
                    }
                }
                other => json!({ "oneOf": [other, { "type": "null" }] }),
            }
        }
        TypeRef::Vec(inner) => {
            json!({ "type": "array", "items": type_ref_to_schema(inner, opts) })
        }
        TypeRef::Map { key: _, value } => {
            // JSON object keys are always strings on the wire; the IR's `key`
            // is informational only at this layer.
            json!({
                "type": "object",
                "additionalProperties": type_ref_to_schema(value, opts),
            })
        }
        TypeRef::Tuple(elems) => {
            let prefix: Vec<Value> = elems.iter().map(|e| type_ref_to_schema(e, opts)).collect();
            let len = prefix.len();
            json!({
                "type": "array",
                "prefixItems": prefix,
                "items": false,
                "minItems": len,
                "maxItems": len,
            })
        }
        TypeRef::FixedArray { elem, len } => {
            json!({
                "type": "array",
                "items": type_ref_to_schema(elem, opts),
                "minItems": len,
                "maxItems": len,
            })
        }
    }
}

fn primitive_to_schema(p: Primitive, opts: &McpOptions) -> Value {
    match p {
        Primitive::Bool => json!({ "type": "boolean" }),
        Primitive::U8 => json!({ "type": "integer", "minimum": 0, "maximum": u8::MAX }),
        Primitive::U16 => json!({ "type": "integer", "minimum": 0, "maximum": u16::MAX }),
        Primitive::U32 => json!({ "type": "integer", "minimum": 0, "maximum": u32::MAX }),
        Primitive::I8 => {
            json!({ "type": "integer", "minimum": i8::MIN, "maximum": i8::MAX })
        }
        Primitive::I16 => {
            json!({ "type": "integer", "minimum": i16::MIN, "maximum": i16::MAX })
        }
        Primitive::I32 => {
            json!({ "type": "integer", "minimum": i32::MIN, "maximum": i32::MAX })
        }
        Primitive::U64 | Primitive::I64 | Primitive::U128 | Primitive::I128 => {
            match opts.bigint_strategy {
                BigIntStrategy::Native => json!({ "type": "integer" }),
                BigIntStrategy::AsString => {
                    json!({ "type": "string", "pattern": "^-?\\d+$" })
                }
            }
        }
        Primitive::F32 | Primitive::F64 => json!({ "type": "number" }),
        Primitive::String => json!({ "type": "string" }),
        Primitive::Bytes => json!({ "type": "string", "contentEncoding": "base64" }),
        Primitive::Unit => json!({ "type": "null" }),
        Primitive::DateTime => json!({ "type": "string", "format": "date-time" }),
        Primitive::Uuid => json!({ "type": "string", "format": "uuid" }),
    }
}

// ---------------------------------------------------------------------------
// TypeDef → JSON Schema (used to populate `$defs`).
// ---------------------------------------------------------------------------

fn type_def_to_schema(td: &TypeDef, opts: &McpOptions) -> Value {
    let body = match &td.shape {
        TypeShape::Struct(fields) => struct_to_schema(fields, opts),
        TypeShape::Enum(e) => enum_to_schema(e, opts),
        TypeShape::Tuple(elems) => {
            let prefix: Vec<Value> = elems.iter().map(|e| type_ref_to_schema(e, opts)).collect();
            let len = prefix.len();
            json!({
                "type": "array",
                "prefixItems": prefix,
                "items": false,
                "minItems": len,
                "maxItems": len,
            })
        }
        TypeShape::Newtype(inner) | TypeShape::Alias(inner) => type_ref_to_schema(inner, opts),
    };

    let Value::Object(mut map) = body else {
        return body;
    };
    if let Some(doc) = &td.doc {
        map.insert("description".into(), Value::String(doc.clone()));
    }
    Value::Object(map)
}

fn struct_to_schema(fields: &[Field], opts: &McpOptions) -> Value {
    let mut properties = Map::new();
    let mut required: Vec<Value> = Vec::new();
    for f in fields {
        let mut field_schema = type_ref_to_schema(&f.ty, opts);
        if let (Some(doc), Value::Object(ref mut m)) = (f.doc.as_ref(), &mut field_schema) {
            m.insert("description".into(), Value::String(doc.clone()));
        }
        properties.insert(f.name.clone(), field_schema);
        if !f.optional {
            required.push(Value::String(f.name.clone()));
        }
    }
    let mut out = Map::new();
    out.insert("type".into(), Value::String("object".into()));
    out.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        out.insert("required".into(), Value::Array(required));
    }
    out.insert("additionalProperties".into(), Value::Bool(false));
    Value::Object(out)
}

fn enum_to_schema(e: &EnumDef, opts: &McpOptions) -> Value {
    let variants: Vec<Value> = e
        .variants
        .iter()
        .map(|v| variant_to_schema(&e.tag, v, opts))
        .collect();
    json!({ "oneOf": variants })
}

fn variant_to_schema(tag: &str, v: &Variant, opts: &McpOptions) -> Value {
    let mut props = Map::new();
    props.insert(
        tag.to_string(),
        json!({ "type": "string", "const": v.name }),
    );
    let mut required = vec![Value::String(tag.to_string())];

    match &v.payload {
        VariantPayload::Unit => {}
        VariantPayload::Tuple(elems) => {
            let prefix: Vec<Value> = elems.iter().map(|e| type_ref_to_schema(e, opts)).collect();
            let len = prefix.len();
            props.insert(
                "payload".into(),
                json!({
                    "type": "array",
                    "prefixItems": prefix,
                    "items": false,
                    "minItems": len,
                    "maxItems": len,
                }),
            );
            required.push(Value::String("payload".into()));
        }
        VariantPayload::Struct(fields) => {
            for f in fields {
                let mut field_schema = type_ref_to_schema(&f.ty, opts);
                if let (Some(doc), Value::Object(ref mut m)) = (f.doc.as_ref(), &mut field_schema) {
                    m.insert("description".into(), Value::String(doc.clone()));
                }
                props.insert(f.name.clone(), field_schema);
                if !f.optional {
                    required.push(Value::String(f.name.clone()));
                }
            }
        }
    }

    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use taut_rpc::ir::{HttpMethod, Procedure};

    fn empty_ir() -> Ir {
        Ir::empty()
    }

    fn proc(name: &str, kind: ProcKind, input: TypeRef, doc: Option<&str>) -> Procedure {
        Procedure {
            name: name.into(),
            kind,
            input,
            output: TypeRef::Primitive(Primitive::Unit),
            errors: vec![],
            http_method: HttpMethod::Post,
            doc: doc.map(String::from),
        }
    }

    #[test]
    fn empty_ir_emits_empty_tools_array() {
        let v = render_manifest(&empty_ir(), &McpOptions::default());
        assert_eq!(v, json!({ "tools": [] }));
    }

    #[test]
    fn primitive_input_is_wrapped_in_object_with_input_field() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "ping",
                ProcKind::Query,
                TypeRef::Primitive(Primitive::String),
                Some("Heartbeat."),
            )],
            types: vec![],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        assert_eq!(
            v,
            json!({
                "tools": [{
                    "name": "ping",
                    "description": "Heartbeat.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "input": { "type": "string" } },
                        "required": ["input"],
                    }
                }]
            })
        );
    }

    #[test]
    fn missing_doc_omits_description_field() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "noop",
                ProcKind::Query,
                TypeRef::Primitive(Primitive::Unit),
                None,
            )],
            types: vec![],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let tool = &v["tools"][0];
        assert!(tool.get("description").is_none(), "got: {tool}");
        assert_eq!(tool["name"], "noop");
    }

    #[test]
    fn subscriptions_are_skipped_by_default() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![
                proc(
                    "q",
                    ProcKind::Query,
                    TypeRef::Primitive(Primitive::Unit),
                    None,
                ),
                proc(
                    "ticks",
                    ProcKind::Subscription,
                    TypeRef::Primitive(Primitive::Unit),
                    None,
                ),
            ],
            types: vec![],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let tools = v["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "q");
    }

    #[test]
    fn subscriptions_included_when_flag_is_set() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![
                proc(
                    "q",
                    ProcKind::Query,
                    TypeRef::Primitive(Primitive::Unit),
                    None,
                ),
                proc(
                    "ticks",
                    ProcKind::Subscription,
                    TypeRef::Primitive(Primitive::Unit),
                    None,
                ),
            ],
            types: vec![],
        };
        let opts = McpOptions {
            include_subscriptions: true,
            ..McpOptions::default()
        };
        let v = render_manifest(&ir, &opts);
        let tools = v["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn struct_input_emits_defs_with_ref() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "get_user",
                ProcKind::Query,
                TypeRef::Named("UserId".into()),
                None,
            )],
            types: vec![TypeDef {
                name: "UserId".into(),
                doc: Some("Opaque user id.".into()),
                shape: TypeShape::Struct(vec![Field {
                    name: "raw".into(),
                    ty: TypeRef::Primitive(Primitive::Uuid),
                    optional: false,
                    undefined: false,
                    doc: None,
                    constraints: vec![],
                }]),
            }],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let schema = &v["tools"][0]["inputSchema"];
        assert_eq!(
            schema["properties"]["input"],
            json!({ "$ref": "#/$defs/UserId" })
        );
        let user_id = &schema["$defs"]["UserId"];
        assert_eq!(user_id["type"], "object");
        assert_eq!(user_id["description"], "Opaque user id.");
        assert_eq!(user_id["required"], json!(["raw"]));
        assert_eq!(user_id["additionalProperties"], json!(false));
        assert_eq!(
            user_id["properties"]["raw"],
            json!({ "type": "string", "format": "uuid" })
        );
    }

    #[test]
    fn optional_field_is_not_in_required_and_gets_nullable_type() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "f",
                ProcKind::Query,
                TypeRef::Named("Args".into()),
                None,
            )],
            types: vec![TypeDef {
                name: "Args".into(),
                doc: None,
                shape: TypeShape::Struct(vec![
                    Field {
                        name: "id".into(),
                        ty: TypeRef::Primitive(Primitive::U32),
                        optional: false,
                        undefined: false,
                        doc: None,
                        constraints: vec![],
                    },
                    Field {
                        name: "nickname".into(),
                        ty: TypeRef::Option(Box::new(TypeRef::Primitive(Primitive::String))),
                        optional: true,
                        undefined: false,
                        doc: None,
                        constraints: vec![],
                    },
                ]),
            }],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let args = &v["tools"][0]["inputSchema"]["$defs"]["Args"];
        assert_eq!(args["required"], json!(["id"]));
        assert_eq!(
            args["properties"]["nickname"]["type"],
            json!(["string", "null"])
        );
    }

    #[test]
    fn enum_emits_oneof_with_const_tag() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc("f", ProcKind::Query, TypeRef::Named("E".into()), None)],
            types: vec![TypeDef {
                name: "E".into(),
                doc: None,
                shape: TypeShape::Enum(EnumDef {
                    tag: "type".into(),
                    variants: vec![
                        Variant {
                            name: "Ping".into(),
                            payload: VariantPayload::Unit,
                        },
                        Variant {
                            name: "Msg".into(),
                            payload: VariantPayload::Struct(vec![Field {
                                name: "text".into(),
                                ty: TypeRef::Primitive(Primitive::String),
                                optional: false,
                                undefined: false,
                                doc: None,
                                constraints: vec![],
                            }]),
                        },
                    ],
                }),
            }],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let e = &v["tools"][0]["inputSchema"]["$defs"]["E"];
        let variants = e["oneOf"].as_array().unwrap();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0]["properties"]["type"]["const"], "Ping");
        assert_eq!(variants[1]["properties"]["type"]["const"], "Msg");
        assert!(variants[1]["properties"]
            .as_object()
            .unwrap()
            .contains_key("text"));
    }

    #[test]
    fn vec_emits_array_with_items() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "f",
                ProcKind::Query,
                TypeRef::Vec(Box::new(TypeRef::Primitive(Primitive::U32))),
                None,
            )],
            types: vec![],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        assert_eq!(
            v["tools"][0]["inputSchema"]["properties"]["input"]["type"],
            "array"
        );
        assert_eq!(
            v["tools"][0]["inputSchema"]["properties"]["input"]["items"]["type"],
            "integer"
        );
    }

    #[test]
    fn tuple_emits_prefix_items_with_minmax_len() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "f",
                ProcKind::Query,
                TypeRef::Tuple(vec![
                    TypeRef::Primitive(Primitive::String),
                    TypeRef::Primitive(Primitive::U32),
                ]),
                None,
            )],
            types: vec![],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let inp = &v["tools"][0]["inputSchema"]["properties"]["input"];
        assert_eq!(inp["type"], "array");
        assert_eq!(inp["minItems"], 2);
        assert_eq!(inp["maxItems"], 2);
        assert_eq!(inp["prefixItems"][0]["type"], "string");
        assert_eq!(inp["prefixItems"][1]["type"], "integer");
    }

    #[test]
    fn fixed_array_constrains_min_max() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "f",
                ProcKind::Query,
                TypeRef::FixedArray {
                    elem: Box::new(TypeRef::Primitive(Primitive::U8)),
                    len: 16,
                },
                None,
            )],
            types: vec![],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let inp = &v["tools"][0]["inputSchema"]["properties"]["input"];
        assert_eq!(inp["minItems"], 16);
        assert_eq!(inp["maxItems"], 16);
        assert_eq!(inp["items"]["type"], "integer");
    }

    #[test]
    fn map_emits_additional_properties() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "f",
                ProcKind::Query,
                TypeRef::Map {
                    key: Box::new(TypeRef::Primitive(Primitive::String)),
                    value: Box::new(TypeRef::Primitive(Primitive::U32)),
                },
                None,
            )],
            types: vec![],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let inp = &v["tools"][0]["inputSchema"]["properties"]["input"];
        assert_eq!(inp["type"], "object");
        assert_eq!(inp["additionalProperties"]["type"], "integer");
    }

    #[test]
    fn bigint_strategy_native_emits_integer() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "f",
                ProcKind::Query,
                TypeRef::Primitive(Primitive::U64),
                None,
            )],
            types: vec![],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        assert_eq!(
            v["tools"][0]["inputSchema"]["properties"]["input"]["type"],
            "integer"
        );
    }

    #[test]
    fn bigint_strategy_as_string_emits_string_with_pattern() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "f",
                ProcKind::Query,
                TypeRef::Primitive(Primitive::I64),
                None,
            )],
            types: vec![],
        };
        let opts = McpOptions {
            bigint_strategy: BigIntStrategy::AsString,
            ..McpOptions::default()
        };
        let v = render_manifest(&ir, &opts);
        let inp = &v["tools"][0]["inputSchema"]["properties"]["input"];
        assert_eq!(inp["type"], "string");
        assert_eq!(inp["pattern"], "^-?\\d+$");
    }

    #[test]
    fn nested_named_types_are_pulled_into_defs() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "f",
                ProcKind::Query,
                TypeRef::Named("Outer".into()),
                None,
            )],
            types: vec![
                TypeDef {
                    name: "Outer".into(),
                    doc: None,
                    shape: TypeShape::Struct(vec![Field {
                        name: "inner".into(),
                        ty: TypeRef::Named("Inner".into()),
                        optional: false,
                        undefined: false,
                        doc: None,
                        constraints: vec![],
                    }]),
                },
                TypeDef {
                    name: "Inner".into(),
                    doc: None,
                    shape: TypeShape::Struct(vec![Field {
                        name: "x".into(),
                        ty: TypeRef::Primitive(Primitive::U32),
                        optional: false,
                        undefined: false,
                        doc: None,
                        constraints: vec![],
                    }]),
                },
                TypeDef {
                    name: "Unused".into(),
                    doc: None,
                    shape: TypeShape::Struct(vec![]),
                },
            ],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let defs = &v["tools"][0]["inputSchema"]["$defs"];
        assert!(defs.get("Outer").is_some());
        assert!(defs.get("Inner").is_some());
        assert!(defs.get("Unused").is_none());
    }

    #[test]
    fn recursive_named_type_terminates() {
        let ir = Ir {
            ir_version: 0,
            procedures: vec![proc(
                "f",
                ProcKind::Query,
                TypeRef::Named("Tree".into()),
                None,
            )],
            types: vec![TypeDef {
                name: "Tree".into(),
                doc: None,
                shape: TypeShape::Struct(vec![Field {
                    name: "children".into(),
                    ty: TypeRef::Vec(Box::new(TypeRef::Named("Tree".into()))),
                    optional: false,
                    undefined: false,
                    doc: None,
                    constraints: vec![],
                }]),
            }],
        };
        let v = render_manifest(&ir, &McpOptions::default());
        let tree = &v["tools"][0]["inputSchema"]["$defs"]["Tree"];
        assert_eq!(tree["properties"]["children"]["type"], "array");
        assert_eq!(
            tree["properties"]["children"]["items"],
            json!({ "$ref": "#/$defs/Tree" })
        );
    }

    #[test]
    fn manifest_top_level_has_only_tools_field() {
        let v = render_manifest(&empty_ir(), &McpOptions::default());
        let map = v.as_object().unwrap();
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("tools"));
    }
}

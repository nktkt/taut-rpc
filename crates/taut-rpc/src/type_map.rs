//! Rust → TypeScript type mapping. See SPEC §3.
//!
//! This module renders a [`TypeRef`] from the IR into a TypeScript type
//! expression string. It is purely syntactic; it does not validate that
//! named types exist.
//!
//! The mapping table is the spec — see `SPEC.md` §3.1. Anything not
//! covered there should be added to the spec first, then mirrored here.

use crate::ir::{Primitive, TypeRef};

/// Maximum length of a [`TypeRef::FixedArray`] that we render as a TS
/// tuple type (`[T, T, ...]`). Beyond this we fall back to `T[]` to
/// avoid generating absurdly large tuple types that drag on TS perf.
const FIXED_ARRAY_TUPLE_CAP: usize = 16;

/// How `u64`/`i64`/`u128`/`i128` are emitted in TypeScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BigIntStrategy {
    /// Emit as `bigint` (default per SPEC §3.1).
    Native,
    /// Emit as `string` (configurable, see SPEC §3.1).
    AsString,
}

/// Knobs for [`render_type`] / [`render_primitive`].
#[derive(Debug, Clone)]
pub struct Options {
    /// How to emit 64- and 128-bit integers.
    pub bigint: BigIntStrategy,
    /// If true, use `T | undefined` for optional fields with
    /// `#[taut(undefined)]` hints. (The hint propagation is the caller's
    /// responsibility; this flag just gates whether we honor it at all.)
    pub honor_undefined: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            bigint: BigIntStrategy::Native,
            honor_undefined: true,
        }
    }
}

/// Render a [`TypeRef`] as a TypeScript type expression.
///
/// Examples (with default [`Options`]):
///
/// - `Primitive(Bool)` → `"boolean"`
/// - `Named("User")` → `"User"`
/// - `Vec<i32>` → `"number[]"`
/// - `Option<String>` → `"string | null"`
/// - `HashMap<String, User>` → `"Record<string, User>"`
/// - `(i32, String)` → `"[number, string]"`
#[must_use]
pub fn render_type(t: &TypeRef, opts: &Options) -> String {
    match t {
        TypeRef::Primitive(p) => render_primitive(*p, opts).to_string(),
        TypeRef::Named(name) => name.clone(),
        TypeRef::Option(inner) => render_option(inner, opts),
        TypeRef::Vec(inner) => render_vec_like(inner, opts),
        TypeRef::Map { key, value } => render_map(key, value, opts),
        TypeRef::Tuple(elems) => render_tuple(elems, opts),
        TypeRef::FixedArray { elem, len } => {
            render_fixed_array(elem, usize::try_from(*len).unwrap_or(usize::MAX), opts)
        }
    }
}

/// Render a [`Primitive`] as a TypeScript type expression.
///
/// Returns `&'static str` because the result is always one of a fixed
/// set of TS keywords / built-ins.
#[must_use]
#[allow(clippy::match_same_arms)] // arms kept distinct so per-variant SPEC comments stay attached
pub fn render_primitive(p: Primitive, opts: &Options) -> &'static str {
    use Primitive::{
        Bool, Bytes, DateTime, String, Unit, Uuid, F32, F64, I128, I16, I32, I64, I8, U128, U16,
        U32, U64, U8,
    };
    match p {
        Bool => "boolean",
        U8 | U16 | U32 | I8 | I16 | I32 | F32 | F64 => "number",
        U64 | I64 | U128 | I128 => match opts.bigint {
            BigIntStrategy::Native => "bigint",
            BigIntStrategy::AsString => "string",
        },
        String => "string",
        // SPEC §3.1: bytes go on the wire as base64. TODO: brand or wrap
        // in a `Bytes` nominal type once the runtime helper lands.
        Bytes => "string",
        Unit => "void",
        // SPEC §3.1: chrono / time render as ISO-8601 strings.
        DateTime => "string",
        // SPEC §3.1: Uuid is "ideally branded" — for v0 it's a plain
        // string. TODO: emit a `Uuid` brand type alias.
        Uuid => "string",
    }
}

/// `Option<T>` → `T | null`. Collapses nested options:
/// `Option<Option<T>>` → `T | null` (idempotent rather than
/// `T | null | null`).
fn render_option(inner: &TypeRef, opts: &Options) -> String {
    // Peel nested Options so the rendered output stays sane.
    let mut cur = inner;
    while let TypeRef::Option(next) = cur {
        cur = next;
    }
    let rendered = render_type(cur, opts);
    // Wrap union-bearing inners in parens: `(A | B) | null`. A bare
    // `Named` / `Primitive` / array / Record / tuple does not need
    // parens.
    if needs_parens_for_union(cur) {
        format!("({rendered}) | null")
    } else {
        format!("{rendered} | null")
    }
}

/// `Vec<T>` (and `&[T]`) → `T[]`.
///
/// If the element type itself contains a top-level union (currently
/// only `Option<T>`), we wrap it in parens so the result parses as
/// `(T | null)[]` rather than the ambiguous `T | null[]`.
fn render_vec_like(inner: &TypeRef, opts: &Options) -> String {
    let rendered = render_type(inner, opts);
    if needs_parens_for_array(inner) {
        format!("({rendered})[]")
    } else {
        format!("{rendered}[]")
    }
}

/// `HashMap<K, V>` → `Record<string, V>` if `K` is `string`-shaped,
/// otherwise `Array<[K, V]>` per SPEC §3.1.
fn render_map(key: &TypeRef, value: &TypeRef, opts: &Options) -> String {
    let v = render_type(value, opts);
    if is_string_keyed(key) {
        format!("Record<string, {v}>")
    } else {
        let k = render_type(key, opts);
        format!("Array<[{k}, {v}]>")
    }
}

/// `(T1, T2, ...)` → `[T1, T2, ...]`. Empty tuple → `void`.
fn render_tuple(elems: &[TypeRef], opts: &Options) -> String {
    if elems.is_empty() {
        return "void".to_string();
    }
    let inner: Vec<String> = elems.iter().map(|e| render_type(e, opts)).collect();
    format!("[{}]", inner.join(", "))
}

/// `[T; N]` → `[T, T, ..., T]` (N entries) if `N <= FIXED_ARRAY_TUPLE_CAP`.
/// For larger N we fall back to `T[]` and embed a TODO marker so the
/// generated `.ts` is still valid but flags the precision loss.
fn render_fixed_array(elem: &TypeRef, len: usize, opts: &Options) -> String {
    let rendered = render_type(elem, opts);
    if len == 0 {
        // `[T; 0]` is degenerate but legal in Rust; TS empty tuple is `[]`.
        return "[]".to_string();
    }
    if len <= FIXED_ARRAY_TUPLE_CAP {
        let parts: Vec<&str> = (0..len).map(|_| rendered.as_str()).collect();
        return format!("[{}]", parts.join(", "));
    }
    // Fallback: too long for a tuple. Keep it as an array and leave a
    // breadcrumb in the rendered output so reviewers notice.
    if needs_parens_for_array(elem) {
        format!("/* TODO: fixed-size [{rendered}; {len}] */ ({rendered})[]")
    } else {
        format!("/* TODO: fixed-size [{rendered}; {len}] */ {rendered}[]")
    }
}

/// Whether the rendered form of `t` is a union that needs parens when
/// composed into another union (e.g. nesting under `| null`).
fn needs_parens_for_union(t: &TypeRef) -> bool {
    matches!(t, TypeRef::Option(_))
}

/// Whether the rendered form of `t` needs parens before `[]` to parse
/// correctly as an array of that thing.
fn needs_parens_for_array(t: &TypeRef) -> bool {
    matches!(t, TypeRef::Option(_))
}

/// A "string-keyed" map renders as `Record<string, V>`. We accept the
/// obvious primitive `String` plus the conventional brand-y named types
/// (`Uuid`, `DateTime`) which serialize as strings.
fn is_string_keyed(key: &TypeRef) -> bool {
    matches!(
        key,
        TypeRef::Primitive(Primitive::String | Primitive::Uuid | Primitive::DateTime)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Primitive::*;
    use crate::ir::{Primitive, TypeRef};

    fn r(t: &TypeRef) -> std::string::String {
        render_type(t, &Options::default())
    }

    fn r_opts(t: &TypeRef, opts: &Options) -> std::string::String {
        render_type(t, opts)
    }

    fn prim(p: Primitive) -> TypeRef {
        TypeRef::Primitive(p)
    }

    // ---- primitives, default options -----------------------------------

    #[test]
    fn primitive_bool() {
        assert_eq!(r(&prim(Bool)), "boolean");
    }

    #[test]
    fn primitive_small_ints_and_floats_are_number() {
        for p in [U8, U16, U32, I8, I16, I32, F32, F64] {
            assert_eq!(render_primitive(p, &Options::default()), "number", "{p:?}");
        }
    }

    #[test]
    fn primitive_big_ints_default_to_bigint() {
        for p in [U64, I64, U128, I128] {
            assert_eq!(render_primitive(p, &Options::default()), "bigint", "{p:?}");
        }
    }

    #[test]
    fn primitive_string_unit_bytes_datetime_uuid() {
        let opts = Options::default();
        assert_eq!(render_primitive(String, &opts), "string");
        assert_eq!(render_primitive(Unit, &opts), "void");
        assert_eq!(render_primitive(Bytes, &opts), "string");
        assert_eq!(render_primitive(DateTime, &opts), "string");
        assert_eq!(render_primitive(Uuid, &opts), "string");
    }

    // ---- bigint strategy -----------------------------------------------

    #[test]
    fn bigint_as_string_mode_for_64_and_128_bit_ints() {
        let opts = Options {
            bigint: BigIntStrategy::AsString,
            honor_undefined: true,
        };
        for p in [U64, I64, U128, I128] {
            assert_eq!(render_primitive(p, &opts), "string", "{p:?}");
        }
        // Smaller ints stay `number`.
        assert_eq!(render_primitive(U32, &opts), "number");
        assert_eq!(render_primitive(I32, &opts), "number");
    }

    // ---- Option / Vec composition --------------------------------------

    #[test]
    fn option_string_renders_as_string_pipe_null() {
        let t = TypeRef::Option(Box::new(prim(String)));
        assert_eq!(r(&t), "string | null");
    }

    #[test]
    fn vec_u32_renders_as_number_array() {
        let t = TypeRef::Vec(Box::new(prim(U32)));
        assert_eq!(r(&t), "number[]");
    }

    #[test]
    fn option_vec_string_renders_as_string_array_pipe_null() {
        // Option<Vec<String>> → string[] | null. The inner Vec doesn't
        // need parens because `T[] | null` parses unambiguously.
        let t = TypeRef::Option(Box::new(TypeRef::Vec(Box::new(prim(String)))));
        assert_eq!(r(&t), "string[] | null");
    }

    #[test]
    fn vec_option_string_parenthesizes_the_union() {
        // Vec<Option<String>> → (string | null)[].
        let t = TypeRef::Vec(Box::new(TypeRef::Option(Box::new(prim(String)))));
        assert_eq!(r(&t), "(string | null)[]");
    }

    #[test]
    fn nested_option_collapses() {
        // Option<Option<T>> → T | null (not `T | null | null`).
        let t = TypeRef::Option(Box::new(TypeRef::Option(Box::new(prim(String)))));
        assert_eq!(r(&t), "string | null");
    }

    // ---- HashMap -------------------------------------------------------

    #[test]
    fn hashmap_string_user_is_record() {
        let t = TypeRef::Map {
            key: Box::new(prim(String)),
            value: Box::new(TypeRef::Named("User".into())),
        };
        assert_eq!(r(&t), "Record<string, User>");
    }

    #[test]
    fn hashmap_u64_user_is_array_of_pairs() {
        let t = TypeRef::Map {
            key: Box::new(prim(U64)),
            value: Box::new(TypeRef::Named("User".into())),
        };
        assert_eq!(r(&t), "Array<[bigint, User]>");
    }

    // ---- Tuple ---------------------------------------------------------

    #[test]
    fn empty_tuple_is_void() {
        let t = TypeRef::Tuple(vec![]);
        assert_eq!(r(&t), "void");
    }

    #[test]
    fn tuple_i32_string() {
        let t = TypeRef::Tuple(vec![prim(I32), prim(String)]);
        assert_eq!(r(&t), "[number, string]");
    }

    // ---- FixedArray ----------------------------------------------------

    #[test]
    fn fixed_array_short_becomes_tuple() {
        // [u8; 3] → [number, number, number].
        let t = TypeRef::FixedArray {
            elem: Box::new(prim(U8)),
            len: 3,
        };
        assert_eq!(r(&t), "[number, number, number]");
    }

    #[test]
    fn fixed_array_long_falls_back_to_array_with_todo() {
        // [u8; 32] → fallback. We don't pin the exact comment text, but
        // we do require the rendered output to (a) be a valid TS array
        // type and (b) carry a TODO breadcrumb mentioning the length.
        let t = TypeRef::FixedArray {
            elem: Box::new(prim(U8)),
            len: 32,
        };
        let rendered = r(&t);
        assert!(rendered.contains("number[]"), "got: {rendered}");
        assert!(rendered.contains("TODO"), "got: {rendered}");
        assert!(rendered.contains("32"), "got: {rendered}");
    }

    // ---- Named ---------------------------------------------------------

    #[test]
    fn named_type_passes_through_verbatim() {
        let t = TypeRef::Named("User".into());
        assert_eq!(r(&t), "User");
    }

    // ---- Spot-check: bigint AsString flows through render_type ---------

    #[test]
    fn render_type_honors_bigint_as_string_for_composites() {
        let opts = Options {
            bigint: BigIntStrategy::AsString,
            honor_undefined: true,
        };
        let t = TypeRef::Vec(Box::new(prim(U64)));
        assert_eq!(r_opts(&t, &opts), "string[]");
    }
}

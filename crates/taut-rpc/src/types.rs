//! Bridge between Rust types and the taut-rpc IR.
//!
//! [`TautType`] is implemented for every Rust type that can appear in an
//! `#[rpc]` procedure signature or a `#[derive(Type)]`-decorated user type.
//! It exposes three pieces of information to the proc-macro and to runtime
//! reflection:
//!
//! - [`TautType::ir_type_ref`] — how this type is *referenced* from another
//!   type or procedure signature (e.g. `Primitive(U64)`, `Named("User")`,
//!   `Option<...>`, `Vec<...>`).
//! - [`TautType::ir_type_def`] — the *definition* of this type when it is a
//!   user-defined named type. Primitives and built-in containers return
//!   `None`; only types produced by `#[derive(Type)]` return `Some(TypeDef)`.
//! - [`TautType::collect_type_defs`] — walks every transitive `TypeDef`
//!   reachable from `Self` into the supplied vector. The default
//!   implementation pushes only `Self`'s own def (if any). Composite types
//!   (`Option<T>`, `Vec<T>`, tuples, …) override it to recurse into their
//!   element types, so the proc-macro can collect every named type a
//!   procedure transitively touches with a single entry-point call.
//!
//! This module ships built-in impls for primitives (`bool`, the integer and
//! float families, `String`, `&'static str`, `char`, `()`) and the standard
//! generic containers (`Option<T>`, `Vec<T>`, `Box<T>`, `[T; N]`, tuples up
//! to arity 4, `HashMap<K, V>`). User-defined types acquire their `TautType`
//! impl via the `#[derive(Type)]` proc-macro.
//!
//! Optional, feature-gated impls cover the well-known external types listed
//! in SPEC §3.1: `uuid::Uuid` (feature `uuid`) and
//! `chrono::DateTime<chrono::Utc>` (feature `chrono`).
//!
//! See SPEC §3 for the full Rust → TypeScript mapping.

use crate::ir::{Primitive, TypeDef, TypeRef};

/// The bridge trait between a Rust type and its IR representation.
///
/// See the [module docs](self) for an overview. Implementations of this trait
/// are the contract every other Phase 1 component (proc-macro, router,
/// codegen) depends on; the trait shape itself is fixed for v0.
pub trait TautType {
    /// How this type is referenced from another type's field, a procedure
    /// signature, or a container's type parameter.
    fn ir_type_ref() -> TypeRef;

    /// The full definition of this type, if it is a user-defined named type.
    ///
    /// Primitives and built-in containers return `None`. Types emitted by
    /// `#[derive(Type)]` return `Some(TypeDef)` describing their structure.
    fn ir_type_def() -> Option<TypeDef> {
        None
    }

    /// Walks transitive type defs reachable from `Self` into `out`.
    ///
    /// The default pushes `Self`'s own def (if any) and stops. Composite
    /// types (`Vec<T>`, `Option<T>`, `HashMap<K, V>`, tuples, …) override
    /// this to additionally recurse into their element types so codegen can
    /// reach every named type a procedure transitively touches with a
    /// single call.
    fn collect_type_defs(out: &mut Vec<TypeDef>) {
        if let Some(d) = Self::ir_type_def() {
            out.push(d);
        }
    }
}

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

macro_rules! impl_primitive {
    ($rust:ty, $variant:ident) => {
        impl TautType for $rust {
            fn ir_type_ref() -> TypeRef {
                TypeRef::Primitive(Primitive::$variant)
            }
        }
    };
}

impl_primitive!(bool, Bool);
impl_primitive!(u8, U8);
impl_primitive!(u16, U16);
impl_primitive!(u32, U32);
impl_primitive!(u64, U64);
impl_primitive!(u128, U128);
impl_primitive!(i8, I8);
impl_primitive!(i16, I16);
impl_primitive!(i32, I32);
impl_primitive!(i64, I64);
impl_primitive!(i128, I128);
impl_primitive!(f32, F32);
impl_primitive!(f64, F64);
impl_primitive!(String, String);
impl_primitive!((), Unit);

// `&'static str` is IR-identical to `String`. Useful for literal returns
// like `async fn ping() -> &'static str { "pong" }` (SPEC §5).
impl TautType for &'static str {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Primitive(Primitive::String)
    }
}

// `char` becomes a single-character TypeScript string. There is no separate
// IR primitive — it shares the `String` shape. JSON has no char type either,
// so this is the natural mapping.
impl TautType for char {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Primitive(Primitive::String)
    }
}

// ---------------------------------------------------------------------------
// Container types
// ---------------------------------------------------------------------------

impl<T: TautType> TautType for Option<T> {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Option(Box::new(T::ir_type_ref()))
    }

    fn collect_type_defs(out: &mut Vec<TypeDef>) {
        T::collect_type_defs(out);
    }
}

impl<T: TautType> TautType for Vec<T> {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Vec(Box::new(T::ir_type_ref()))
    }

    fn collect_type_defs(out: &mut Vec<TypeDef>) {
        T::collect_type_defs(out);
    }
}

impl<T: TautType, const N: usize> TautType for [T; N] {
    fn ir_type_ref() -> TypeRef {
        TypeRef::FixedArray {
            elem: Box::new(T::ir_type_ref()),
            len: N as u64,
        }
    }

    fn collect_type_defs(out: &mut Vec<TypeDef>) {
        T::collect_type_defs(out);
    }
}

impl<K: TautType, V: TautType> TautType for std::collections::HashMap<K, V> {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Map {
            key: Box::new(K::ir_type_ref()),
            value: Box::new(V::ir_type_ref()),
        }
    }

    fn collect_type_defs(out: &mut Vec<TypeDef>) {
        K::collect_type_defs(out);
        V::collect_type_defs(out);
    }
}

// `Box<T>` is transparent in the IR — purely a Rust ownership concern.
impl<T: TautType> TautType for Box<T> {
    fn ir_type_ref() -> TypeRef {
        T::ir_type_ref()
    }

    fn ir_type_def() -> Option<TypeDef> {
        T::ir_type_def()
    }

    fn collect_type_defs(out: &mut Vec<TypeDef>) {
        T::collect_type_defs(out);
    }
}

// ---------------------------------------------------------------------------
// Tuples (arity 2..=4)
// ---------------------------------------------------------------------------

impl<A: TautType, B: TautType> TautType for (A, B) {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Tuple(vec![A::ir_type_ref(), B::ir_type_ref()])
    }

    fn collect_type_defs(out: &mut Vec<TypeDef>) {
        A::collect_type_defs(out);
        B::collect_type_defs(out);
    }
}

impl<A: TautType, B: TautType, C: TautType> TautType for (A, B, C) {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Tuple(vec![A::ir_type_ref(), B::ir_type_ref(), C::ir_type_ref()])
    }

    fn collect_type_defs(out: &mut Vec<TypeDef>) {
        A::collect_type_defs(out);
        B::collect_type_defs(out);
        C::collect_type_defs(out);
    }
}

impl<A: TautType, B: TautType, C: TautType, D: TautType> TautType for (A, B, C, D) {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Tuple(vec![
            A::ir_type_ref(),
            B::ir_type_ref(),
            C::ir_type_ref(),
            D::ir_type_ref(),
        ])
    }

    fn collect_type_defs(out: &mut Vec<TypeDef>) {
        A::collect_type_defs(out);
        B::collect_type_defs(out);
        C::collect_type_defs(out);
        D::collect_type_defs(out);
    }
}

// ---------------------------------------------------------------------------
// Optional integrations (feature-gated)
// ---------------------------------------------------------------------------
//
// Per SPEC §3.1, both `uuid::Uuid` and `chrono::DateTime` map to TS `string`
// but acquire dedicated `Primitive` variants so codegen can brand them
// (`Uuid`) or format them (ISO-8601 for `DateTime`). Cargo.toml wiring for
// these features is owned by another agent; the impls below compile out
// cleanly when the feature is off.

#[cfg(feature = "uuid")]
impl TautType for uuid::Uuid {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Primitive(Primitive::Uuid)
    }
}

#[cfg(feature = "chrono")]
impl TautType for chrono::DateTime<chrono::Utc> {
    fn ir_type_ref() -> TypeRef {
        TypeRef::Primitive(Primitive::DateTime)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn primitive_impls_return_expected_variants() {
        assert_eq!(bool::ir_type_ref(), TypeRef::Primitive(Primitive::Bool));
        assert_eq!(u8::ir_type_ref(), TypeRef::Primitive(Primitive::U8));
        assert_eq!(u16::ir_type_ref(), TypeRef::Primitive(Primitive::U16));
        assert_eq!(u32::ir_type_ref(), TypeRef::Primitive(Primitive::U32));
        assert_eq!(u64::ir_type_ref(), TypeRef::Primitive(Primitive::U64));
        assert_eq!(u128::ir_type_ref(), TypeRef::Primitive(Primitive::U128));
        assert_eq!(i8::ir_type_ref(), TypeRef::Primitive(Primitive::I8));
        assert_eq!(i16::ir_type_ref(), TypeRef::Primitive(Primitive::I16));
        assert_eq!(i32::ir_type_ref(), TypeRef::Primitive(Primitive::I32));
        assert_eq!(i64::ir_type_ref(), TypeRef::Primitive(Primitive::I64));
        assert_eq!(i128::ir_type_ref(), TypeRef::Primitive(Primitive::I128));
        assert_eq!(f32::ir_type_ref(), TypeRef::Primitive(Primitive::F32));
        assert_eq!(f64::ir_type_ref(), TypeRef::Primitive(Primitive::F64));
        assert_eq!(String::ir_type_ref(), TypeRef::Primitive(Primitive::String));
        assert_eq!(
            <&'static str as TautType>::ir_type_ref(),
            TypeRef::Primitive(Primitive::String),
        );
        assert_eq!(char::ir_type_ref(), TypeRef::Primitive(Primitive::String));
        assert_eq!(
            <() as TautType>::ir_type_ref(),
            TypeRef::Primitive(Primitive::Unit),
        );
    }

    #[test]
    fn option_of_u32_wraps_correctly() {
        assert_eq!(
            Option::<u32>::ir_type_ref(),
            TypeRef::Option(Box::new(TypeRef::Primitive(Primitive::U32))),
        );
    }

    #[test]
    fn vec_of_string_wraps_correctly() {
        assert_eq!(
            Vec::<String>::ir_type_ref(),
            TypeRef::Vec(Box::new(TypeRef::Primitive(Primitive::String))),
        );
    }

    #[test]
    fn vec_of_option_i64_composes() {
        assert_eq!(
            Vec::<Option<i64>>::ir_type_ref(),
            TypeRef::Vec(Box::new(TypeRef::Option(Box::new(TypeRef::Primitive(
                Primitive::I64,
            ))))),
        );
    }

    #[test]
    fn fixed_array_u8_16_has_len_16() {
        assert_eq!(
            <[u8; 16] as TautType>::ir_type_ref(),
            TypeRef::FixedArray {
                elem: Box::new(TypeRef::Primitive(Primitive::U8)),
                len: 16,
            },
        );
    }

    #[test]
    fn hashmap_string_to_u64_is_map() {
        assert_eq!(
            HashMap::<String, u64>::ir_type_ref(),
            TypeRef::Map {
                key: Box::new(TypeRef::Primitive(Primitive::String)),
                value: Box::new(TypeRef::Primitive(Primitive::U64)),
            },
        );
    }

    #[test]
    fn tuple_of_two_primitives() {
        assert_eq!(
            <(u32, String) as TautType>::ir_type_ref(),
            TypeRef::Tuple(vec![
                TypeRef::Primitive(Primitive::U32),
                TypeRef::Primitive(Primitive::String),
            ]),
        );
    }

    #[test]
    fn collect_type_defs_for_primitives_is_empty() {
        let mut out = Vec::new();
        <u32 as TautType>::collect_type_defs(&mut out);
        <bool as TautType>::collect_type_defs(&mut out);
        <String as TautType>::collect_type_defs(&mut out);
        <() as TautType>::collect_type_defs(&mut out);
        assert!(
            out.is_empty(),
            "primitives should not contribute any TypeDefs, got {out:?}",
        );
    }

    #[test]
    fn collect_type_defs_for_composites_of_primitives_is_empty() {
        // Composites override `collect_type_defs` to recurse, but if every
        // leaf is a primitive the cumulative output is still empty.
        let mut out = Vec::new();
        <Option<u32> as TautType>::collect_type_defs(&mut out);
        <Vec<Option<i64>> as TautType>::collect_type_defs(&mut out);
        <[u8; 4] as TautType>::collect_type_defs(&mut out);
        <HashMap<String, u64> as TautType>::collect_type_defs(&mut out);
        <(u32, String, bool, char) as TautType>::collect_type_defs(&mut out);
        <Box<u64> as TautType>::collect_type_defs(&mut out);
        assert!(out.is_empty(), "expected no defs, got {out:?}");
    }
}

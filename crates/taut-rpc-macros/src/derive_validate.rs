//! Expansion for `#[derive(Validate)]` (SPEC §7).
//!
//! Phase 4 implementation: lowers a Rust `struct` or `enum` to an
//! `impl ::taut_rpc::Validate for #ident` block. The emitted body collects
//! per-field validation failures via `::taut_rpc::validate::run` /
//! `::taut_rpc::validate::collect`, dispatching to the `check::*` helpers in
//! the runtime crate's `validate` module for each declared constraint.
//!
//! # Supported attributes
//!
//! Field-level `#[taut(...)]` keys recognised by this derive:
//!
//! | Key                                  | Constraint                                  |
//! |--------------------------------------|---------------------------------------------|
//! | `min = N` (int or float literal)     | `Constraint::Min` — emits `check::min`.     |
//! | `max = N`                            | `Constraint::Max` — emits `check::max`.     |
//! | `length(min = N, max = M)`           | `Constraint::Length` — emits `check::length`. Either bound may be omitted. |
//! | `pattern = "regex"`                  | `Constraint::Pattern` — emits `check::pattern`. |
//! | `email`                              | `Constraint::Email` — emits `check::email`. |
//! | `url`                                | `Constraint::Url` — emits `check::url`.     |
//! | `custom = "name"`                    | `Constraint::Custom` — recorded by the IR side; emits no runtime check. |
//!
//! Other `#[taut(...)]` keys (`rename`, `tag`, `optional`, `undefined`,
//! `code`, `status`) are owned by `#[derive(Type)]` / `#[derive(TautError)]`
//! and are silently ignored here so that a single `#[taut(...)]` block can
//! drive multiple derives. Unknown keys that don't match any known taut key
//! are likewise tolerated; only malformed *validation* constraint args
//! (e.g. `#[taut(min = "abc")]`) raise a compile error.
//!
//! # Type / enum shape
//!
//! - Structs with named fields walk each field, parse its constraints, and
//!   emit a `collect(...)` call per constraint.
//! - Tuple structs and unit structs have no per-field validation; they expand
//!   to an empty `run(...)` body.
//! - Enums dispatch via `match self { ... }`. Struct-variant fields can carry
//!   field-level constraints; tuple variants and unit variants validate as
//!   `Ok(())` (empty arm).
//!
//! Generics, lifetimes, and unions are rejected with a clear `syn::Error`.
//! The macro never panics on user input.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{
    parse2, Attribute, Data, DataEnum, DataStruct, DeriveInput, Expr, ExprLit, ExprUnary,
    Field as SynField, Fields, Lit, LitStr, UnOp, Variant as SynVariant,
};

pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let derive_input: DeriveInput = parse2(input)?;
    reject_generics(&derive_input)?;

    let ident = derive_input.ident.clone();

    let body = match &derive_input.data {
        Data::Struct(s) => expand_struct(s)?,
        Data::Enum(e) => expand_enum(e)?,
        Data::Union(u) => {
            return Err(syn::Error::new(
                u.union_token.span(),
                "taut_rpc: unions are not supported by #[derive(Validate)]",
            ));
        }
    };

    Ok(quote! {
        impl ::taut_rpc::Validate for #ident {
            fn validate(&self) -> ::std::result::Result<
                (),
                ::std::vec::Vec<::taut_rpc::ValidationError>,
            > {
                ::taut_rpc::validate::run(|__errors| {
                    #body
                })
            }
        }
    })
}

fn reject_generics(input: &DeriveInput) -> syn::Result<()> {
    if input.generics.params.is_empty() {
        return Ok(());
    }
    Err(syn::Error::new(
        input.generics.span(),
        "taut_rpc: generic types are not yet supported in v0.1; please monomorphize manually for now",
    ))
}

// ----------------------------------------------------------------------------
// Struct expansion
// ----------------------------------------------------------------------------

/// Expand a struct body. The struct is treated as a single match arm whose
/// access pattern is `self.<field>`; tuple/unit structs collapse to an empty
/// body since they have no addressable named fields.
fn expand_struct(s: &DataStruct) -> syn::Result<TokenStream> {
    match &s.fields {
        Fields::Named(named) => {
            let mut stmts = Vec::with_capacity(named.named.len());
            for f in &named.named {
                let constraints = FieldConstraints::parse(&f.attrs)?;
                let raw_ident = f.ident.as_ref().expect("named field must have an ident");
                let path_lit = LitStr::new(&raw_ident.to_string(), f.span());
                let access = quote_spanned! {f.span()=> self.#raw_ident };
                stmts.push(emit_field_checks(f, &access, &path_lit, &constraints));
            }
            Ok(quote! { #( #stmts )* })
        }
        Fields::Unnamed(unnamed) => {
            // Walk attrs to surface malformed validation constraints, but
            // tuple-struct fields have no name to put on the error path, so
            // we don't emit checks for them in v0.
            for f in &unnamed.unnamed {
                let _ = FieldConstraints::parse(&f.attrs)?;
            }
            Ok(quote! {})
        }
        Fields::Unit => Ok(quote! {}),
    }
}

// ----------------------------------------------------------------------------
// Enum expansion
// ----------------------------------------------------------------------------

fn expand_enum(e: &DataEnum) -> syn::Result<TokenStream> {
    let mut arms = Vec::with_capacity(e.variants.len());
    for v in &e.variants {
        arms.push(expand_variant(v)?);
    }
    if arms.is_empty() {
        // Empty enum: there is no value to validate, so no body is needed.
        return Ok(quote! {});
    }
    Ok(quote! {
        match self {
            #( #arms )*
        }
    })
}

fn expand_variant(v: &SynVariant) -> syn::Result<TokenStream> {
    let variant_ident = &v.ident;
    match &v.fields {
        Fields::Unit => Ok(quote_spanned! {v.span()=>
            Self::#variant_ident => {}
        }),
        Fields::Unnamed(unnamed) => {
            // Walk attrs to surface malformed constraints, but tuple-variant
            // fields are unsupported for validation in Phase 4 v0.
            for f in &unnamed.unnamed {
                let _ = FieldConstraints::parse(&f.attrs)?;
            }
            Ok(quote_spanned! {v.span()=>
                Self::#variant_ident(..) => {}
            })
        }
        Fields::Named(named) => {
            let mut binders = Vec::with_capacity(named.named.len());
            let mut stmts = Vec::with_capacity(named.named.len());
            for f in &named.named {
                let constraints = FieldConstraints::parse(&f.attrs)?;
                let raw_ident = f.ident.as_ref().expect("named field must have an ident");
                let path_lit = LitStr::new(&raw_ident.to_string(), f.span());
                binders.push(quote_spanned! {f.span()=> #raw_ident });
                stmts.push(emit_variant_field_checks(
                    f,
                    raw_ident,
                    &path_lit,
                    &constraints,
                ));
            }
            Ok(quote_spanned! {v.span()=>
                Self::#variant_ident { #( #binders ),* } => {
                    #( #stmts )*
                }
            })
        }
    }
}

// ----------------------------------------------------------------------------
// Per-field check emission
// ----------------------------------------------------------------------------

/// Emit the sequence of `collect(...)` / `nested(...)` calls for one field
/// inside a struct, where the value is reached via `self.<ident>`.
fn emit_field_checks(
    f: &SynField,
    access: &TokenStream,
    path_lit: &LitStr,
    c: &FieldConstraints,
) -> TokenStream {
    let mut out = TokenStream::new();

    if let Some(length) = &c.length {
        let min_tok = if let Some(n) = length.min {
            quote! { ::std::option::Option::Some(#n) }
        } else {
            quote! { ::std::option::Option::<u32>::None }
        };
        let max_tok = if let Some(n) = length.max {
            quote! { ::std::option::Option::Some(#n) }
        } else {
            quote! { ::std::option::Option::<u32>::None }
        };
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::length(
                #path_lit, #access.as_str(), #min_tok, #max_tok
            ));
        });
    }
    if c.email {
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::email(
                #path_lit, #access.as_str()
            ));
        });
    }
    if c.url {
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::url(
                #path_lit, #access.as_str()
            ));
        });
    }
    if let Some(min) = c.min {
        let min_lit = float_lit(min, f.span());
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::min(
                #path_lit, #access, #min_lit
            ));
        });
    }
    if let Some(max) = c.max {
        let max_lit = float_lit(max, f.span());
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::max(
                #path_lit, #access, #max_lit
            ));
        });
    }
    if let Some(pat) = &c.pattern {
        let pattern_lit = LitStr::new(pat, f.span());
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::pattern(
                #path_lit, #access.as_str(), #pattern_lit
            ));
        });
    }
    // `Custom` is intentionally a no-op at runtime: codegen carries the schema
    // fragment forward, and the trait impl emits nothing. See SPEC §7.
    out
}

/// Emit the sequence of checks for a struct-variant field, where the value is
/// reached through a binding (e.g. `username` from `Self::V { username, .. }`).
/// Bindings to named fields in a match arm are references (`&FieldTy`), so we
/// adjust accessors accordingly: numeric values are dereferenced (`*username`),
/// string-like values are passed through as `username.as_str()` etc.
fn emit_variant_field_checks(
    f: &SynField,
    binding: &syn::Ident,
    path_lit: &LitStr,
    c: &FieldConstraints,
) -> TokenStream {
    let mut out = TokenStream::new();

    if let Some(length) = &c.length {
        let min_tok = if let Some(n) = length.min {
            quote! { ::std::option::Option::Some(#n) }
        } else {
            quote! { ::std::option::Option::<u32>::None }
        };
        let max_tok = if let Some(n) = length.max {
            quote! { ::std::option::Option::Some(#n) }
        } else {
            quote! { ::std::option::Option::<u32>::None }
        };
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::length(
                #path_lit, #binding.as_str(), #min_tok, #max_tok
            ));
        });
    }
    if c.email {
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::email(
                #path_lit, #binding.as_str()
            ));
        });
    }
    if c.url {
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::url(
                #path_lit, #binding.as_str()
            ));
        });
    }
    if let Some(min) = c.min {
        let min_lit = float_lit(min, f.span());
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::min(
                #path_lit, *#binding, #min_lit
            ));
        });
    }
    if let Some(max) = c.max {
        let max_lit = float_lit(max, f.span());
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::max(
                #path_lit, *#binding, #max_lit
            ));
        });
    }
    if let Some(pat) = &c.pattern {
        let pattern_lit = LitStr::new(pat, f.span());
        out.extend(quote_spanned! {f.span()=>
            ::taut_rpc::validate::collect(__errors, || ::taut_rpc::validate::check::pattern(
                #path_lit, #binding.as_str(), #pattern_lit
            ));
        });
    }
    if out.is_empty() {
        // Suppress unused-variable warnings for fields without checks.
        out.extend(quote_spanned! {f.span()=> let _ = #binding; });
    }
    out
}

/// Render a `f64` literal token, preserving precision and emitting an explicit
/// `f64` suffix to match the spec'd macro output (`18f64`).
fn float_lit(v: f64, span: proc_macro2::Span) -> proc_macro2::Literal {
    // `Literal::f64_suffixed` always adds `f64`. It also rejects NaN / inf,
    // but our parser only produces finite values from numeric literals so
    // that path is unreachable in practice. If that ever changes, fall back
    // to `Literal::f64_unsuffixed` to avoid a panic.
    if v.is_finite() {
        let mut lit = proc_macro2::Literal::f64_suffixed(v);
        lit.set_span(span);
        lit
    } else {
        let mut lit = proc_macro2::Literal::f64_unsuffixed(v);
        lit.set_span(span);
        lit
    }
}

// ----------------------------------------------------------------------------
// Attribute parsing
// ----------------------------------------------------------------------------

/// Validation constraints parsed from one field's `#[taut(...)]` attributes.
///
/// Constraints are accumulated across multiple `#[taut(...)]` attributes if
/// they appear, e.g. `#[taut(email)] #[taut(length(min = 1))]`. Repeated
/// declarations of the same key overwrite the previous value (the last one
/// wins) — we don't try to be clever about merging.
#[derive(Debug, Default)]
struct FieldConstraints {
    min: Option<f64>,
    max: Option<f64>,
    length: Option<LengthBounds>,
    pattern: Option<String>,
    email: bool,
    url: bool,
    #[allow(dead_code)]
    custom: Option<String>,
}

#[derive(Debug, Default)]
struct LengthBounds {
    min: Option<u32>,
    max: Option<u32>,
}

impl FieldConstraints {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = FieldConstraints::default();
        for attr in attrs {
            if !attr.path().is_ident("taut") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("min") {
                    let expr: Expr = meta.value()?.parse()?;
                    out.min = Some(parse_number_expr(&expr)?);
                    Ok(())
                } else if meta.path.is_ident("max") {
                    let expr: Expr = meta.value()?.parse()?;
                    out.max = Some(parse_number_expr(&expr)?);
                    Ok(())
                } else if meta.path.is_ident("length") {
                    let bounds = parse_length(&meta)?;
                    out.length = Some(bounds);
                    Ok(())
                } else if meta.path.is_ident("pattern") {
                    let s: LitStr = meta.value()?.parse()?;
                    out.pattern = Some(s.value());
                    Ok(())
                } else if meta.path.is_ident("email") {
                    // Bare ident — must NOT have a value.
                    reject_value(&meta, "email")?;
                    out.email = true;
                    Ok(())
                } else if meta.path.is_ident("url") {
                    reject_value(&meta, "url")?;
                    out.url = true;
                    Ok(())
                } else if meta.path.is_ident("custom") {
                    let s: LitStr = meta.value()?.parse()?;
                    out.custom = Some(s.value());
                    Ok(())
                } else {
                    // Unknown / foreign keys (rename, tag, optional, undefined,
                    // code, status, …): silently consume any `= <expr>` payload
                    // or `(...)` group so this attr is owned by another derive.
                    consume_foreign(&meta)
                }
            })?;
        }
        Ok(out)
    }
}

/// Parse a `#[taut(length(min = N, max = M))]` group. Either bound is
/// optional; at least one must be present (otherwise the attribute is
/// pointless). Unknown keys inside `length(...)` produce a clear error.
fn parse_length(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<LengthBounds> {
    let mut bounds = LengthBounds::default();
    let mut saw_any = false;
    meta.parse_nested_meta(|inner| {
        if inner.path.is_ident("min") {
            let n: syn::LitInt = inner.value()?.parse()?;
            bounds.min = Some(n.base10_parse()?);
            saw_any = true;
            Ok(())
        } else if inner.path.is_ident("max") {
            let n: syn::LitInt = inner.value()?.parse()?;
            bounds.max = Some(n.base10_parse()?);
            saw_any = true;
            Ok(())
        } else {
            Err(inner.error(
                "taut_rpc: unknown key inside #[taut(length(...))]; expected `min` or `max`",
            ))
        }
    })?;
    if !saw_any {
        return Err(
            meta.error("taut_rpc: #[taut(length(...))] requires at least one of `min` or `max`")
        );
    }
    Ok(bounds)
}

/// Coerce an integer or float literal into `f64`. Other literal forms are
/// rejected as malformed validation arguments. A leading unary `-` is folded
/// into the result so users can write `#[taut(min = -10)]`.
fn parse_number_expr(expr: &Expr) -> syn::Result<f64> {
    match expr {
        Expr::Lit(ExprLit { lit, .. }) => parse_number_lit(lit),
        Expr::Unary(ExprUnary {
            op: UnOp::Neg(_),
            expr,
            ..
        }) => Ok(-parse_number_expr(expr)?),
        other => Err(syn::Error::new(
            other.span(),
            "taut_rpc: expected a numeric literal (integer or float) for `min` / `max`",
        )),
    }
}

fn parse_number_lit(lit: &Lit) -> syn::Result<f64> {
    match lit {
        Lit::Int(i) => i.base10_parse::<f64>(),
        Lit::Float(f) => f.base10_parse::<f64>(),
        other => Err(syn::Error::new(
            other.span(),
            "taut_rpc: expected a numeric literal (integer or float) for `min` / `max`",
        )),
    }
}

/// Ensure a bare-identifier constraint like `email` / `url` was not given
/// an `= <expr>` payload, which would be a sign of user confusion.
fn reject_value(meta: &syn::meta::ParseNestedMeta<'_>, key: &str) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        return Err(meta.error(format!(
            "taut_rpc: `#[taut({key})]` is a bare flag and does not take a value"
        )));
    }
    if meta.input.peek(syn::token::Paren) {
        return Err(meta.error(format!(
            "taut_rpc: `#[taut({key})]` is a bare flag and does not take a group"
        )));
    }
    Ok(())
}

/// Consume the payload of an unrecognised `#[taut(<key> ...)]` argument so
/// other derives owning the same attribute can interpret it. Tolerates:
/// - bare identifiers (`#[taut(optional)]`),
/// - `key = <any-expr>` (`#[taut(rename = "x")]`, `#[taut(status = 401)]`),
/// - `key(<group>)` (`#[taut(serde(...))]` style nested groups).
fn consume_foreign(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    let input = meta.input;
    if input.peek(syn::Token![=]) {
        let _: syn::Token![=] = input.parse()?;
        let _: proc_macro2::TokenStream = input.parse()?;
        // ^ The remaining tokens up to the next `,` would belong to the
        //   sibling parser; but `parse_nested_meta` actually slices on commas
        //   for us, so consuming everything up to end-of-input here is
        //   correct: the input passed to this callback ends at the next
        //   comma boundary.
        Ok(())
    } else if input.peek(syn::token::Paren) {
        // Nested group: parse and discard.
        let content;
        syn::parenthesized!(content in input);
        let _: proc_macro2::TokenStream = content.parse()?;
        Ok(())
    } else {
        // Bare ident with no payload — nothing to consume.
        Ok(())
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    fn expand_str(input: TokenStream) -> String {
        expand(input).expect("expansion succeeds").to_string()
    }

    #[test]
    fn expand_simple_struct_with_length_emits_check_length() {
        let input: TokenStream = quote! {
            struct S {
                #[taut(length(min = 3, max = 32))]
                name: String,
            }
        };
        let out = expand_str(input);
        assert!(
            out.contains("impl :: taut_rpc :: Validate for S"),
            "missing impl header: {out}"
        );
        assert!(
            out.contains(":: check :: length"),
            "missing check::length call: {out}"
        );
        assert!(
            out.contains("\"name\""),
            "missing field path literal: {out}"
        );
        assert!(out.contains("Some (3"), "missing min bound: {out}");
        assert!(out.contains("Some (32"), "missing max bound: {out}");
    }

    #[test]
    fn expand_struct_with_min_and_max_emits_both_checks() {
        let input: TokenStream = quote! {
            struct S {
                #[taut(min = 18, max = 120)]
                age: u8,
            }
        };
        let out = expand_str(input);
        assert!(
            out.contains(":: check :: min"),
            "missing check::min call: {out}"
        );
        assert!(
            out.contains(":: check :: max"),
            "missing check::max call: {out}"
        );
        assert!(
            out.contains("18f64"),
            "min bound should be emitted as f64: {out}"
        );
        assert!(
            out.contains("120f64"),
            "max bound should be emitted as f64: {out}"
        );
    }

    #[test]
    fn expand_struct_with_email_and_url_and_pattern() {
        let input: TokenStream = quote! {
            struct S {
                #[taut(email)]
                e: String,
                #[taut(url)]
                u: String,
                #[taut(pattern = "^[a-z]+$")]
                handle: String,
            }
        };
        let out = expand_str(input);
        assert!(out.contains(":: check :: email"), "missing email: {out}");
        assert!(out.contains(":: check :: url"), "missing url: {out}");
        assert!(
            out.contains(":: check :: pattern"),
            "missing pattern: {out}"
        );
        assert!(
            out.contains("\"^[a-z]+$\""),
            "missing pattern literal: {out}"
        );
    }

    #[test]
    fn expand_struct_with_no_constraints_emits_empty_run_body() {
        let input: TokenStream = quote! {
            struct S {
                /// no constraints
                note: String,
            }
        };
        let out = expand_str(input);
        // Trait impl is still emitted; the body inside `run` is empty.
        assert!(
            out.contains("impl :: taut_rpc :: Validate for S"),
            "missing impl header: {out}"
        );
        assert!(
            !out.contains(":: check :: "),
            "no check calls expected: {out}"
        );
    }

    #[test]
    fn expand_handles_enum_with_struct_variant() {
        let input: TokenStream = quote! {
            enum E {
                A,
                B(u32),
                C {
                    #[taut(length(min = 1))]
                    name: String,
                },
            }
        };
        let out = expand_str(input);
        assert!(
            out.contains("impl :: taut_rpc :: Validate for E"),
            "missing impl header: {out}"
        );
        assert!(out.contains("match self"), "missing match dispatch: {out}");
        assert!(out.contains("Self :: A =>"), "missing unit arm: {out}");
        assert!(out.contains("Self :: B (..)"), "missing tuple arm: {out}");
        assert!(out.contains("Self :: C {"), "missing struct arm: {out}");
        assert!(
            out.contains(":: check :: length"),
            "struct-variant field check missing: {out}"
        );
    }

    #[test]
    fn expand_rejects_union() {
        let input: TokenStream = quote! {
            union U { a: u32, b: f32 }
        };
        let err = expand(input).expect_err("unions must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("unions are not supported"),
            "error message was: {msg}"
        );
    }

    #[test]
    fn expand_rejects_generics() {
        let input: TokenStream = quote! {
            struct S<T> {
                #[taut(min = 0)]
                value: T,
            }
        };
        let err = expand(input).expect_err("generics must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("generic types are not yet supported"),
            "error message was: {msg}"
        );
    }

    #[test]
    fn expand_ignores_unknown_taut_keys() {
        // `rename`, `optional`, `undefined`, `tag`, `code`, `status` are owned
        // by Type / TautError and must be silently ignored.
        let input: TokenStream = quote! {
            struct S {
                #[taut(rename = "x", optional, undefined)]
                #[taut(length(min = 1))]
                a: String,
                #[taut(tag = "kind")]
                #[taut(min = 0)]
                b: i32,
            }
        };
        let out = expand_str(input);
        assert!(
            out.contains(":: check :: length"),
            "validation constraint should still fire: {out}"
        );
        assert!(
            out.contains(":: check :: min"),
            "min constraint should still fire: {out}"
        );
    }

    #[test]
    fn expand_rejects_malformed_min_value() {
        // `min = "abc"` looks like a validation constraint but the value is a
        // string literal — must error rather than be silently dropped.
        let input: TokenStream = quote! {
            struct S {
                #[taut(min = "abc")]
                age: u8,
            }
        };
        let err = expand(input).expect_err("must reject string for min");
        let msg = err.to_string();
        assert!(
            msg.contains("expected a numeric literal"),
            "error message was: {msg}"
        );
    }

    #[test]
    fn expand_rejects_length_with_no_bounds() {
        let input: TokenStream = quote! {
            struct S {
                #[taut(length())]
                name: String,
            }
        };
        let err = expand(input).expect_err("length() with no bounds must error");
        let msg = err.to_string();
        // Either our explicit "at least one of `min` or `max`" message, or
        // syn's lower-level "expected nested attribute" — both flag the same
        // user mistake.
        assert!(
            msg.contains("at least one of `min` or `max`")
                || msg.contains("expected nested attribute"),
            "error message was: {msg}"
        );
    }

    #[test]
    fn expand_rejects_unknown_key_inside_length() {
        let input: TokenStream = quote! {
            struct S {
                #[taut(length(bogus = 1))]
                name: String,
            }
        };
        let err = expand(input).expect_err("must reject unknown key inside length");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown key inside #[taut(length(...))]"),
            "error message was: {msg}"
        );
    }

    #[test]
    fn expand_struct_emits_run_wrapper_around_body() {
        // Sanity-check that the spec'd `validate::run(|__errors| { ... })`
        // wrapper is what we emit, not a hand-rolled error vec.
        let input: TokenStream = quote! {
            struct S {
                #[taut(min = 0)]
                age: i32,
            }
        };
        let out = expand_str(input);
        assert!(
            out.contains(":: validate :: run"),
            "missing validate::run wrapper: {out}"
        );
        assert!(
            out.contains("__errors"),
            "missing __errors closure binding: {out}"
        );
        assert!(
            out.contains(":: validate :: collect"),
            "missing validate::collect call: {out}"
        );
    }

    // ----- FieldConstraints unit tests -----------------------------------

    #[test]
    fn field_constraints_parses_min_and_max() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(min = 1, max = 10)])];
        let c = FieldConstraints::parse(&attrs).expect("parse");
        assert_eq!(c.min, Some(1.0));
        assert_eq!(c.max, Some(10.0));
    }

    #[test]
    fn field_constraints_parses_length() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(length(min = 3, max = 32))])];
        let c = FieldConstraints::parse(&attrs).expect("parse");
        let length = c.length.expect("length present");
        assert_eq!(length.min, Some(3));
        assert_eq!(length.max, Some(32));
    }

    #[test]
    fn field_constraints_parses_email_and_url_flags() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(email, url)])];
        let c = FieldConstraints::parse(&attrs).expect("parse");
        assert!(c.email);
        assert!(c.url);
    }

    #[test]
    fn field_constraints_parses_pattern() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(pattern = "^x$")])];
        let c = FieldConstraints::parse(&attrs).expect("parse");
        assert_eq!(c.pattern.as_deref(), Some("^x$"));
    }

    #[test]
    fn field_constraints_parses_custom() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(custom = "is_prime")])];
        let c = FieldConstraints::parse(&attrs).expect("parse");
        assert_eq!(c.custom.as_deref(), Some("is_prime"));
    }

    #[test]
    fn field_constraints_ignores_foreign_keys() {
        let attrs: Vec<Attribute> = vec![
            parse_quote!(#[taut(rename = "x")]),
            parse_quote!(#[taut(optional, undefined)]),
            parse_quote!(#[taut(tag = "kind")]),
            parse_quote!(#[taut(code = "x", status = 401)]),
        ];
        let c = FieldConstraints::parse(&attrs).expect("parse");
        assert!(c.min.is_none());
        assert!(c.max.is_none());
        assert!(c.length.is_none());
        assert!(c.pattern.is_none());
        assert!(!c.email);
        assert!(!c.url);
    }
}

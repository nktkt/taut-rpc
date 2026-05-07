//! Expansion for `#[derive(Type)]` (SPEC §3.2).
//!
//! Phase 1 implementation: lowers a Rust `struct` or `enum` to an
//! `impl ::taut_rpc::TautType for #ident` block. The trait exposes three
//! associated functions:
//!
//! - `ir_type_ref()` — returns a [`TypeRef::Named`] with the IR name (which can
//!   be overridden via `#[taut(rename = "...")]`).
//! - `ir_type_def()` — returns the structural [`TypeDef`] for the type.
//! - `collect_type_defs(out)` — walks the type's transitive dependencies,
//!   appending each reachable [`TypeDef`] to `out`. Each field type's own
//!   `collect_type_defs` is called so the registry is closed under reachability.
//!
//! Supported attribute keys (parsed from `#[taut(...)]`):
//!
//! | Position           | Key                       | Effect                                              |
//! |--------------------|---------------------------|-----------------------------------------------------|
//! | type (struct/enum) | `rename = "..."`          | Override the IR type name.                          |
//! | type (enum)        | `tag = "..."`             | Override the discriminator tag (default `"type"`).  |
//! | field              | `rename = "..."`          | Override the IR field name.                         |
//! | field              | `optional`                | Set `Field.optional = true` (TS `field?: T`).       |
//! | field              | `undefined`               | Set `Field.undefined = true` (`T | undefined`).     |
//! | field              | `min = N`, `max = N`      | Numeric range constraint (SPEC §7).                 |
//! | field              | `length(min = .., max = ..)` | String length constraint (SPEC §7).              |
//! | field              | `pattern = "..."`         | Regex pattern constraint (SPEC §7).                 |
//! | field              | `email`, `url`            | Built-in string-format constraints (SPEC §7).       |
//! | field              | `custom = "..."`          | Opaque user-supplied predicate tag (SPEC §7).       |
//!
//! Field-level validation constraints (`min`, `max`, `length`, `pattern`,
//! `email`, `url`, `custom`) are recorded into the emitted IR `Field`'s
//! `constraints` vec so codegen can lower them to a Valibot/Zod schema.
//!
//! Unknown keys, generic parameters, lifetime parameters, and `union`s are
//! reported as compile errors via [`syn::Error`]; the macro never panics on
//! user input.
//!
//! [`TypeRef::Named`]: ::taut_rpc::ir::TypeRef::Named
//! [`TypeDef`]: ::taut_rpc::ir::TypeDef

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned, ToTokens};
use syn::spanned::Spanned;
use syn::{
    parse2, Attribute, Data, DataEnum, DataStruct, DeriveInput, Expr, ExprLit, Field as SynField,
    Fields, Lit, LitStr, Meta, Token, Variant as SynVariant,
};

pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let derive_input: DeriveInput = parse2(input)?;
    reject_generics(&derive_input)?;

    let ident = derive_input.ident.clone();
    let type_attrs = TypeAttrs::parse(&derive_input.attrs)?;
    let ir_name = type_attrs.rename.unwrap_or_else(|| ident.to_string());
    let doc_tokens = doc_tokens(extract_doc(&derive_input.attrs));

    let (shape_tokens, collect_tokens) = match &derive_input.data {
        Data::Struct(s) => expand_struct(s)?,
        Data::Enum(e) => expand_enum(e, type_attrs.tag.as_deref())?,
        Data::Union(u) => {
            return Err(syn::Error::new(
                u.union_token.span(),
                "taut_rpc: unions are not supported",
            ));
        }
    };

    let ir_name_lit = LitStr::new(&ir_name, ident.span());

    Ok(quote! {
        impl ::taut_rpc::TautType for #ident {
            fn ir_type_ref() -> ::taut_rpc::ir::TypeRef {
                ::taut_rpc::ir::TypeRef::Named(#ir_name_lit.to_string())
            }

            fn ir_type_def() -> ::std::option::Option<::taut_rpc::ir::TypeDef> {
                ::std::option::Option::Some(::taut_rpc::ir::TypeDef {
                    name: #ir_name_lit.to_string(),
                    doc: #doc_tokens,
                    shape: #shape_tokens,
                })
            }

            fn collect_type_defs(out: &mut ::std::vec::Vec<::taut_rpc::ir::TypeDef>) {
                if let ::std::option::Option::Some(d) = <Self as ::taut_rpc::TautType>::ir_type_def() {
                    out.push(d);
                }
                #collect_tokens
            }
        }
    })
}

fn reject_generics(input: &DeriveInput) -> syn::Result<()> {
    if input.generics.params.is_empty() {
        return Ok(());
    }
    // Both type parameters and lifetime parameters are rejected for v0.1; the
    // SPEC says generics must be monomorphized manually, and lifetimes have no
    // representation in the IR / TS output anyway.
    Err(syn::Error::new(
        input.generics.span(),
        "taut_rpc: generic types are not yet supported in v0.1; please monomorphize manually for now",
    ))
}

// ----------------------------------------------------------------------------
// Struct expansion
// ----------------------------------------------------------------------------

fn expand_struct(s: &DataStruct) -> syn::Result<(TokenStream, TokenStream)> {
    match &s.fields {
        Fields::Named(named) => {
            let mut field_tokens = Vec::with_capacity(named.named.len());
            let mut collect_tokens = Vec::with_capacity(named.named.len());
            for f in &named.named {
                let attrs = FieldAttrs::parse(&f.attrs)?;
                let raw_name = f
                    .ident
                    .as_ref()
                    .expect("named field must have an ident")
                    .to_string();
                let name = attrs.rename.unwrap_or(raw_name);
                let name_lit = LitStr::new(&name, f.span());
                let ty = &f.ty;
                let optional = attrs.optional;
                let undefined = attrs.undefined;
                let doc = doc_tokens(extract_doc(&f.attrs));
                let constraints = constraints_tokens(&attrs.constraints);
                field_tokens.push(quote_spanned! {f.span()=>
                    ::taut_rpc::ir::Field {
                        name: #name_lit.to_string(),
                        ty: <#ty as ::taut_rpc::TautType>::ir_type_ref(),
                        optional: #optional,
                        undefined: #undefined,
                        doc: #doc,
                        constraints: #constraints,
                    }
                });
                collect_tokens.push(quote_spanned! {f.span()=>
                    <#ty as ::taut_rpc::TautType>::collect_type_defs(out);
                });
            }
            let shape = quote! {
                ::taut_rpc::ir::TypeShape::Struct(::std::vec![ #( #field_tokens ),* ])
            };
            let collect = quote! { #( #collect_tokens )* };
            Ok((shape, collect))
        }
        Fields::Unnamed(unnamed) => {
            let fields: Vec<&SynField> = unnamed.unnamed.iter().collect();
            // Reject field-level `#[taut(...)]` on tuple-struct fields too —
            // we still walk attrs for unknown-key validation but only `rename`
            // is meaningful (and even then unused). Keep it simple: parse and
            // discard; this surfaces unknown keys consistently.
            for f in &fields {
                let _ = FieldAttrs::parse(&f.attrs)?;
            }
            match fields.len() {
                0 => {
                    let shape = quote! {
                        ::taut_rpc::ir::TypeShape::Alias(
                            ::taut_rpc::ir::TypeRef::Primitive(::taut_rpc::ir::Primitive::Unit)
                        )
                    };
                    Ok((shape, quote! {}))
                }
                1 => {
                    let ty = &fields[0].ty;
                    let shape = quote_spanned! {fields[0].span()=>
                        ::taut_rpc::ir::TypeShape::Newtype(
                            <#ty as ::taut_rpc::TautType>::ir_type_ref()
                        )
                    };
                    let collect = quote_spanned! {fields[0].span()=>
                        <#ty as ::taut_rpc::TautType>::collect_type_defs(out);
                    };
                    Ok((shape, collect))
                }
                _ => {
                    let mut elems = Vec::with_capacity(fields.len());
                    let mut collect_tokens = Vec::with_capacity(fields.len());
                    for f in &fields {
                        let ty = &f.ty;
                        elems.push(quote_spanned! {f.span()=>
                            <#ty as ::taut_rpc::TautType>::ir_type_ref()
                        });
                        collect_tokens.push(quote_spanned! {f.span()=>
                            <#ty as ::taut_rpc::TautType>::collect_type_defs(out);
                        });
                    }
                    let shape = quote! {
                        ::taut_rpc::ir::TypeShape::Tuple(::std::vec![ #( #elems ),* ])
                    };
                    let collect = quote! { #( #collect_tokens )* };
                    Ok((shape, collect))
                }
            }
        }
        Fields::Unit => {
            let shape = quote! {
                ::taut_rpc::ir::TypeShape::Alias(
                    ::taut_rpc::ir::TypeRef::Primitive(::taut_rpc::ir::Primitive::Unit)
                )
            };
            Ok((shape, quote! {}))
        }
    }
}

// ----------------------------------------------------------------------------
// Enum expansion
// ----------------------------------------------------------------------------

fn expand_enum(
    e: &DataEnum,
    tag_override: Option<&str>,
) -> syn::Result<(TokenStream, TokenStream)> {
    let tag = tag_override.unwrap_or("type").to_string();
    let tag_lit = LitStr::new(&tag, proc_macro2::Span::call_site());

    let mut variants = Vec::with_capacity(e.variants.len());
    let mut collect_tokens = Vec::new();

    for v in &e.variants {
        let (variant_tokens, variant_collect) = expand_variant(v)?;
        variants.push(variant_tokens);
        collect_tokens.push(variant_collect);
    }

    let shape = quote! {
        ::taut_rpc::ir::TypeShape::Enum(::taut_rpc::ir::EnumDef {
            tag: #tag_lit.to_string(),
            variants: ::std::vec![ #( #variants ),* ],
        })
    };
    let collect = quote! { #( #collect_tokens )* };
    Ok((shape, collect))
}

fn expand_variant(v: &SynVariant) -> syn::Result<(TokenStream, TokenStream)> {
    // Variant-level attributes: only doc comments are honoured today. Reject
    // unknown `#[taut(...)]` keys for forward compatibility.
    let _variant_attrs = VariantAttrs::parse(&v.attrs)?;
    let name = v.ident.to_string();
    let name_lit = LitStr::new(&name, v.ident.span());

    let (payload, collect) = match &v.fields {
        Fields::Unit => (quote! { ::taut_rpc::ir::VariantPayload::Unit }, quote! {}),
        Fields::Unnamed(unnamed) => {
            let fields: Vec<&SynField> = unnamed.unnamed.iter().collect();
            for f in &fields {
                let _ = FieldAttrs::parse(&f.attrs)?;
            }
            let mut elems = Vec::with_capacity(fields.len());
            let mut collect_tokens = Vec::with_capacity(fields.len());
            for f in &fields {
                let ty = &f.ty;
                elems.push(quote_spanned! {f.span()=>
                    <#ty as ::taut_rpc::TautType>::ir_type_ref()
                });
                collect_tokens.push(quote_spanned! {f.span()=>
                    <#ty as ::taut_rpc::TautType>::collect_type_defs(out);
                });
            }
            (
                quote! {
                    ::taut_rpc::ir::VariantPayload::Tuple(::std::vec![ #( #elems ),* ])
                },
                quote! { #( #collect_tokens )* },
            )
        }
        Fields::Named(named) => {
            let mut field_tokens = Vec::with_capacity(named.named.len());
            let mut collect_tokens = Vec::with_capacity(named.named.len());
            for f in &named.named {
                let attrs = FieldAttrs::parse(&f.attrs)?;
                let raw_name = f
                    .ident
                    .as_ref()
                    .expect("named field must have an ident")
                    .to_string();
                let name = attrs.rename.unwrap_or(raw_name);
                let name_lit = LitStr::new(&name, f.span());
                let ty = &f.ty;
                let optional = attrs.optional;
                let undefined = attrs.undefined;
                let doc = doc_tokens(extract_doc(&f.attrs));
                let constraints = constraints_tokens(&attrs.constraints);
                field_tokens.push(quote_spanned! {f.span()=>
                    ::taut_rpc::ir::Field {
                        name: #name_lit.to_string(),
                        ty: <#ty as ::taut_rpc::TautType>::ir_type_ref(),
                        optional: #optional,
                        undefined: #undefined,
                        doc: #doc,
                        constraints: #constraints,
                    }
                });
                collect_tokens.push(quote_spanned! {f.span()=>
                    <#ty as ::taut_rpc::TautType>::collect_type_defs(out);
                });
            }
            (
                quote! {
                    ::taut_rpc::ir::VariantPayload::Struct(::std::vec![ #( #field_tokens ),* ])
                },
                quote! { #( #collect_tokens )* },
            )
        }
    };

    let variant = quote! {
        ::taut_rpc::ir::Variant {
            name: #name_lit.to_string(),
            payload: #payload,
        }
    };
    Ok((variant, collect))
}

// ----------------------------------------------------------------------------
// Attribute parsing
// ----------------------------------------------------------------------------

#[derive(Default)]
struct TypeAttrs {
    rename: Option<String>,
    tag: Option<String>,
}

impl TypeAttrs {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = TypeAttrs::default();
        for attr in attrs {
            if !attr.path().is_ident("taut") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let s: LitStr = meta.value()?.parse()?;
                    out.rename = Some(s.value());
                    Ok(())
                } else if meta.path.is_ident("tag") {
                    let s: LitStr = meta.value()?.parse()?;
                    out.tag = Some(s.value());
                    Ok(())
                } else if meta.path.is_ident("code") || meta.path.is_ident("status") {
                    // Owned by `#[derive(TautError)]`; consume any value and skip.
                    if meta.input.peek(Token![=]) {
                        let _: syn::Expr = meta.value()?.parse()?;
                    }
                    Ok(())
                } else {
                    let key = path_to_string(&meta.path);
                    Err(meta.error(format!(
                        "unknown taut attribute key: {key}; supported keys are rename, tag, optional, undefined, code, status"
                    )))
                }
            })?;
        }
        Ok(out)
    }
}

/// One parsed validation constraint attached to a struct/struct-variant field.
///
/// Mirrors the public `::taut_rpc::ir::Constraint` enum (re-exported from
/// `::taut_rpc::validate::Constraint`). The macro records constraints in this
/// shape during parsing and lowers them to a token literal in
/// [`constraint_tokens`] when emitting the IR `Field`.
#[derive(Debug)]
enum ConstraintTokens {
    Min(f64),
    Max(f64),
    Length { min: Option<u32>, max: Option<u32> },
    Pattern(String),
    Email,
    Url,
    Custom(String),
}

#[derive(Debug, Default)]
struct FieldAttrs {
    rename: Option<String>,
    optional: bool,
    undefined: bool,
    constraints: Vec<ConstraintTokens>,
}

impl FieldAttrs {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = FieldAttrs::default();
        for attr in attrs {
            if !attr.path().is_ident("taut") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let s: LitStr = meta.value()?.parse()?;
                    out.rename = Some(s.value());
                    Ok(())
                } else if meta.path.is_ident("optional") {
                    out.optional = true;
                    Ok(())
                } else if meta.path.is_ident("undefined") {
                    out.undefined = true;
                    Ok(())
                } else if meta.path.is_ident("min") {
                    let v = parse_f64_meta(&meta)?;
                    out.constraints.push(ConstraintTokens::Min(v));
                    Ok(())
                } else if meta.path.is_ident("max") {
                    let v = parse_f64_meta(&meta)?;
                    out.constraints.push(ConstraintTokens::Max(v));
                    Ok(())
                } else if meta.path.is_ident("length") {
                    let (min, max) = parse_length_meta(&meta)?;
                    out.constraints
                        .push(ConstraintTokens::Length { min, max });
                    Ok(())
                } else if meta.path.is_ident("pattern") {
                    let s: LitStr = meta.value()?.parse()?;
                    out.constraints.push(ConstraintTokens::Pattern(s.value()));
                    Ok(())
                } else if meta.path.is_ident("email") {
                    out.constraints.push(ConstraintTokens::Email);
                    Ok(())
                } else if meta.path.is_ident("url") {
                    out.constraints.push(ConstraintTokens::Url);
                    Ok(())
                } else if meta.path.is_ident("custom") {
                    let s: LitStr = meta.value()?.parse()?;
                    out.constraints.push(ConstraintTokens::Custom(s.value()));
                    Ok(())
                } else if meta.path.is_ident("code") || meta.path.is_ident("status") {
                    // Owned by `#[derive(TautError)]`; consume any value and skip.
                    if meta.input.peek(Token![=]) {
                        let _: syn::Expr = meta.value()?.parse()?;
                    }
                    Ok(())
                } else {
                    let key = path_to_string(&meta.path);
                    Err(meta.error(format!(
                        "unknown taut attribute key: {key}; supported keys are rename, optional, undefined, min, max, length, pattern, email, url, custom, code, status"
                    )))
                }
            })?;
        }
        Ok(out)
    }
}

/// Parse a numeric (`Int` or `Float`) literal from `meta.value()` and coerce
/// it to `f64`. Used for `min = N` / `max = N`.
fn parse_f64_meta(meta: &syn::meta::ParseNestedMeta) -> syn::Result<f64> {
    let lit: Lit = meta.value()?.parse()?;
    lit_to_f64(&lit)
}

fn lit_to_f64(lit: &Lit) -> syn::Result<f64> {
    match lit {
        Lit::Int(i) => i.base10_parse::<f64>(),
        Lit::Float(f) => f.base10_parse::<f64>(),
        other => Err(syn::Error::new(
            other.span(),
            "taut_rpc: expected numeric literal (integer or float) for min/max",
        )),
    }
}

fn lit_to_u32(lit: &Lit) -> syn::Result<u32> {
    match lit {
        Lit::Int(i) => i.base10_parse::<u32>(),
        other => Err(syn::Error::new(
            other.span(),
            "taut_rpc: expected u32 integer literal for length bounds",
        )),
    }
}

/// Parse `length(min = N, max = M)` — both bounds optional, but at least one
/// must be present.
fn parse_length_meta(meta: &syn::meta::ParseNestedMeta) -> syn::Result<(Option<u32>, Option<u32>)> {
    let mut min: Option<u32> = None;
    let mut max: Option<u32> = None;
    let span = meta.path.span();
    meta.parse_nested_meta(|inner| {
        if inner.path.is_ident("min") {
            let lit: Lit = inner.value()?.parse()?;
            min = Some(lit_to_u32(&lit)?);
            Ok(())
        } else if inner.path.is_ident("max") {
            let lit: Lit = inner.value()?.parse()?;
            max = Some(lit_to_u32(&lit)?);
            Ok(())
        } else {
            let key = path_to_string(&inner.path);
            Err(inner.error(format!(
                "taut_rpc: unknown key in length(...): {key}; expected min or max"
            )))
        }
    })?;
    if min.is_none() && max.is_none() {
        return Err(syn::Error::new(
            span,
            "taut_rpc: length(...) requires at least one of min or max",
        ));
    }
    Ok((min, max))
}

/// Lower an `f64` to a Rust float-literal token stream (`<v>f64`).
///
/// `syn::LitFloat` does not represent the leading `-` (Rust treats unary
/// negation as an operator, not part of the literal), so for negative
/// values we emit `- <lit>` as a two-token stream. We always append the
/// explicit `f64` suffix so the emitted token is unambiguous regardless of
/// how the value formats via `Display` (`100.0_f64` formats as `"100"`).
fn f64_lit_tokens(v: f64) -> TokenStream {
    if v < 0.0 {
        let abs = -v;
        let lit = syn::LitFloat::new(&format!("{abs}f64"), proc_macro2::Span::call_site());
        quote! { -#lit }
    } else {
        let lit = syn::LitFloat::new(&format!("{v}f64"), proc_macro2::Span::call_site());
        quote! { #lit }
    }
}

/// Lower a slice of [`ConstraintTokens`] to a `vec![ Constraint::..., ... ]`
/// token literal that constructs the runtime IR `Constraint` enum.
fn constraints_tokens(constraints: &[ConstraintTokens]) -> TokenStream {
    if constraints.is_empty() {
        return quote! { ::std::vec![] };
    }
    let elems = constraints.iter().map(constraint_tokens);
    quote! { ::std::vec![ #( #elems ),* ] }
}

fn constraint_tokens(c: &ConstraintTokens) -> TokenStream {
    match c {
        ConstraintTokens::Min(v) => {
            // Emit as a Rust float literal so the token is unambiguous
            // regardless of whether the user wrote an integer or float in the
            // source attribute. Negative values are emitted as `-<lit>` since
            // Rust treats unary minus as an operator outside the literal.
            let v_lit = f64_lit_tokens(*v);
            quote! { ::taut_rpc::ir::Constraint::Min(#v_lit) }
        }
        ConstraintTokens::Max(v) => {
            let v_lit = f64_lit_tokens(*v);
            quote! { ::taut_rpc::ir::Constraint::Max(#v_lit) }
        }
        ConstraintTokens::Length { min, max } => {
            let min_tokens = if let Some(n) = min {
                let lit = syn::LitInt::new(&format!("{n}u32"), proc_macro2::Span::call_site());
                quote! { ::std::option::Option::Some(#lit) }
            } else {
                quote! { ::std::option::Option::None }
            };
            let max_tokens = if let Some(n) = max {
                let lit = syn::LitInt::new(&format!("{n}u32"), proc_macro2::Span::call_site());
                quote! { ::std::option::Option::Some(#lit) }
            } else {
                quote! { ::std::option::Option::None }
            };
            quote! {
                ::taut_rpc::ir::Constraint::Length {
                    min: #min_tokens,
                    max: #max_tokens,
                }
            }
        }
        ConstraintTokens::Pattern(s) => {
            let lit = LitStr::new(s, proc_macro2::Span::call_site());
            quote! { ::taut_rpc::ir::Constraint::Pattern(#lit.to_string()) }
        }
        ConstraintTokens::Email => {
            quote! { ::taut_rpc::ir::Constraint::Email }
        }
        ConstraintTokens::Url => {
            quote! { ::taut_rpc::ir::Constraint::Url }
        }
        ConstraintTokens::Custom(s) => {
            let lit = LitStr::new(s, proc_macro2::Span::call_site());
            quote! { ::taut_rpc::ir::Constraint::Custom(#lit.to_string()) }
        }
    }
}

#[derive(Default)]
struct VariantAttrs {
    #[allow(dead_code)]
    rename: Option<String>,
}

impl VariantAttrs {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = VariantAttrs::default();
        for attr in attrs {
            if !attr.path().is_ident("taut") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let s: LitStr = meta.value()?.parse()?;
                    out.rename = Some(s.value());
                    Ok(())
                } else if meta.path.is_ident("code") || meta.path.is_ident("status") {
                    // Owned by `#[derive(TautError)]`; consume any value and skip.
                    if meta.input.peek(Token![=]) {
                        let _: syn::Expr = meta.value()?.parse()?;
                    }
                    Ok(())
                } else {
                    let key = path_to_string(&meta.path);
                    Err(meta.error(format!(
                        "unknown taut attribute key: {key}; supported keys are rename, tag, optional, undefined, code, status"
                    )))
                }
            })?;
        }
        Ok(out)
    }
}

fn path_to_string(path: &syn::Path) -> String {
    path.to_token_stream().to_string().replace(' ', "")
}

// ----------------------------------------------------------------------------
// Doc-comment extraction
// ----------------------------------------------------------------------------

/// Concatenate `#[doc = "..."]` attributes into a single trimmed string.
///
/// Each Rust `///` line lowers to a `#[doc = "..."]` attribute with a leading
/// space. We concatenate the literal contents with `\n`, then trim outer
/// whitespace. Returns `None` when no doc attributes are present (after
/// trimming).
fn extract_doc(attrs: &[Attribute]) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let Meta::NameValue(nv) = &attr.meta {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
            {
                lines.push(s.value());
            }
        }
    }
    if lines.is_empty() {
        return None;
    }
    let joined = lines.join("\n");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Lower an `Option<String>` doc into a token stream that produces the same
/// `Option<String>` at runtime.
fn doc_tokens(doc: Option<String>) -> TokenStream {
    if let Some(s) = doc {
        let lit = LitStr::new(&s, proc_macro2::Span::call_site());
        quote! { ::std::option::Option::Some(#lit.to_string()) }
    } else {
        quote! { ::std::option::Option::None }
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn extract_doc_trims_single_line() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[doc = " hello "])];
        assert_eq!(extract_doc(&attrs), Some("hello".to_string()));
    }

    #[test]
    fn extract_doc_joins_multiple_lines() {
        let attrs: Vec<Attribute> = vec![
            parse_quote!(#[doc = " first line"]),
            parse_quote!(#[doc = " second line"]),
        ];
        // Join with newline, then trim outer whitespace; inner spaces preserved.
        assert_eq!(
            extract_doc(&attrs),
            Some("first line\n second line".to_string())
        );
    }

    #[test]
    fn extract_doc_returns_none_when_absent_or_empty() {
        let none_attrs: Vec<Attribute> = vec![];
        assert_eq!(extract_doc(&none_attrs), None);

        let empty_attrs: Vec<Attribute> = vec![parse_quote!(#[doc = "   "])];
        assert_eq!(extract_doc(&empty_attrs), None);
    }

    #[test]
    fn type_attrs_parses_rename_and_tag() {
        let attrs: Vec<Attribute> = vec![
            parse_quote!(#[taut(rename = "Other")]),
            parse_quote!(#[taut(tag = "kind")]),
        ];
        let parsed = TypeAttrs::parse(&attrs).expect("parse");
        assert_eq!(parsed.rename.as_deref(), Some("Other"));
        assert_eq!(parsed.tag.as_deref(), Some("kind"));
    }

    #[test]
    fn field_attrs_rejects_unknown_key() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(bogus)])];
        let err = FieldAttrs::parse(&attrs).expect_err("must reject unknown key");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown taut attribute key"),
            "error message was: {msg}"
        );
    }

    #[test]
    fn field_attrs_parses_optional_and_undefined_flags() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(optional, undefined, rename = "x")])];
        let parsed = FieldAttrs::parse(&attrs).expect("parse");
        assert!(parsed.optional);
        assert!(parsed.undefined);
        assert_eq!(parsed.rename.as_deref(), Some("x"));
    }

    // -- Constraint attribute parsing -----------------------------------------

    #[test]
    fn field_attrs_parses_min_and_max_as_f64() {
        // Integer literals coerce to f64; float literals also accepted.
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(min = 0, max = 100)])];
        let parsed = FieldAttrs::parse(&attrs).expect("parse");
        assert_eq!(parsed.constraints.len(), 2);
        match &parsed.constraints[0] {
            ConstraintTokens::Min(v) => assert_eq!(*v, 0.0_f64),
            other => panic!("expected Min, got {other:?}"),
        }
        match &parsed.constraints[1] {
            ConstraintTokens::Max(v) => assert_eq!(*v, 100.0_f64),
            other => panic!("expected Max, got {other:?}"),
        }
    }

    #[test]
    fn field_attrs_parses_length_with_both_bounds() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(length(min = 3, max = 32))])];
        let parsed = FieldAttrs::parse(&attrs).expect("parse");
        assert_eq!(parsed.constraints.len(), 1);
        match &parsed.constraints[0] {
            ConstraintTokens::Length { min, max } => {
                assert_eq!(*min, Some(3));
                assert_eq!(*max, Some(32));
            }
            other => panic!("expected Length, got {other:?}"),
        }
    }

    #[test]
    fn field_attrs_parses_length_with_only_max() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(length(max = 64))])];
        let parsed = FieldAttrs::parse(&attrs).expect("parse");
        match &parsed.constraints[0] {
            ConstraintTokens::Length { min, max } => {
                assert_eq!(*min, None);
                assert_eq!(*max, Some(64));
            }
            other => panic!("expected Length, got {other:?}"),
        }
    }

    #[test]
    fn field_attrs_rejects_empty_length() {
        // `length()` fails inside syn's nested-meta parser before our own
        // "at least one of min/max" check fires (the parser rejects an empty
        // meta list as "expected nested attribute"). Either error message is
        // acceptable; the user-visible behaviour is "compile error on empty
        // length()".
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(length())])];
        let _err = FieldAttrs::parse(&attrs).expect_err("empty length must error");
    }

    #[test]
    fn field_attrs_rejects_unknown_length_key() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(length(other = 1))])];
        let err = FieldAttrs::parse(&attrs).expect_err("unknown inner key must error");
        assert!(
            err.to_string().contains("unknown key in length"),
            "error message was: {err}"
        );
    }

    #[test]
    fn field_attrs_parses_pattern_and_custom_strings() {
        let attrs: Vec<Attribute> = vec![
            parse_quote!(#[taut(pattern = "^\\d+$")]),
            parse_quote!(#[taut(custom = "must_be_prime")]),
        ];
        let parsed = FieldAttrs::parse(&attrs).expect("parse");
        assert_eq!(parsed.constraints.len(), 2);
        match &parsed.constraints[0] {
            ConstraintTokens::Pattern(s) => assert_eq!(s, r"^\d+$"),
            other => panic!("expected Pattern, got {other:?}"),
        }
        match &parsed.constraints[1] {
            ConstraintTokens::Custom(s) => assert_eq!(s, "must_be_prime"),
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn field_attrs_parses_email_and_url_bare_idents() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(email, url)])];
        let parsed = FieldAttrs::parse(&attrs).expect("parse");
        assert_eq!(parsed.constraints.len(), 2);
        assert!(matches!(parsed.constraints[0], ConstraintTokens::Email));
        assert!(matches!(parsed.constraints[1], ConstraintTokens::Url));
    }

    // -- IR emission contains constraint literals -----------------------------

    fn emit(input: TokenStream) -> String {
        expand(input).expect("expand").to_string()
    }

    #[test]
    fn field_with_min_max_constraints_emits_them() {
        let out = emit(quote! {
            struct Profile {
                #[taut(min = 0, max = 100)]
                age: u8,
            }
        });
        let normalized: String = out.split_whitespace().collect();
        assert!(
            normalized.contains("Constraint::Min(0f64)"),
            "expected Min(0f64) literal, got: {out}"
        );
        assert!(
            normalized.contains("Constraint::Max(100f64)"),
            "expected Max(100f64) literal, got: {out}"
        );
    }

    #[test]
    fn field_with_length_constraint() {
        let out = emit(quote! {
            struct Account {
                #[taut(length(min = 3, max = 32))]
                username: String,
            }
        });
        // The emitted token stream is whitespace-canonicalised by `to_string()`
        // (spaces around `::` and around `{`/`}`). Strip whitespace before
        // matching so the assertion isn't sensitive to formatting.
        let normalized: String = out.split_whitespace().collect();
        assert!(
            normalized.contains("Constraint::Length"),
            "expected Constraint::Length, got: {out}"
        );
        assert!(
            normalized.contains("Some(3u32)"),
            "expected Some(3u32) for length min, got: {out}"
        );
        assert!(
            normalized.contains("Some(32u32)"),
            "expected Some(32u32) for length max, got: {out}"
        );
    }

    #[test]
    fn field_with_email_constraint() {
        let out = emit(quote! {
            struct Signup {
                #[taut(email)]
                contact: String,
            }
        });
        let normalized: String = out.split_whitespace().collect();
        assert!(
            normalized.contains("Constraint::Email"),
            "expected Constraint::Email literal, got: {out}"
        );
    }

    #[test]
    fn field_with_url_constraint() {
        let out = emit(quote! {
            struct Link {
                #[taut(url)]
                href: String,
            }
        });
        let normalized: String = out.split_whitespace().collect();
        assert!(
            normalized.contains("Constraint::Url"),
            "expected Constraint::Url literal, got: {out}"
        );
    }

    #[test]
    fn field_with_pattern_and_custom_constraints_emit_string_literals() {
        let out = emit(quote! {
            struct Token {
                #[taut(pattern = "^[a-z]+$", custom = "must_be_prime")]
                value: String,
            }
        });
        let normalized: String = out.split_whitespace().collect();
        assert!(
            normalized.contains("Constraint::Pattern"),
            "expected Constraint::Pattern literal, got: {out}"
        );
        assert!(
            out.contains("\"^[a-z]+$\""),
            "expected pattern string literal in output, got: {out}"
        );
        assert!(
            normalized.contains("Constraint::Custom"),
            "expected Constraint::Custom literal, got: {out}"
        );
        assert!(
            out.contains("\"must_be_prime\""),
            "expected custom-tag string literal in output, got: {out}"
        );
    }

    #[test]
    fn field_with_no_taut_constraints_emits_empty_vec() {
        let out = emit(quote! {
            struct Plain {
                name: String,
            }
        });
        let normalized: String = out.split_whitespace().collect();
        // The Field literal carries `constraints: vec![]` (or its fully-qualified
        // form). Either way, no Constraint:: variant should appear.
        assert!(
            !normalized.contains("Constraint::Min")
                && !normalized.contains("Constraint::Max")
                && !normalized.contains("Constraint::Length")
                && !normalized.contains("Constraint::Pattern")
                && !normalized.contains("Constraint::Email")
                && !normalized.contains("Constraint::Url")
                && !normalized.contains("Constraint::Custom"),
            "expected no Constraint:: variants in emitted tokens, got: {out}"
        );
        assert!(
            normalized.contains("constraints:::std::vec![]")
                || normalized.contains("constraints:vec![]"),
            "expected `constraints: vec![]` in emitted Field, got: {out}"
        );
    }

    #[test]
    fn enum_struct_variant_field_emits_constraints() {
        let out = emit(quote! {
            enum Event {
                Signup {
                    #[taut(email)]
                    email: String,
                },
            }
        });
        let normalized: String = out.split_whitespace().collect();
        assert!(
            normalized.contains("Constraint::Email"),
            "expected Constraint::Email literal on struct-variant field, got: {out}"
        );
    }

    #[test]
    fn enum_tuple_variant_does_not_emit_constraints() {
        // Tuple-variant elements have no field name, so the IR carries no
        // per-field constraints. Verify no `Constraint::<Variant>` literal
        // leaks into the emitted token stream.
        let out = emit(quote! {
            enum Payload {
                Number(i32),
                Text(String),
            }
        });
        let normalized: String = out.split_whitespace().collect();
        assert!(
            !normalized.contains("Constraint::Min")
                && !normalized.contains("Constraint::Max")
                && !normalized.contains("Constraint::Length")
                && !normalized.contains("Constraint::Pattern")
                && !normalized.contains("Constraint::Email")
                && !normalized.contains("Constraint::Url")
                && !normalized.contains("Constraint::Custom"),
            "tuple variants must not emit Constraint:: variants, got: {out}"
        );
    }
}

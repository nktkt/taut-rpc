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
//! | Position           | Key                  | Effect                                              |
//! |--------------------|----------------------|-----------------------------------------------------|
//! | type (struct/enum) | `rename = "..."`     | Override the IR type name.                          |
//! | type (enum)        | `tag = "..."`        | Override the discriminator tag (default `"type"`).  |
//! | field              | `rename = "..."`     | Override the IR field name.                         |
//! | field              | `optional`           | Set `Field.optional = true` (TS `field?: T`).       |
//! | field              | `undefined`          | Set `Field.undefined = true` (`T | undefined`).     |
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
    Fields, Lit, LitStr, Meta, Variant as SynVariant,
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
                field_tokens.push(quote_spanned! {f.span()=>
                    ::taut_rpc::ir::Field {
                        name: #name_lit.to_string(),
                        ty: <#ty as ::taut_rpc::TautType>::ir_type_ref(),
                        optional: #optional,
                        undefined: #undefined,
                        doc: #doc,
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

fn expand_enum(e: &DataEnum, tag_override: Option<&str>) -> syn::Result<(TokenStream, TokenStream)> {
    let tag = tag_override.unwrap_or("type").to_string();
    let tag_lit = LitStr::new(&tag, proc_macro2::Span::call_site());

    let mut variants_tokens = Vec::with_capacity(e.variants.len());
    let mut collect_tokens = Vec::new();

    for v in &e.variants {
        let (variant_tokens, variant_collect) = expand_variant(v)?;
        variants_tokens.push(variant_tokens);
        collect_tokens.push(variant_collect);
    }

    let shape = quote! {
        ::taut_rpc::ir::TypeShape::Enum(::taut_rpc::ir::EnumDef {
            tag: #tag_lit.to_string(),
            variants: ::std::vec![ #( #variants_tokens ),* ],
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
        Fields::Unit => (
            quote! { ::taut_rpc::ir::VariantPayload::Unit },
            quote! {},
        ),
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
                field_tokens.push(quote_spanned! {f.span()=>
                    ::taut_rpc::ir::Field {
                        name: #name_lit.to_string(),
                        ty: <#ty as ::taut_rpc::TautType>::ir_type_ref(),
                        optional: #optional,
                        undefined: #undefined,
                        doc: #doc,
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
                } else {
                    let key = path_to_string(&meta.path);
                    Err(meta.error(format!(
                        "unknown taut attribute key: {key}; supported keys are rename, tag, optional, undefined"
                    )))
                }
            })?;
        }
        Ok(out)
    }
}

#[derive(Debug, Default)]
struct FieldAttrs {
    rename: Option<String>,
    optional: bool,
    undefined: bool,
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
                } else {
                    let key = path_to_string(&meta.path);
                    Err(meta.error(format!(
                        "unknown taut attribute key: {key}; supported keys are rename, tag, optional, undefined"
                    )))
                }
            })?;
        }
        Ok(out)
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
                } else {
                    let key = path_to_string(&meta.path);
                    Err(meta.error(format!(
                        "unknown taut attribute key: {key}; supported keys are rename, tag, optional, undefined"
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
    match doc {
        Some(s) => {
            let lit = LitStr::new(&s, proc_macro2::Span::call_site());
            quote! { ::std::option::Option::Some(#lit.to_string()) }
        }
        None => quote! { ::std::option::Option::None },
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
        let attrs: Vec<Attribute> =
            vec![parse_quote!(#[taut(optional, undefined, rename = "x")])];
        let parsed = FieldAttrs::parse(&attrs).expect("parse");
        assert!(parsed.optional);
        assert!(parsed.undefined);
        assert_eq!(parsed.rename.as_deref(), Some("x"));
    }
}

//! Expansion for `#[derive(TautError)]` (SPEC §3.3).
//!
//! Phase 2 implementation: lowers a Rust `enum` to an
//! `impl ::taut_rpc::TautError for #ident` block providing the two trait
//! methods:
//!
//! - `code(&self) -> &'static str` — returns a stable per-variant code. The
//!   default is the variant name converted to `snake_case` (matching the
//!   convention `serde(rename_all = "snake_case")` would produce), and can be
//!   overridden per-variant via `#[taut(code = "...")]`.
//! - `http_status(&self) -> u16` — returns the HTTP status to use when this
//!   variant is rendered on the wire. The default is `400`, overridable
//!   per-variant via `#[taut(status = 401)]`.
//!
//! Both can be combined: `#[taut(code = "auth_required", status = 401)]`.
//!
//! # Relationship to the serde wire shape
//!
//! This derive ONLY emits the `TautError` trait impl. It does NOT emit
//! `#[derive(Serialize)]` or `#[serde(tag = "code", content = "payload",
//! rename_all = "snake_case")]`. Users must add those themselves on the same
//! type — the SPEC §4.1 wire envelope (`{ "err": { "code": ..., "payload": ... } }`)
//! is produced by serde, while `code()` / `http_status()` are produced by this
//! derive. Keeping the two concerns separate lets users opt out of serde's tag
//! conventions when they need a custom serializer while still getting the
//! `TautError` impl for free.
//!
//! # Constraints
//!
//! - The deriving type must be an `enum`. Structs and unions are rejected.
//! - Generics (type parameters and lifetimes) are rejected, mirroring
//!   `derive(Type)` — Phase 1/2 require monomorphic forms.
//! - Unknown `#[taut(...)]` keys on a variant produce a compile error rather
//!   than being silently ignored. Supported keys are `code` and `status`.

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned, ToTokens};
use syn::spanned::Spanned;
use syn::{
    parse2, Attribute, Data, DataEnum, DeriveInput, Fields, LitInt, LitStr, Variant as SynVariant,
};

pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let derive_input: DeriveInput = parse2(input)?;
    reject_generics(&derive_input)?;

    let ident = derive_input.ident.clone();

    let data_enum = match &derive_input.data {
        Data::Enum(e) => e,
        Data::Struct(s) => {
            return Err(syn::Error::new(
                s.struct_token.span(),
                "taut_rpc: #[derive(TautError)] can only be applied to enums",
            ));
        }
        Data::Union(u) => {
            return Err(syn::Error::new(
                u.union_token.span(),
                "taut_rpc: #[derive(TautError)] can only be applied to enums",
            ));
        }
    };

    let (code_arms, status_arms) = expand_enum(data_enum)?;

    Ok(quote! {
        impl ::taut_rpc::TautError for #ident {
            fn code(&self) -> &'static str {
                match self {
                    #( #code_arms )*
                }
            }

            fn http_status(&self) -> u16 {
                match self {
                    #( #status_arms )*
                }
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

fn expand_enum(e: &DataEnum) -> syn::Result<(Vec<TokenStream>, Vec<TokenStream>)> {
    let mut code_arms = Vec::with_capacity(e.variants.len());
    let mut status_arms = Vec::with_capacity(e.variants.len());

    for v in &e.variants {
        let attrs = VariantAttrs::parse(&v.attrs)?;
        let variant_ident = &v.ident;
        let default_code = variant_name_to_default_code(&variant_ident.to_string());
        let code = attrs.code.unwrap_or(default_code);
        let status: u16 = attrs.status.unwrap_or(400);

        let code_lit = LitStr::new(&code, variant_ident.span());
        let status_lit = LitInt::new(&status.to_string(), variant_ident.span());
        let pattern = variant_pattern(v);

        code_arms.push(quote_spanned! {v.span()=>
            #pattern => #code_lit,
        });
        status_arms.push(quote_spanned! {v.span()=>
            #pattern => #status_lit,
        });
    }

    Ok((code_arms, status_arms))
}

/// Produce the match-arm pattern for a variant, ignoring all payload data:
/// `Self::A`, `Self::B(..)`, or `Self::C { .. }`.
fn variant_pattern(v: &SynVariant) -> TokenStream {
    let ident = &v.ident;
    match &v.fields {
        Fields::Unit => quote! { Self::#ident },
        Fields::Unnamed(_) => quote! { Self::#ident(..) },
        Fields::Named(_) => quote! { Self::#ident { .. } },
    }
}

// ----------------------------------------------------------------------------
// Attribute parsing
// ----------------------------------------------------------------------------

#[derive(Debug, Default)]
struct VariantAttrs {
    code: Option<String>,
    status: Option<u16>,
}

impl VariantAttrs {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = VariantAttrs::default();
        for attr in attrs {
            if !attr.path().is_ident("taut") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("code") {
                    let s: LitStr = meta.value()?.parse()?;
                    out.code = Some(s.value());
                    Ok(())
                } else if meta.path.is_ident("status") {
                    let n: LitInt = meta.value()?.parse()?;
                    let parsed: u16 = n.base10_parse()?;
                    out.status = Some(parsed);
                    Ok(())
                } else {
                    let key = path_to_string(&meta.path);
                    Err(meta.error(format!(
                        "unknown taut attribute key on TautError variant: {key}; supported keys are code, status"
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
// Default code transformation
// ----------------------------------------------------------------------------

/// Convert a CamelCase variant name to `snake_case`, mirroring the behavior of
/// serde's `rename_all = "snake_case"`. Each uppercase character (other than
/// the first) is preceded by an underscore and lowercased.
fn variant_name_to_default_code(name: &str) -> String {
    // snake_case conversion mirroring serde's `rename_all = "snake_case"`.
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn variant_name_to_default_code_handles_camel_case() {
        assert_eq!(variant_name_to_default_code("NotFound"), "not_found");
    }

    #[test]
    fn variant_name_to_default_code_handles_runs_of_uppercase() {
        // Each uppercase letter (after the first) gets an underscore in front.
        assert_eq!(variant_name_to_default_code("ABC"), "a_b_c");
    }

    #[test]
    fn variant_name_to_default_code_passes_through_lowercase() {
        assert_eq!(variant_name_to_default_code("lowercase"), "lowercase");
    }

    #[test]
    fn variant_attrs_parses_code_and_status() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(code = "x", status = 401)])];
        let parsed = VariantAttrs::parse(&attrs).expect("parse");
        assert_eq!(parsed.code.as_deref(), Some("x"));
        assert_eq!(parsed.status, Some(401));
    }

    #[test]
    fn variant_attrs_rejects_unknown_key() {
        let attrs: Vec<Attribute> = vec![parse_quote!(#[taut(unknown)])];
        let err = VariantAttrs::parse(&attrs).expect_err("must reject unknown key");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown taut attribute key on TautError variant"),
            "error message was: {msg}"
        );
        assert!(
            msg.contains("supported keys are code, status"),
            "error message was: {msg}"
        );
    }

    #[test]
    fn expand_emits_expected_match_arms() {
        let input: TokenStream = quote! {
            enum E {
                A,
                #[taut(code = "auth_required", status = 401)]
                B(u32),
                C { x: u64 },
            }
        };
        let out = expand(input).expect("expansion succeeds").to_string();

        // Trait impl header.
        assert!(
            out.contains("impl :: taut_rpc :: TautError for E"),
            "missing impl header in: {out}"
        );

        // Default snake_case code for `A`, override for `B`, default for `C`.
        assert!(out.contains("\"a\""), "missing default code 'a': {out}");
        assert!(
            out.contains("\"auth_required\""),
            "missing override code 'auth_required': {out}"
        );
        assert!(out.contains("\"c\""), "missing default code 'c': {out}");

        // Status: default 400 for A and C, override 401 for B.
        assert!(out.contains("400"), "missing default status 400: {out}");
        assert!(out.contains("401"), "missing override status 401: {out}");

        // Match patterns reflect the variant shapes.
        assert!(
            out.contains("Self :: A"),
            "missing unit-variant pattern: {out}"
        );
        assert!(
            out.contains("Self :: B (..)"),
            "missing tuple-variant pattern: {out}"
        );
        assert!(
            out.contains("Self :: C { .. }"),
            "missing struct-variant pattern: {out}"
        );
    }

    #[test]
    fn expand_rejects_struct() {
        let input: TokenStream = quote! {
            struct S { x: u64 }
        };
        let err = expand(input).expect_err("structs must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("can only be applied to enums"),
            "error message was: {msg}"
        );
    }

    #[test]
    fn expand_rejects_generics() {
        let input: TokenStream = quote! {
            enum E<T> { A(T) }
        };
        let err = expand(input).expect_err("generics must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("generic types are not yet supported"),
            "error message was: {msg}"
        );
    }
}

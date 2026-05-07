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
//! # `#[taut(...)]` namespace ownership (Phase 5 audit)
//!
//! The `#[taut(...)]` attribute is shared across three derives so a user can
//! decorate one type with `#[derive(Type, Validate, TautError)]` and write a
//! single attribute block per variant/field. Each derive owns a disjoint set
//! of keys and MUST silently consume any key it doesn't own (without erroring)
//! so the others can claim it. Ownership map:
//!
//! | Position             | Owned by `Type`            | Owned by `Validate`                                         | Owned by `TautError`   |
//! |----------------------|----------------------------|-------------------------------------------------------------|------------------------|
//! | type (struct/enum)   | `rename`, `tag`            | —                                                           | —                      |
//! | enum variant         | (none today)               | —                                                           | `code`, `status`       |
//! | named field          | `rename`, `optional`, `undefined` | `min`, `max`, `length(...)`, `pattern`, `email`, `url`, `custom` | —                      |
//!
//! `TautError` itself only reads variant-level `code` / `status`. Any other
//! `#[taut(<key> ...)]` argument it sees on a variant — `rename = "..."` from
//! `Type`, `length(...)` etc. (which would actually live on a field, but be
//! defensive), or yet-unknown keys reserved for future derives — is consumed
//! and discarded by [`consume_foreign`]. Type-level and field-level attributes
//! are not read by this derive at all, so they don't need explicit pass-through
//! handling. See SPEC §3.3.
//!
//! # Constraints
//!
//! - The deriving type must be an `enum`. Structs and unions are rejected.
//! - Generics (type parameters and lifetimes) are rejected, mirroring
//!   `derive(Type)` — Phase 1/2 require monomorphic forms.
//! - Unknown variant-level keys are silently consumed, not rejected, so this
//!   derive composes with sibling derives that share the `taut` namespace.

use proc_macro2::{Span, TokenStream};
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{
    parse2, Attribute, Data, DataEnum, DeriveInput, Fields, LitInt, LitStr, Variant as SynVariant,
};

/// Build a `syn::Error` whose message points at the relevant SPEC section so
/// users can find the rules behind a rejection. Format:
///
/// ```text
/// taut_rpc: <msg>
///   see SPEC §<spec_anchor>
/// ```
fn err(span: Span, msg: &str, spec_anchor: &str) -> syn::Error {
    syn::Error::new(span, format!("taut_rpc: {msg}\n  see SPEC §{spec_anchor}"))
}

pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let derive_input: DeriveInput = parse2(input)?;
    reject_generics(&derive_input)?;

    let ident = derive_input.ident.clone();

    let data_enum = match &derive_input.data {
        Data::Enum(e) => e,
        Data::Struct(s) => {
            return Err(err(
                s.struct_token.span(),
                "#[derive(TautError)] can only be applied to enums",
                "3.3",
            ));
        }
        Data::Union(u) => {
            return Err(err(
                u.union_token.span(),
                "#[derive(TautError)] can only be applied to enums",
                "3.3",
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
    Err(err(
        input.generics.span(),
        "generic types are not yet supported in v0.1; please monomorphize manually for now",
        "3.3",
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
                    // Foreign key (owned by `Type` / `Validate`, or reserved
                    // for a future derive). Silently consume any `= <expr>`
                    // payload or `(...)` group so this attribute can be
                    // shared across the three derives. See the namespace
                    // ownership table at the top of this file.
                    consume_foreign(&meta)
                }
            })?;
        }
        Ok(out)
    }
}

/// Consume the payload of a foreign `#[taut(<key> ...)]` argument so the other
/// derives sharing this attribute can interpret it. Tolerates:
/// - bare identifiers (`#[taut(optional)]`),
/// - `key = <any-expr>` (`#[taut(rename = "x")]`),
/// - `key(<group>)` (`#[taut(length(min = 1))]`).
///
/// Only the current key's payload is consumed — sibling keys (after a comma)
/// remain in `meta.input` so `parse_nested_meta`'s loop can dispatch them.
fn consume_foreign(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    let input = meta.input;
    if input.peek(syn::Token![=]) {
        // `key = <expr>` — parse a single Expr (comma-aware) via meta.value().
        // The returned ParseStream advances `input` past the `=` and the
        // value but stops at the comma boundary, leaving sibling keys intact.
        let value = meta.value()?;
        let _: syn::Expr = value.parse()?;
        Ok(())
    } else if input.peek(syn::token::Paren) {
        // `key(...)` — parse and discard the parenthesized group; the outer
        // input is left at the closing paren so the comma after is still
        // available for the next sibling.
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
    fn variant_attrs_silently_consumes_foreign_keys() {
        // Phase 5 namespace coexistence: `rename` is owned by `derive(Type)`,
        // `length(...)` is a field-level concern of `derive(Validate)`. When
        // the user writes them on the same variant alongside `code`/`status`,
        // TautError must consume them without erroring so all three derives
        // can share `#[taut(...)]`.
        let attrs: Vec<Attribute> = vec![
            parse_quote!(#[taut(rename = "Other", code = "x", status = 401)]),
            parse_quote!(#[taut(unknown_future_key)]),
            parse_quote!(#[taut(length(min = 1, max = 10))]),
        ];
        let parsed = VariantAttrs::parse(&attrs).expect("foreign keys must be consumed");
        assert_eq!(parsed.code.as_deref(), Some("x"));
        assert_eq!(parsed.status, Some(401));
    }

    #[test]
    fn expand_handles_phase5_combined_attrs_example() {
        // Mirrors the SPEC §5 "namespace coexistence" example: a single enum
        // wears `#[derive(Type, Validate, TautError)]` and each variant carries
        // owned-by-others keys (`length(...)` on a field is owned by
        // Validate/Type; `code`/`status` on the variant are owned by us).
        // TautError must lower without erroring on the foreign keys.
        let input: TokenStream = quote! {
            enum AppError {
                #[taut(status = 401, code = "auth_required")]
                NotAuthed,

                #[taut(status = 400)]
                BadInput { #[taut(length(min = 1))] msg: String },
            }
        };
        let out = expand(input).expect("expansion must succeed").to_string();
        assert!(
            out.contains("\"auth_required\""),
            "missing override code: {out}"
        );
        assert!(out.contains("401"), "missing override status: {out}");
        assert!(
            out.contains("\"bad_input\""),
            "missing default snake_case code for BadInput: {out}"
        );
        assert!(out.contains("400"), "missing default status: {out}");
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

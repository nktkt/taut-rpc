//! Expansion for `#[derive(Type)]`.
//!
//! Day-0 stub: validates the input is a struct or enum and emits a hidden
//! marker constant. The real expansion is deferred until the runtime crate
//! exists and exposes the marker trait.
//
// TODO(phase 1, SPEC §3.2): once `taut-rpc` exposes `taut_rpc::__private::TautType`
// the real expansion must
//   1. emit `impl ::taut_rpc::__private::TautType for #ident { ... }` so the
//      type is reachable from the compile-time type registry;
//   2. lower the type into an IR fragment (struct → interface, enum →
//      discriminated union with `#[taut(tag = "...")]` honoured);
//   3. honour field-level attributes such as `#[taut(undefined)]` (SPEC §3.1)
//      and the future `#[taut(rename = "...")]`.
// For now we only parse-validate and emit a unit constant so the derive is a
// no-op rather than a hard error during early bring-up.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse2, Data, DeriveInput};

pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let derive_input: DeriveInput = parse2(input)?;

    match &derive_input.data {
        Data::Struct(_) | Data::Enum(_) => {}
        Data::Union(union_data) => {
            return Err(syn::Error::new_spanned(
                union_data.union_token,
                "`#[derive(Type)]` cannot be applied to a union; only structs and enums are supported",
            ));
        }
    }

    // Reference the ident so a typo in the input still produces a span-anchored
    // error if anything goes wrong above; not strictly needed for the unit
    // emission but keeps the stub future-friendly.
    let _ident = &derive_input.ident;

    Ok(quote! {
        // Placeholder until `taut_rpc::__private::TautType` exists.
        // Replaced in phase 1 by the real `impl` plus IR registration.
        const _: () = ();
    })
}

//! Procedural macros for `taut-rpc`.
//!
//! This crate is an implementation detail of `taut-rpc`; users should depend on
//! `taut-rpc` and import the macros via its re-exports rather than depending on
//! this crate directly.
//!
//! Two macros are provided:
//!
//! - `#[rpc]` — attribute macro applied to an `async fn` to register it as an
//!   RPC procedure. Supports the forms `#[rpc]`, `#[rpc(stream)]`, and
//!   `#[rpc(method = "GET")]`. See SPEC §2 (architecture) and §4 (wire format).
//! - `#[derive(Type)]` — derive macro that records a Rust type in the IR so
//!   the codegen step can emit a corresponding TypeScript definition. See
//!   SPEC §3 (type mapping).
//!
//! Both macros report errors via `syn::Error::into_compile_error` so failures
//! surface as compiler diagnostics rather than panics.

use proc_macro::TokenStream;

mod derive_type;
mod rpc_attr;

/// Marks an `async fn` as a `taut-rpc` procedure.
///
/// Forms:
/// - `#[rpc]` — query (default).
/// - `#[rpc(stream)]` — server-streaming subscription.
/// - `#[rpc(method = "GET")]` — opt-in cacheable GET query.
#[proc_macro_attribute]
pub fn rpc(attr: TokenStream, item: TokenStream) -> TokenStream {
    rpc_attr::expand(attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derives `taut-rpc`'s type-registration trait for a struct or enum so the
/// type appears in the IR and gets a TypeScript definition emitted.
#[proc_macro_derive(Type, attributes(taut))]
pub fn derive_type(input: TokenStream) -> TokenStream {
    derive_type::expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

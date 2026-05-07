//! Procedural macros for `taut-rpc`.
//!
//! This crate is an implementation detail of `taut-rpc`; users should depend on
//! `taut-rpc` and import the macros via its re-exports rather than depending on
//! this crate directly.
//!
//! Four macros are provided through Phase 4:
//!
//! - `#[rpc]` — attribute macro applied to a free `async fn` (queries and
//!   mutations only in Phase 1; `#[rpc(stream)]` lands in Phase 3). Supports
//!   `#[rpc]` and `#[rpc(method = "GET")]`. See SPEC §2 (architecture) and §4
//!   (wire format).
//! - `#[derive(Type)]` — derive macro that records a Rust type in the IR so
//!   the codegen step can emit a corresponding TypeScript definition. Works on
//!   structs (named, tuple, unit) and enums (unit, tuple, struct variants).
//!   See SPEC §3 (type mapping).
//! - `#[derive(TautError)]` — derive macro that supplies the `TautError`
//!   trait impl (per-variant `code()` and `http_status()`) for an enum. See
//!   SPEC §3.3 (errors).
//! - `#[derive(Validate)]` — derive macro that emits the `Validate` trait impl
//!   from per-field `#[taut(...)]` constraint attributes (`min`, `max`,
//!   `length`, `pattern`, `email`, `url`, `custom`). See SPEC §7 (validation
//!   bridge).
//!
//! All macros report errors via `syn::Error::into_compile_error` so failures
//! surface as compiler diagnostics rather than panics.

use proc_macro::TokenStream;

mod derive_taut_error;
mod derive_type;
mod derive_validate;
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

/// Derives `taut-rpc`'s `TautError` trait for an enum, supplying `code()`
/// (default: variant name in `snake_case`) and `http_status()` (default: 400)
/// per variant. Both can be overridden via `#[taut(code = "...", status =
/// 401)]`. See SPEC §3.3.
#[proc_macro_derive(TautError, attributes(taut))]
pub fn derive_taut_error(input: TokenStream) -> TokenStream {
    derive_taut_error::expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derives `taut-rpc`'s `Validate` trait for a struct or enum, walking each
/// field's `#[taut(...)]` constraints (`min`, `max`, `length`, `pattern`,
/// `email`, `url`, `custom`) and dispatching to the corresponding
/// `validate::check::*` runtime helpers. Foreign `#[taut(...)]` keys owned by
/// other derives (`rename`, `tag`, `optional`, `undefined`, `code`, `status`)
/// are silently ignored. See SPEC §7.
#[proc_macro_derive(Validate, attributes(taut))]
pub fn derive_validate(input: TokenStream) -> TokenStream {
    derive_validate::expand(input.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

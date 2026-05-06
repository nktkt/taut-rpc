//! Expansion for the `#[rpc]` attribute macro.
//!
//! Day-0 stub: validates the shape of the input (`async fn`, recognised
//! attribute form) and re-emits the function unchanged together with a hidden
//! marker constant so downstream tooling can detect the macro has run without
//! changing the function's observable behaviour.
//
// TODO(phase 1, SPEC §2 / ROADMAP Phase 1): real expansion must
//   1. extract the function signature (name, inputs, output, generics) and
//      lower it into an IR fragment serialisable to `target/taut/ir.json`;
//   2. generate an axum handler that deserialises the request body
//      (or query string for `method = "GET"`), invokes the user fn, and
//      serialises the response per SPEC §4.1 / §4.2;
//   3. for `ProcKind::Stream`, wire the return type as an SSE/WebSocket
//      stream framed as documented in SPEC §4.2;
//   4. emit the procedure registration glue consumed by `Router::procedure`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse2, Ident, ItemFn, LitStr, Token,
};

/// Variant of procedure declared by the attribute. Recorded for use by the
/// real expansion in a later phase; unused for now beyond parse-validation.
#[allow(dead_code)]
enum ProcKind {
    Query,
    Mutation,
    Stream,
}

/// Parsed `#[rpc(...)]` arguments.
struct RpcArgs {
    kind: ProcKind,
}

impl Parse for RpcArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // `#[rpc]` — no arguments.
        if input.is_empty() {
            return Ok(Self {
                kind: ProcKind::Query,
            });
        }

        let lookahead = input.lookahead1();
        if lookahead.peek(Ident) {
            let ident: Ident = input.parse()?;
            if ident == "stream" {
                // `#[rpc(stream)]`
                if !input.is_empty() {
                    return Err(syn::Error::new(
                        input.span(),
                        "unexpected tokens after `stream`",
                    ));
                }
                return Ok(Self {
                    kind: ProcKind::Stream,
                });
            }
            if ident == "method" {
                // `#[rpc(method = "GET")]`
                input.parse::<Token![=]>()?;
                let lit: LitStr = input.parse()?;
                let value = lit.value();
                if value != "GET" {
                    return Err(syn::Error::new(
                        lit.span(),
                        format!(
                            "unsupported method `{value}`; only `\"GET\"` is recognised in phase 1"
                        ),
                    ));
                }
                if !input.is_empty() {
                    return Err(syn::Error::new(
                        input.span(),
                        "unexpected tokens after `method = \"GET\"`",
                    ));
                }
                // Method-tagged queries stay queries; the HTTP verb override
                // is recorded separately once the IR is wired up.
                return Ok(Self {
                    kind: ProcKind::Query,
                });
            }
            return Err(syn::Error::new(
                ident.span(),
                format!(
                    "unrecognised `#[rpc]` argument `{ident}`; expected `stream` or `method = \"GET\"`"
                ),
            ));
        }

        Err(lookahead.error())
    }
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args: RpcArgs = parse2(attr)?;
    let _kind = args.kind; // consumed by real expansion; silence unused warning here.

    let func: ItemFn = parse2(item)?;

    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            func.sig.fn_token,
            "`#[rpc]` requires an `async fn`",
        ));
    }

    Ok(quote! {
        #func

        // Hidden marker so downstream tooling can detect the macro has run.
        // Replaced by real registration glue in phase 1.
        const _: () = ();
    })
}

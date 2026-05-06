//! Expansion for the `#[rpc]` attribute macro (Phase 1).
//!
//! Given an `async fn` of one of the supported shapes, this macro
//!
//! 1. emits the original function unchanged, and
//! 2. emits a sibling `pub fn __taut_proc_<name>() -> ::taut_rpc::ProcedureDescriptor`
//!    that the runtime calls to register the procedure.
//!
//! The descriptor carries everything the server needs at request time
//! (the JSON-in / JSON-out handler) and everything the codegen step needs
//! at build time (the IR fragment plus reachable type definitions). See
//! SPEC §2 (architecture), §3.3 (errors), §4.1 (wire format).
//!
//! ## Supported shapes (Phase 1)
//!
//! ```ignore
//! #[rpc]                         // query, POST
//! #[rpc(mutation)]               // mutation, POST
//! #[rpc(stream)]                 // subscription — Phase 3, currently a stub
//! #[rpc(method = "GET")]         // accepted, currently still routed as POST
//! ```
//!
//! The function must be `async`, free-standing (not a method), non-generic,
//! and take either zero arguments or a single argument that is the RPC input.
//! Multi-argument procedures are deliberately rejected in v0.1 with a hint to
//! wrap them in a struct.
//!
//! Return types may be either `T` (success-only) or `Result<T, E>`. In the
//! `Result` case `E` is recorded in the IR as a procedure-level error type so
//! the generated TS client can narrow per-procedure error unions.
//!
//! Errors are surfaced via `syn::Error`; the macro never panics on user input.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse2,
    spanned::Spanned,
    FnArg, GenericArgument, Ident, ItemFn, LitStr, PathArguments, ReturnType, Token, Type,
    TypePath,
};

/// Variant of procedure declared by the attribute.
#[derive(Debug, Clone, Copy)]
enum ProcKind {
    Query,
    Mutation,
    Stream,
}

/// Parsed `#[rpc(...)]` arguments.
#[derive(Debug)]
struct RpcArgs {
    kind: ProcKind,
    /// `Some(span)` if the user wrote `method = "..."`. We accept the
    /// argument in Phase 1 but still emit a POST route; the span is kept for
    /// future diagnostics if we want to lint on it.
    _method_override: Option<Span>,
}

impl Parse for RpcArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // `#[rpc]` — no arguments.
        if input.is_empty() {
            return Ok(Self {
                kind: ProcKind::Query,
                _method_override: None,
            });
        }

        let mut kind = ProcKind::Query;
        let mut method_override: Option<Span> = None;

        // Comma-separated list of `ident` or `ident = "lit"`.
        loop {
            if input.is_empty() {
                break;
            }
            let ident: Ident = input.parse()?;
            if ident == "stream" {
                kind = ProcKind::Stream;
            } else if ident == "mutation" {
                kind = ProcKind::Mutation;
            } else if ident == "query" {
                kind = ProcKind::Query;
            } else if ident == "method" {
                input.parse::<Token![=]>()?;
                let lit: LitStr = input.parse()?;
                // Phase 1 still routes everything via POST; the value is
                // accepted to keep call sites stable but is otherwise unused.
                method_override = Some(lit.span());
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!(
                        "taut_rpc: unrecognised `#[rpc]` argument `{ident}`; \
                         expected one of `query`, `mutation`, `stream`, `method = \"...\"`"
                    ),
                ));
            }

            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }

        Ok(Self {
            kind,
            _method_override: method_override,
        })
    }
}

/// Outcome of inspecting the user's return type.
struct ReturnShape {
    /// The success type, e.g. `String` for `-> String` or `i32` for
    /// `-> Result<i32, AddError>`. Always present.
    success: Type,
    /// `Some(E)` if the function returns `Result<T, E>`, otherwise `None`.
    error: Option<Type>,
}

/// If `ty` is a path type whose final segment is `Result<A, B>`, returns
/// `Some((A, B))`. Otherwise returns `None`.
///
/// This intentionally only inspects the *final* path segment, so both bare
/// `Result<...>` and prefixed `std::result::Result<...>` are detected. Any
/// reference, slice, tuple, or other shape is not a `Result`.
fn match_result_type(ty: &Type) -> Option<(Type, Type)> {
    let TypePath { qself: None, path } = (match ty {
        Type::Path(p) => p,
        _ => return None,
    }) else {
        return None;
    };

    let last = path.segments.last()?;
    if last.ident != "Result" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    // Must be exactly two type arguments.
    let mut types = args.args.iter().filter_map(|a| match a {
        GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    });
    let ok = types.next()?;
    let err = types.next()?;
    if types.next().is_some() {
        return None;
    }
    Some((ok, err))
}

/// Classify the function's return type into success/error components.
fn classify_return(output: &ReturnType) -> ReturnShape {
    match output {
        ReturnType::Default => ReturnShape {
            success: syn::parse_quote!(()),
            error: None,
        },
        ReturnType::Type(_, ty) => match match_result_type(ty) {
            Some((ok, err)) => ReturnShape {
                success: ok,
                error: Some(err),
            },
            None => ReturnShape {
                success: (**ty).clone(),
                error: None,
            },
        },
    }
}

/// Find the single non-receiver argument, if any. Returns:
/// - `Ok(None)` for a zero-argument fn.
/// - `Ok(Some(ty))` for a one-argument fn (the input type).
/// - `Err(...)` for `&self` / multi-argument / pattern-typed forms we don't support.
fn extract_input_type(func: &ItemFn) -> syn::Result<Option<Type>> {
    let inputs = &func.sig.inputs;

    // Reject methods up front for a clearer message than the multi-arg path
    // would produce.
    if let Some(FnArg::Receiver(rcv)) = inputs.first() {
        return Err(syn::Error::new(
            rcv.span(),
            "taut_rpc: #[rpc] cannot be applied to methods; use a free-standing async fn",
        ));
    }

    match inputs.len() {
        0 => Ok(None),
        1 => match inputs.first().expect("len == 1") {
            FnArg::Typed(pat_type) => Ok(Some((*pat_type.ty).clone())),
            FnArg::Receiver(_) => unreachable!("handled above"),
        },
        _ => Err(syn::Error::new(
            inputs.span(),
            "taut_rpc: multi-argument procedures are not yet supported in v0.1; \
             wrap your arguments in a struct that derives Type and Deserialize",
        )),
    }
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args: RpcArgs = parse2(attr)?;
    let func: ItemFn = parse2(item)?;

    // Phase 3 — subscriptions are not implemented yet. Per spec, emit a
    // hard compile error pointing at the roadmap.
    if matches!(args.kind, ProcKind::Stream) {
        return Err(syn::Error::new(
            func.sig.fn_token.span(),
            "taut_rpc: subscriptions are not yet implemented; Phase 3 in ROADMAP.md",
        ));
    }

    // Validate the function shape.
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            func.sig.fn_token.span(),
            "taut_rpc: #[rpc] requires an async fn",
        ));
    }
    if !func.sig.generics.params.is_empty() || func.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            func.sig.generics.span(),
            "taut_rpc: generic procedures are not supported in v0.1; \
             monomorphise by writing a wrapper fn with concrete types",
        ));
    }
    if let Some(variadic) = &func.sig.variadic {
        return Err(syn::Error::new(
            variadic.span(),
            "taut_rpc: variadic procedures are not supported",
        ));
    }

    let fn_ident = func.sig.ident.clone();
    let fn_name_str = fn_ident.to_string();
    let descriptor_ident = Ident::new(
        &format!("__taut_proc_{fn_name_str}"),
        Span::call_site(),
    );

    let input_ty_opt = extract_input_type(&func)?;
    let return_shape = classify_return(&func.sig.output);

    // Build the IR `ProcKind` and runtime `ProcKindRuntime` tokens.
    let (ir_kind_tok, runtime_kind_tok) = match args.kind {
        ProcKind::Query => (
            quote!(::taut_rpc::ir::ProcKind::Query),
            quote!(::taut_rpc::ProcKindRuntime::Query),
        ),
        ProcKind::Mutation => (
            quote!(::taut_rpc::ir::ProcKind::Mutation),
            quote!(::taut_rpc::ProcKindRuntime::Mutation),
        ),
        // Stream is short-circuited above; this arm is here only so the
        // match is exhaustive without an `unreachable!`.
        ProcKind::Stream => (
            quote!(::taut_rpc::ir::ProcKind::Subscription),
            quote!(::taut_rpc::ProcKindRuntime::Subscription),
        ),
    };

    // Tokens for the input side of the descriptor.
    let input_ir_expr = match &input_ty_opt {
        Some(ty) => quote!(<#ty as ::taut_rpc::TautType>::ir_type_ref()),
        None => quote!(<() as ::taut_rpc::TautType>::ir_type_ref()),
    };
    let input_collect = match &input_ty_opt {
        Some(ty) => quote!(<#ty as ::taut_rpc::TautType>::collect_type_defs(&mut type_defs);),
        None => quote!(<() as ::taut_rpc::TautType>::collect_type_defs(&mut type_defs);),
    };

    // Tokens for the output (success) side of the descriptor.
    let success_ty = &return_shape.success;
    let output_ir_expr = quote!(<#success_ty as ::taut_rpc::TautType>::ir_type_ref());
    let output_collect =
        quote!(<#success_ty as ::taut_rpc::TautType>::collect_type_defs(&mut type_defs););

    // Tokens for the optional error type.
    let (errors_vec_expr, error_collect) = match &return_shape.error {
        Some(err_ty) => (
            quote!(vec![<#err_ty as ::taut_rpc::TautType>::ir_type_ref()]),
            quote!(<#err_ty as ::taut_rpc::TautType>::collect_type_defs(&mut type_defs);),
        ),
        None => (quote!(::std::vec::Vec::new()), quote!()),
    };

    // The handler body needs two distinct shapes depending on the input arity
    // and another two depending on whether the user fn returns `Result`.
    //
    // Decode step.
    let decode_block = if let Some(ty) = &input_ty_opt {
        quote! {
            let __input: #ty = match ::serde_json::from_value(__input_value) {
                ::std::result::Result::Ok(v) => v,
                ::std::result::Result::Err(e) => {
                    return ::taut_rpc::ProcedureResult::Err {
                        http_status: 400,
                        code: ::std::string::String::from("decode_error"),
                        payload: ::serde_json::json!({ "message": e.to_string() }),
                    };
                }
            };
        }
    } else {
        // Zero-arg procedures expect a JSON `null` body; anything else is a
        // client bug and we surface it the same way as a decode failure.
        quote! {
            if !__input_value.is_null() {
                return ::taut_rpc::ProcedureResult::Err {
                    http_status: 400,
                    code: ::std::string::String::from("decode_error"),
                    payload: ::serde_json::json!({ "message": "expected null input" }),
                };
            }
        }
    };

    // Call step (returns `__user_result` whose type matches the user fn).
    let call_expr = if input_ty_opt.is_some() {
        quote!(#fn_ident(__input).await)
    } else {
        quote!(#fn_ident().await)
    };

    // Result handling.
    let result_block = if return_shape.error.is_some() {
        quote! {
            match #call_expr {
                ::std::result::Result::Ok(__out) => match ::serde_json::to_value(&__out) {
                    ::std::result::Result::Ok(__v) => ::taut_rpc::ProcedureResult::Ok(__v),
                    ::std::result::Result::Err(__e) => ::taut_rpc::ProcedureResult::Err {
                        http_status: 500,
                        code: ::std::string::String::from("serialization_error"),
                        payload: ::serde_json::json!({ "message": __e.to_string() }),
                    },
                },
                ::std::result::Result::Err(__err) => match ::serde_json::to_value(&__err) {
                    ::std::result::Result::Ok(__payload) => {
                        // If the serialised error is a tagged object with a
                        // `code` field, surface it to the client; otherwise
                        // fall back to a generic discriminant. The TS side
                        // narrows on `code` regardless, so this only affects
                        // unexpected error shapes.
                        let __code = __payload
                            .get("code")
                            .and_then(|c| c.as_str())
                            .unwrap_or("error")
                            .to_string();
                        ::taut_rpc::ProcedureResult::Err {
                            http_status: 400,
                            code: __code,
                            payload: __payload,
                        }
                    }
                    ::std::result::Result::Err(__e) => ::taut_rpc::ProcedureResult::Err {
                        http_status: 500,
                        code: ::std::string::String::from("serialization_error"),
                        payload: ::serde_json::json!({ "message": __e.to_string() }),
                    },
                },
            }
        }
    } else {
        quote! {
            let __out = #call_expr;
            match ::serde_json::to_value(&__out) {
                ::std::result::Result::Ok(__v) => ::taut_rpc::ProcedureResult::Ok(__v),
                ::std::result::Result::Err(__e) => ::taut_rpc::ProcedureResult::Err {
                    http_status: 500,
                    code: ::std::string::String::from("serialization_error"),
                    payload: ::serde_json::json!({ "message": __e.to_string() }),
                },
            }
        }
    };

    // Pull the rustdoc string out of the function's `#[doc = "..."]` attrs so
    // the IR (and ultimately the generated TS) keeps the user's documentation.
    let doc_expr = extract_doc_expr(&func);

    let descriptor = quote! {
        #[allow(non_snake_case)]
        pub fn #descriptor_ident() -> ::taut_rpc::ProcedureDescriptor {
            let input_ty = #input_ir_expr;
            let output_ty = #output_ir_expr;
            let mut type_defs: ::std::vec::Vec<::taut_rpc::ir::TypeDef> =
                ::std::vec::Vec::new();
            #input_collect
            #output_collect
            #error_collect
            // Dedup type_defs by name, preserving first occurrence.
            {
                let mut seen = ::std::collections::HashSet::<::std::string::String>::new();
                type_defs.retain(|d| seen.insert(d.name.clone()));
            }
            ::taut_rpc::ProcedureDescriptor {
                name: #fn_name_str,
                kind: #runtime_kind_tok,
                ir: ::taut_rpc::ir::Procedure {
                    name: ::std::string::String::from(#fn_name_str),
                    kind: #ir_kind_tok,
                    input: input_ty,
                    output: output_ty,
                    errors: #errors_vec_expr,
                    http_method: ::taut_rpc::ir::HttpMethod::Post,
                    doc: #doc_expr,
                },
                type_defs,
                handler: ::std::sync::Arc::new(|__input_value: ::serde_json::Value| {
                    ::std::boxed::Box::pin(async move {
                        #decode_block
                        #result_block
                    })
                }),
            }
        }
    };

    Ok(quote! {
        #func

        #descriptor
    })
}

/// Concatenate `#[doc = "..."]` attributes on the function into a single
/// `Option<String>` token. Returns `None`-tokens if there are no doc attrs.
fn extract_doc_expr(func: &ItemFn) -> TokenStream {
    let mut lines: Vec<String> = Vec::new();
    for attr in &func.attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                lines.push(s.value());
            }
        }
    }
    if lines.is_empty() {
        quote!(::std::option::Option::None)
    } else {
        // Match rustdoc's own behaviour of stripping a single leading space
        // from each line and joining with newlines.
        let joined = lines
            .iter()
            .map(|l| l.strip_prefix(' ').unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
        quote!(::std::option::Option::Some(::std::string::String::from(#joined)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn match_result_recognises_bare_result() {
        let ty: Type = parse_quote!(Result<i32, MyError>);
        let (ok, err) = match_result_type(&ty).expect("should match");
        let ok_str = quote!(#ok).to_string();
        let err_str = quote!(#err).to_string();
        assert_eq!(ok_str, "i32");
        assert_eq!(err_str, "MyError");
    }

    #[test]
    fn match_result_recognises_qualified_result() {
        let ty: Type = parse_quote!(::std::result::Result<String, ()>);
        assert!(match_result_type(&ty).is_some());
    }

    #[test]
    fn match_result_rejects_non_result_path() {
        let ty: Type = parse_quote!(Option<i32>);
        assert!(match_result_type(&ty).is_none());
    }

    #[test]
    fn match_result_rejects_wrong_arity() {
        let ty: Type = parse_quote!(Result<i32>);
        assert!(match_result_type(&ty).is_none());
    }

    #[test]
    fn match_result_rejects_non_path_types() {
        let ty: Type = parse_quote!(&str);
        assert!(match_result_type(&ty).is_none());
        let ty: Type = parse_quote!((i32, String));
        assert!(match_result_type(&ty).is_none());
    }

    #[test]
    fn classify_return_default_is_unit_no_error() {
        let rt: ReturnType = ReturnType::Default;
        let shape = classify_return(&rt);
        assert!(shape.error.is_none());
        // The resulting type must round-trip to `()`.
        let success = shape.success;
        let rendered = quote!(#success).to_string();
        assert_eq!(rendered.replace(' ', ""), "()");
    }

    #[test]
    fn classify_return_plain_type() {
        let rt: ReturnType = parse_quote!(-> String);
        let shape = classify_return(&rt);
        assert!(shape.error.is_none());
        let success = shape.success;
        assert_eq!(quote!(#success).to_string(), "String");
    }

    #[test]
    fn classify_return_result_type() {
        let rt: ReturnType = parse_quote!(-> Result<i32, AddError>);
        let shape = classify_return(&rt);
        let success = shape.success;
        let err = shape.error.expect("error should be present");
        assert_eq!(quote!(#success).to_string(), "i32");
        assert_eq!(quote!(#err).to_string(), "AddError");
    }

    #[test]
    fn extract_input_type_zero_args() {
        let func: ItemFn = parse_quote!(
            async fn ping() -> String { String::new() }
        );
        let got = extract_input_type(&func).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn extract_input_type_one_arg() {
        let func: ItemFn = parse_quote!(
            async fn add(input: AddInput) -> i32 { 0 }
        );
        let got = extract_input_type(&func).unwrap().unwrap();
        assert_eq!(quote!(#got).to_string(), "AddInput");
    }

    #[test]
    fn extract_input_type_rejects_multi_arg() {
        let func: ItemFn = parse_quote!(
            async fn add(a: i32, b: i32) -> i32 { a + b }
        );
        let err = extract_input_type(&func).unwrap_err();
        assert!(err.to_string().contains("wrap your arguments in a struct"));
    }

    #[test]
    fn extract_input_type_rejects_self_receiver() {
        let func: ItemFn = parse_quote!(
            async fn ping(&self) -> String { String::new() }
        );
        let err = extract_input_type(&func).unwrap_err();
        assert!(err.to_string().contains("methods"));
    }

    #[test]
    fn rpc_args_default_is_query() {
        let args: RpcArgs = parse2(quote!()).unwrap();
        assert!(matches!(args.kind, ProcKind::Query));
    }

    #[test]
    fn rpc_args_mutation() {
        let args: RpcArgs = parse2(quote!(mutation)).unwrap();
        assert!(matches!(args.kind, ProcKind::Mutation));
    }

    #[test]
    fn rpc_args_stream() {
        let args: RpcArgs = parse2(quote!(stream)).unwrap();
        assert!(matches!(args.kind, ProcKind::Stream));
    }

    #[test]
    fn rpc_args_method_accepts_get() {
        let args: RpcArgs = parse2(quote!(method = "GET")).unwrap();
        assert!(matches!(args.kind, ProcKind::Query));
    }

    #[test]
    fn rpc_args_unknown_token_errors() {
        let err = parse2::<RpcArgs>(quote!(banana)).unwrap_err();
        assert!(err.to_string().contains("unrecognised"));
    }

    #[test]
    fn expand_rejects_non_async() {
        let item = quote! {
            fn ping() -> String { String::new() }
        };
        let err = expand(quote!(), item).unwrap_err();
        assert!(err.to_string().contains("requires an async fn"));
    }

    #[test]
    fn expand_rejects_generic_fn() {
        let item = quote! {
            async fn ping<T>() -> T { todo!() }
        };
        let err = expand(quote!(), item).unwrap_err();
        assert!(err.to_string().contains("generic"));
    }

    #[test]
    fn expand_stream_emits_phase3_error() {
        let item = quote! {
            async fn events() -> String { String::new() }
        };
        let err = expand(quote!(stream), item).unwrap_err();
        assert!(err.to_string().contains("Phase 3"));
    }

    #[test]
    fn expand_rejects_multi_arg() {
        let item = quote! {
            async fn add(a: i32, b: i32) -> i32 { a + b }
        };
        let err = expand(quote!(), item).unwrap_err();
        assert!(err.to_string().contains("wrap your arguments in a struct"));
    }

    #[test]
    fn expand_emits_descriptor_for_simple_fn() {
        let item = quote! {
            async fn ping() -> String { String::from("pong") }
        };
        let out = expand(quote!(), item).unwrap().to_string();
        assert!(out.contains("__taut_proc_ping"));
        assert!(out.contains("ProcedureDescriptor"));
        assert!(out.contains("ProcKindRuntime :: Query"));
    }
}

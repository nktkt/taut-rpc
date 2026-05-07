//! Expansion for the `#[rpc]` attribute macro (Phases 1–3).
//!
//! Given an `async fn` of one of the supported shapes, this macro
//!
//! 1. emits the original function unchanged, and
//! 2. emits a sibling `pub fn __taut_proc_<name>() -> ::taut_rpc::ProcedureDescriptor`
//!    that the runtime calls to register the procedure.
//!
//! The descriptor carries everything the server needs at request time
//! (the JSON-in / JSON-out unary handler, or the JSON-in / `StreamFrame`-out
//! subscription handler) and everything the codegen step needs at build time
//! (the IR fragment plus reachable type definitions). See SPEC §2
//! (architecture), §3.3 (errors), §4.1 (unary wire format), §4.2
//! (subscription wire format).
//!
//! ## Supported shapes
//!
//! ```ignore
//! #[rpc]                         // query, POST
//! #[rpc(mutation)]               // mutation, POST
//! #[rpc(stream)]                 // subscription, GET (Phase 3)
//! #[rpc(method = "GET")]         // accepted, currently still routed as POST
//! ```
//!
//! The function must be `async`, free-standing (not a method), non-generic,
//! and take either zero arguments or a single argument that is the RPC input.
//! Multi-argument procedures are deliberately rejected in v0.1 with a hint to
//! wrap them in a struct.
//!
//! Unary return types may be either `T` (success-only) or `Result<T, E>`. In
//! the `Result` case `E` is recorded in the IR as a procedure-level error type
//! so the generated TS client can narrow per-procedure error unions.
//!
//! Error types in `Result<T, E>` returns must implement `taut_rpc::TautError`
//! (use `#[derive(TautError)]`). The macro doesn't add an explicit
//! where-clause; the trait bound is enforced when the trait methods are called.
//!
//! Subscription return types must be of the form
//! `impl Stream<Item = T> [+ Send] [+ 'static]`. The trait path may be bare
//! (`Stream`), `futures::Stream`, or `::futures::Stream`. The `Item` associated
//! type binding is what defines the per-frame payload type.
//!
//! ## Crate dependency note
//!
//! The expansion for `#[rpc(stream)]` references `::async_stream::stream!` and
//! `::futures::{pin_mut, StreamExt}`. Users do not need to add
//! `async-stream`/`futures` to their own `Cargo.toml`: `taut-rpc` re-exports
//! both crates as part of its public dependency surface (Agent 1 added
//! `async-stream` to `taut-rpc`'s `[dependencies]`; `futures` was already
//! transitively pulled in via the runtime). The emitted paths are absolute
//! (`::async_stream::...` / `::futures::...`) so users only need to depend on
//! `taut-rpc` itself.
//!
//! Errors are surfaced via `syn::Error`; the macro never panics on user input.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    parse2,
    spanned::Spanned,
    FnArg, GenericArgument, Ident, ItemFn, LitStr, PathArguments, ReturnType, Token,
    TraitBoundModifier, Type, TypeImplTrait, TypeParamBound, TypePath,
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

/// For `#[rpc(stream)]`, dig the per-frame `Item = T` type out of an
/// `impl Stream<Item = T> [+ Send] [+ 'static]` return type.
///
/// Accepts the `Stream` trait under any path prefix — bare `Stream`,
/// `futures::Stream`, or `::futures::Stream` — by matching only on the final
/// path segment. Extra trait bounds (`Send`, `Sync`) and lifetime bounds
/// (`'static`) are tolerated and ignored: only the binding for the `Item`
/// associated type is consumed.
///
/// Returns the bound `T` on success. Returns `None` if the return type is
/// missing, isn't `impl Trait`, doesn't include a `Stream` trait bound, or
/// the `Stream` bound has no `Item = T` associated-type binding.
fn extract_stream_item(output: &ReturnType) -> Option<Type> {
    let ty = match output {
        ReturnType::Type(_, ty) => &**ty,
        ReturnType::Default => return None,
    };
    let Type::ImplTrait(TypeImplTrait { bounds, .. }) = ty else {
        return None;
    };

    for bound in bounds {
        let TypeParamBound::Trait(tb) = bound else {
            continue;
        };
        // We don't care about `?Sized`-style modifiers; just look at the path.
        if !matches!(tb.modifier, TraitBoundModifier::None) {
            continue;
        }
        let Some(last) = tb.path.segments.last() else {
            continue;
        };
        if last.ident != "Stream" {
            continue;
        }
        let PathArguments::AngleBracketed(args) = &last.arguments else {
            continue;
        };
        for arg in &args.args {
            if let GenericArgument::AssocType(assoc) = arg {
                if assoc.ident == "Item" {
                    return Some(assoc.ty.clone());
                }
            }
        }
    }
    None
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

/// Validation common to every `#[rpc]` shape: must be `async`, non-generic,
/// non-variadic. Method receivers are rejected later by `extract_input_type`
/// for a more specific message.
fn validate_common(func: &ItemFn) -> syn::Result<()> {
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
    Ok(())
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args: RpcArgs = parse2(attr)?;
    let func: ItemFn = parse2(item)?;

    validate_common(&func)?;

    match args.kind {
        ProcKind::Query | ProcKind::Mutation => expand_unary(&func, args.kind),
        ProcKind::Stream => expand_stream(&func),
    }
}

/// Expansion for `#[rpc]` / `#[rpc(mutation)]` — the unary (request →
/// response) shape. The emitted descriptor uses
/// [`taut_rpc::ProcedureBody::Unary`] and threads the user's fn through a
/// JSON decode → call → JSON encode pipeline.
fn expand_unary(func: &ItemFn, kind: ProcKind) -> syn::Result<TokenStream> {
    let fn_ident = func.sig.ident.clone();
    let fn_name_str = fn_ident.to_string();
    let descriptor_ident = Ident::new(&format!("__taut_proc_{fn_name_str}"), Span::call_site());

    let input_ty_opt = extract_input_type(func)?;
    let return_shape = classify_return(&func.sig.output);

    // Build the IR `ProcKind` and runtime `ProcKindRuntime` tokens.
    let (ir_kind_tok, runtime_kind_tok) = match kind {
        ProcKind::Query => (
            quote!(::taut_rpc::ir::ProcKind::Query),
            quote!(::taut_rpc::ProcKindRuntime::Query),
        ),
        ProcKind::Mutation => (
            quote!(::taut_rpc::ir::ProcKind::Mutation),
            quote!(::taut_rpc::ProcKindRuntime::Mutation),
        ),
        ProcKind::Stream => unreachable!("expand_unary called with ProcKind::Stream"),
    };

    // Tokens for the input side of the descriptor.
    let input_ir_expr = if let Some(ty) = &input_ty_opt {
        quote!(<#ty as ::taut_rpc::TautType>::ir_type_ref())
    } else {
        quote!(<() as ::taut_rpc::TautType>::ir_type_ref())
    };
    let input_collect = if let Some(ty) = &input_ty_opt {
        quote!(<#ty as ::taut_rpc::TautType>::collect_type_defs(&mut type_defs);)
    } else {
        quote!(<() as ::taut_rpc::TautType>::collect_type_defs(&mut type_defs);)
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
    // Decode step. For typed input we also run server-side validation per
    // SPEC §7: after a successful `serde_json::from_value`, we call
    // `<I as Validate>::validate(&__input)` and surface any failures as an
    // HTTP 400 with the `validation_error` envelope. Validate is required to
    // be implemented by the input type — primitives, `()`, `String`,
    // `Option<T>`, and `Vec<T>` get blanket impls in
    // `taut_rpc::validate` so users only need `#[derive(Validate)]` on their
    // own structs.
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
            if let ::std::result::Result::Err(__errors) =
                <#ty as ::taut_rpc::Validate>::validate(&__input)
            {
                return ::taut_rpc::ProcedureResult::Err {
                    http_status: 400,
                    code: ::std::string::String::from("validation_error"),
                    payload: ::serde_json::json!({ "errors": __errors }),
                };
            }
        }
    } else {
        // Zero-arg procedures expect a JSON `null` body; anything else is a
        // client bug and we surface it the same way as a decode failure.
        // No validation step: the input is `()`.
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
                ::std::result::Result::Err(__err) => {
                    // The bound `E: ::taut_rpc::TautError` is enforced
                    // implicitly by these method calls — without
                    // `#[derive(TautError)]` (or a hand-written impl) on `E`
                    // the user gets a "trait not satisfied" compile error.
                    let __code = ::taut_rpc::TautError::code(&__err).to_string();
                    let __http_status = ::taut_rpc::TautError::http_status(&__err);
                    let __payload = ::serde_json::to_value(&__err)
                        .unwrap_or(::serde_json::Value::Null);
                    ::taut_rpc::ProcedureResult::Err {
                        http_status: __http_status,
                        code: __code,
                        payload: __payload,
                    }
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
    let doc_expr = extract_doc_expr(func);

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
                body: ::taut_rpc::ProcedureBody::Unary(
                    ::std::sync::Arc::new(|__input_value: ::serde_json::Value| {
                        ::std::boxed::Box::pin(async move {
                            #decode_block
                            #result_block
                        })
                    })
                ),
            }
        }
    };

    Ok(quote! {
        #func

        #descriptor
    })
}

/// Expansion for `#[rpc(stream)]` — subscriptions.
///
/// The user fn is `async fn name(input?: I) -> impl Stream<Item = T>`. We
/// emit a [`taut_rpc::ProcedureBody::Stream`] handler that:
///
/// 1. Decodes `serde_json::Value` → `I` (or asserts JSON null for zero-arg).
/// 2. `await`s the user's async fn to obtain the inner `Stream`.
/// 3. Polls the inner stream and yields a [`taut_rpc::StreamFrame::Data`]
///    per item (serialized via `serde_json::to_value`), or a
///    [`taut_rpc::StreamFrame::Error`] on the first decode/serialization
///    failure followed by a `return` to terminate the stream.
///
/// Subscriptions are addressable as `GET` per SPEC §4.2 (so they can be
/// opened from an `EventSource`), distinct from the `POST` used by unary
/// query/mutation procedures.
fn expand_stream(func: &ItemFn) -> syn::Result<TokenStream> {
    let fn_ident = func.sig.ident.clone();
    let fn_name_str = fn_ident.to_string();
    let descriptor_ident = Ident::new(&format!("__taut_proc_{fn_name_str}"), Span::call_site());

    let input_ty_opt = extract_input_type(func)?;

    // The whole point of `#[rpc(stream)]` is the `impl Stream<Item = T>` shape.
    // Anything else is rejected with a hint that mirrors the docs.
    let item_ty = extract_stream_item(&func.sig.output).ok_or_else(|| {
        syn::Error::new(
            func.sig.output.span(),
            "taut_rpc: #[rpc(stream)] requires `async fn ... -> impl Stream<Item = T>`",
        )
    })?;

    // Tokens for the input side of the descriptor.
    let input_ir_expr = if let Some(ty) = &input_ty_opt {
        quote!(<#ty as ::taut_rpc::TautType>::ir_type_ref())
    } else {
        quote!(<() as ::taut_rpc::TautType>::ir_type_ref())
    };
    let input_collect = if let Some(ty) = &input_ty_opt {
        quote!(<#ty as ::taut_rpc::TautType>::collect_type_defs(&mut type_defs);)
    } else {
        quote!(<() as ::taut_rpc::TautType>::collect_type_defs(&mut type_defs);)
    };

    // Output is the per-frame `Item` type — that's what shows up in the IR
    // `output` slot for a subscription.
    let output_ir_expr = quote!(<#item_ty as ::taut_rpc::TautType>::ir_type_ref());
    let output_collect =
        quote!(<#item_ty as ::taut_rpc::TautType>::collect_type_defs(&mut type_defs););

    // Decode step inside the `async_stream::stream!` block.
    //
    // On decode failure we yield a single `StreamFrame::Error` and `return`.
    // The wire layer will close the SSE stream after sending the error event;
    // see SPEC §4.2.
    let decode_block = if let Some(ty) = &input_ty_opt {
        quote! {
            let __input: #ty = match ::serde_json::from_value(__input_value) {
                ::std::result::Result::Ok(v) => v,
                ::std::result::Result::Err(__e) => {
                    yield ::taut_rpc::StreamFrame::Error {
                        code: ::std::string::String::from("decode_error"),
                        payload: ::serde_json::json!({ "message": __e.to_string() }),
                    };
                    return;
                }
            };
            // Server-side validation per SPEC §7. On failure, emit a single
            // `StreamFrame::Error` and terminate the stream (the wire layer
            // closes the SSE connection after the error event).
            if let ::std::result::Result::Err(__errors) =
                <#ty as ::taut_rpc::Validate>::validate(&__input)
            {
                yield ::taut_rpc::StreamFrame::Error {
                    code: ::std::string::String::from("validation_error"),
                    payload: ::serde_json::json!({ "errors": __errors }),
                };
                return;
            }
        }
    } else {
        quote! {
            if !__input_value.is_null() {
                yield ::taut_rpc::StreamFrame::Error {
                    code: ::std::string::String::from("decode_error"),
                    payload: ::serde_json::json!({ "message": "expected null input" }),
                };
                return;
            }
        }
    };

    // Call step: `await` the user's async fn to obtain the inner stream.
    let call_expr = if input_ty_opt.is_some() {
        quote!(#fn_ident(__input).await)
    } else {
        quote!(#fn_ident().await)
    };

    let doc_expr = extract_doc_expr(func);

    let descriptor = quote! {
        #[allow(non_snake_case)]
        pub fn #descriptor_ident() -> ::taut_rpc::ProcedureDescriptor {
            let input_ty = #input_ir_expr;
            let output_ty = #output_ir_expr;
            let mut type_defs: ::std::vec::Vec<::taut_rpc::ir::TypeDef> =
                ::std::vec::Vec::new();
            #input_collect
            #output_collect
            // Dedup type_defs by name, preserving first occurrence.
            {
                let mut seen = ::std::collections::HashSet::<::std::string::String>::new();
                type_defs.retain(|d| seen.insert(d.name.clone()));
            }
            ::taut_rpc::ProcedureDescriptor {
                name: #fn_name_str,
                kind: ::taut_rpc::ProcKindRuntime::Subscription,
                ir: ::taut_rpc::ir::Procedure {
                    name: ::std::string::String::from(#fn_name_str),
                    kind: ::taut_rpc::ir::ProcKind::Subscription,
                    input: input_ty,
                    output: output_ty,
                    // Subscriptions never declare per-procedure error types in
                    // v0.1: errors at the stream level are encoded as
                    // `StreamFrame::Error { code, payload }` per SPEC §4.2.
                    errors: ::std::vec::Vec::new(),
                    // Subscriptions are GET so they can be opened by an
                    // EventSource / equivalent (SPEC §4.2).
                    http_method: ::taut_rpc::ir::HttpMethod::Get,
                    doc: #doc_expr,
                },
                type_defs,
                body: ::taut_rpc::ProcedureBody::Stream(
                    ::std::sync::Arc::new(|__input_value: ::serde_json::Value| {
                        ::std::boxed::Box::pin(::async_stream::stream! {
                            #decode_block

                            // Await the user's async fn to obtain the stream,
                            // then pin it on the local stack so we can call
                            // `.next()` against `&mut`.
                            let __inner = #call_expr;
                            ::futures::pin_mut!(__inner);
                            while let ::std::option::Option::Some(__item) =
                                ::futures::StreamExt::next(&mut __inner).await
                            {
                                match ::serde_json::to_value(&__item) {
                                    ::std::result::Result::Ok(__v) => {
                                        yield ::taut_rpc::StreamFrame::Data(__v);
                                    }
                                    ::std::result::Result::Err(__e) => {
                                        yield ::taut_rpc::StreamFrame::Error {
                                            code: ::std::string::String::from("serialization_error"),
                                            payload: ::serde_json::json!({ "message": __e.to_string() }),
                                        };
                                        return;
                                    }
                                }
                            }
                        })
                    })
                ),
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
            async fn ping() -> String {
                String::new()
            }
        );
        let got = extract_input_type(&func).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn extract_input_type_one_arg() {
        let func: ItemFn = parse_quote!(
            async fn add(input: AddInput) -> i32 {
                0
            }
        );
        let got = extract_input_type(&func).unwrap().unwrap();
        assert_eq!(quote!(#got).to_string(), "AddInput");
    }

    #[test]
    fn extract_input_type_rejects_multi_arg() {
        let func: ItemFn = parse_quote!(
            async fn add(a: i32, b: i32) -> i32 {
                a + b
            }
        );
        let err = extract_input_type(&func).unwrap_err();
        assert!(err.to_string().contains("wrap your arguments in a struct"));
    }

    #[test]
    fn extract_input_type_rejects_self_receiver() {
        let func: ItemFn = parse_quote!(
            async fn ping(&self) -> String {
                String::new()
            }
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
        // Phase 3 contract: unary descriptors are wrapped in `ProcedureBody::Unary`.
        assert!(
            out.contains("ProcedureBody :: Unary"),
            "expected unary handler to be wrapped in ProcedureBody::Unary, got: {out}"
        );
    }

    #[test]
    fn expand_result_fn_uses_taut_error_trait_methods() {
        // For a `Result<T, E>`-returning fn, the emitted handler must obtain
        // the wire `code` and `http_status` through the `TautError` trait
        // rather than by poking at the serialized payload.
        let item = quote! {
            async fn fails() -> Result<i32, MyErr> { todo!() }
        };
        let out = expand(quote!(), item).unwrap().to_string();
        assert!(
            out.contains("TautError :: code (& __err)"),
            "expected TautError::code(&__err) call in emitted tokens, got: {out}"
        );
        assert!(
            out.contains("TautError :: http_status (& __err)"),
            "expected TautError::http_status(&__err) call in emitted tokens, got: {out}"
        );
        // The Phase 1 hack (poking the serialized payload's `"code"` field)
        // must be gone.
        assert!(
            !out.contains(". get (\"code\")"),
            "expected the JSON-poking `__payload.get(\"code\")` lookup to be removed, got: {out}"
        );
        // And the Phase 1 fallback `"error"` discriminant must be gone too,
        // since the trait method always supplies a real code.
        assert!(
            !out.contains("unwrap_or (\"error\")"),
            "expected the `unwrap_or(\"error\")` fallback to be removed, got: {out}"
        );
    }

    // ----- Phase 3: subscription expansion -----

    #[test]
    fn extract_stream_item_from_bare_stream() {
        // `impl Stream<Item = u64>` → u64
        let rt: ReturnType = parse_quote!(-> impl Stream<Item = u64>);
        let item = extract_stream_item(&rt).expect("should extract Item");
        assert_eq!(quote!(#item).to_string(), "u64");
    }

    #[test]
    fn extract_stream_item_from_qualified_path() {
        // `impl ::futures::Stream<Item = String> + Send + 'static` → String
        let rt: ReturnType =
            parse_quote!(-> impl ::futures::Stream<Item = String> + Send + 'static);
        let item = extract_stream_item(&rt).expect("should extract Item");
        assert_eq!(quote!(#item).to_string(), "String");
    }

    #[test]
    fn extract_stream_item_from_futures_path() {
        let rt: ReturnType = parse_quote!(-> impl futures::Stream<Item = MyMsg> + Send);
        let item = extract_stream_item(&rt).expect("should extract Item");
        assert_eq!(quote!(#item).to_string(), "MyMsg");
    }

    #[test]
    fn extract_stream_item_returns_none_for_plain_type() {
        let rt: ReturnType = parse_quote!(-> u64);
        assert!(extract_stream_item(&rt).is_none());
    }

    #[test]
    fn extract_stream_item_returns_none_when_item_binding_missing() {
        // `impl Stream + Send` (no `Item =` binding) — we can't recover T.
        let rt: ReturnType = parse_quote!(-> impl Stream + Send);
        assert!(extract_stream_item(&rt).is_none());
    }

    #[test]
    fn expand_stream_emits_subscription_descriptor() {
        let item = quote! {
            async fn ticks(input: TicksInput) -> impl futures::Stream<Item = u64> + Send + 'static {
                ::futures::stream::empty()
            }
        };
        let out = expand(quote!(stream), item).unwrap().to_string();
        // Descriptor / fn naming.
        assert!(out.contains("__taut_proc_ticks"));
        assert!(out.contains("ProcedureDescriptor"));
        // Subscription-shaped IR + runtime kinds.
        assert!(
            out.contains("ProcKindRuntime :: Subscription"),
            "expected runtime kind Subscription, got: {out}"
        );
        assert!(
            out.contains("ProcKind :: Subscription"),
            "expected IR kind Subscription, got: {out}"
        );
        // Subscriptions are addressed via GET per SPEC §4.2.
        assert!(
            out.contains("HttpMethod :: Get"),
            "expected HttpMethod::Get for subscription, got: {out}"
        );
        // Body is the streaming variant, built around `async_stream::stream!`.
        assert!(
            out.contains("ProcedureBody :: Stream"),
            "expected ProcedureBody::Stream wrapping, got: {out}"
        );
        assert!(
            out.contains("async_stream :: stream"),
            "expected async_stream::stream! invocation, got: {out}"
        );
        // The `Item = u64` binding flows into the IR `output_ty` slot.
        assert!(
            out.contains("< u64 as :: taut_rpc :: TautType > :: ir_type_ref"),
            "expected ir_type_ref<u64> for the Item type, got: {out}"
        );
    }

    #[test]
    fn expand_stream_emits_descriptor_for_zero_input_fn() {
        let item = quote! {
            async fn server_time() -> impl futures::Stream<Item = String> + Send + 'static {
                ::futures::stream::empty()
            }
        };
        let out = expand(quote!(stream), item).unwrap().to_string();
        assert!(out.contains("__taut_proc_server_time"));
        // Zero-arg path: we assert the input is JSON null and yield a
        // decode_error otherwise.
        assert!(
            out.contains("is_null"),
            "expected zero-arg path to assert input_value.is_null(), got: {out}"
        );
        assert!(
            out.contains("StreamFrame :: Error"),
            "expected StreamFrame::Error emission for invalid zero-arg input, got: {out}"
        );
    }

    #[test]
    fn expand_stream_rejects_non_async() {
        // `fn` (no async) → standard "requires an async fn" error.
        let item = quote! {
            fn ticks() -> impl Stream<Item = u64> { ::futures::stream::empty() }
        };
        let err = expand(quote!(stream), item).unwrap_err();
        assert!(
            err.to_string().contains("requires an async fn"),
            "expected user-friendly async-fn error, got: {err}"
        );
    }

    #[test]
    fn expand_stream_rejects_non_stream_return() {
        // `-> u64` with `#[rpc(stream)]` is invalid.
        let item = quote! {
            async fn ticks() -> u64 { 0 }
        };
        let err = expand(quote!(stream), item).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("impl Stream<Item = T>"),
            "expected error pointing at `impl Stream<Item = T>`, got: {msg}"
        );
    }

    // ----- Phase 4: server-side input validation hook -----

    #[test]
    fn expand_unary_emits_validate_call_for_typed_input() {
        // For a unary procedure with a typed input, the emitted handler must
        // call `<I as Validate>::validate(&__input)` after deserialization
        // and surface the SPEC §7 envelope on failure.
        let item = quote! {
            async fn add(input: AddInput) -> i32 { 0 }
        };
        let out = expand(quote!(), item).unwrap().to_string();
        assert!(
            out.contains("< AddInput as :: taut_rpc :: Validate > :: validate (& __input)"),
            "expected `<AddInput as ::taut_rpc::Validate>::validate(&__input)` call, got: {out}"
        );
        // SPEC §7 envelope: code is `validation_error`, status 400, payload
        // carries the `errors` array.
        assert!(
            out.contains("\"validation_error\""),
            "expected `validation_error` code in emitted tokens, got: {out}"
        );
        assert!(
            out.contains("http_status : 400"),
            "expected HTTP 400 status for validation failure, got: {out}"
        );
        assert!(
            out.contains("\"errors\""),
            "expected `errors` field in validation_error payload, got: {out}"
        );
    }

    #[test]
    fn expand_unary_skips_validate_for_zero_input_fn() {
        // Zero-input procedures have no `__input` to validate; the macro must
        // not emit a `Validate::validate` call.
        let item = quote! {
            async fn ping() -> String { String::new() }
        };
        let out = expand(quote!(), item).unwrap().to_string();
        assert!(
            !out.contains(":: taut_rpc :: Validate"),
            "expected no Validate call for zero-input fn, got: {out}"
        );
    }

    #[test]
    fn expand_stream_emits_validate_call_for_typed_input() {
        // Subscriptions run validation just before invoking the user's async
        // fn; on failure they yield a single `StreamFrame::Error` and return.
        let item = quote! {
            async fn ticks(input: TicksInput) -> impl futures::Stream<Item = u64> + Send + 'static {
                ::futures::stream::empty()
            }
        };
        let out = expand(quote!(stream), item).unwrap().to_string();
        assert!(
            out.contains("< TicksInput as :: taut_rpc :: Validate > :: validate (& __input)"),
            "expected `<TicksInput as ::taut_rpc::Validate>::validate(&__input)` call, got: {out}"
        );
        assert!(
            out.contains("StreamFrame :: Error"),
            "expected StreamFrame::Error emission for validation failure, got: {out}"
        );
        assert!(
            out.contains("\"validation_error\""),
            "expected `validation_error` code in emitted tokens, got: {out}"
        );
    }

    #[test]
    fn expand_stream_skips_validate_for_zero_input_fn() {
        let item = quote! {
            async fn server_time() -> impl futures::Stream<Item = String> + Send + 'static {
                ::futures::stream::empty()
            }
        };
        let out = expand(quote!(stream), item).unwrap().to_string();
        assert!(
            !out.contains(":: taut_rpc :: Validate"),
            "expected no Validate call for zero-input subscription, got: {out}"
        );
    }
}

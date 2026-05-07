//! # taut-rpc
//!
//! End-to-end type-safe RPC between a Rust/axum server and a TypeScript client.
//!
//! `taut-rpc` makes Rust types the single source of truth for an HTTP/SSE/WebSocket
//! API: a procedural macro records each procedure's signature into an intermediate
//! representation (IR), and a separate codegen step turns that IR into a static
//! `.ts` client. Changing a Rust signature surfaces as a TypeScript compile error
//! at the call site, with no runtime schema fetch on boot.
//!
//! This crate provides the public runtime API: the [`Router`] used to mount
//! procedures onto an `axum::Router`, the [`TautError`] trait that constrains
//! procedure error types, and the [`Validate`] bridge for input/output validation.
//!
//! ## v0.1 features
//!
//! - **Validation** (Phase 4): the [`Validate`] derive macro and [`validate`]
//!   module provide declarative input/output validation. Constraints are
//!   declared with `#[validate(...)]` attributes and emitted into the IR so
//!   the generated TypeScript client can mirror server-side checks. See
//!   [`validate::run`], [`validate::collect`], [`validate::nested`], and
//!   [`validate::check`] for the runtime helpers used by generated code.
//!
//! See [`SPEC.md`](https://github.com/anthropics/taut-rpc/blob/main/SPEC.md) for
//! the full design — wire format, type mapping, IR schema, and versioning rules.
//! The [`IR_VERSION`] constant tracks SPEC §9.
//!
//! For ergonomic imports use `use taut_rpc::prelude::*;` — see [`prelude`].

#![warn(missing_docs)]

pub mod prelude;

pub mod dump;
pub mod error;
pub mod ir;
pub mod procedure;
pub mod router;
pub mod type_map;
pub mod types;
pub mod validate;
pub mod wire;

pub use dump::{dump_if_requested, ir_json};
pub use error::{StandardError, TautError};
pub use procedure::{
    ProcedureBody, ProcedureDescriptor, ProcedureHandler, ProcedureResult, StreamFrame,
    StreamHandler, UnaryHandler,
};
pub use router::{ProcKindRuntime, Router};
pub use types::TautType;
pub use validate::{Constraint, Validate, ValidationError};

pub use taut_rpc_macros::{rpc, TautError, Type, Validate};

/// Current IR schema version. Tracks SPEC §9 — codegen rejects mismatches.
pub const IR_VERSION: u32 = 1;

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
//! See [`SPEC.md`](https://github.com/anthropics/taut-rpc/blob/main/SPEC.md) for
//! the full design — wire format, type mapping, IR schema, and versioning rules.
//! The [`IR_VERSION`] constant tracks SPEC §9.

pub mod ir;
pub mod type_map;
pub mod wire;
pub mod error;
pub mod router;
pub mod validate;
pub mod types;
pub mod procedure;
pub mod dump;

pub use error::{StandardError, TautError};
pub use procedure::{ProcedureDescriptor, ProcedureHandler, ProcedureResult};
pub use router::{ProcKindRuntime, Router};
pub use types::TautType;
pub use validate::{Constraint, Validate, ValidationError};
pub use dump::{dump_if_requested, ir_json};

pub use taut_rpc_macros::{rpc, Type, TautError};

pub const IR_VERSION: u32 = 0;

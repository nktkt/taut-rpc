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

pub mod error;
pub mod ir;
pub mod router;
pub mod type_map;
pub mod validate;
pub mod wire;

pub use error::TautError;
pub use router::Router;
pub use validate::{Validate, ValidationError};

pub const IR_VERSION: u32 = 0;

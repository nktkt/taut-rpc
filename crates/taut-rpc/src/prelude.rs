//! Convenient re-exports for typical taut-rpc users.
//!
//! ```rust
//! use taut_rpc::prelude::*;
//! ```
//!
//! Brings into scope the most commonly used names: the macros, the trait, and
//! the types you need to wire a Router. Doesn't cover every export — for
//! advanced uses (custom transports, IR introspection) reach for the regular
//! crate-root re-exports.

pub use crate::dump_if_requested;
pub use crate::Router;
pub use crate::TautError;
pub use crate::TautType;
pub use crate::Validate;
pub use crate::{rpc, Type};
// `Validate` derive macro shares its name with the trait — both resolve.

//! Error contract for taut-rpc procedures. See SPEC §3.3.
//!
//! A `#[rpc]` function returns `Result<T, E>` where `E: TautError`. On the wire,
//! errors are serialized per SPEC §4.1 as:
//!
//! ```json
//! { "err": { "code": "...", "payload": ... } }
//! ```
//!
//! Implementations of [`TautError`] supply a stable string [`code`](TautError::code)
//! per variant and a `Serialize` payload. The HTTP status code is also chosen by
//! the implementation (default `400`).
//!
//! # TODO (ROADMAP Phase 2)
//!
//! A `#[derive(taut_rpc_macros::TautError)]` derive macro will be provided so users
//! can write:
//!
//! ```ignore
//! #[derive(taut_rpc_macros::TautError, serde::Serialize)]
//! #[serde(tag = "code", content = "payload", rename_all = "snake_case")]
//! enum MyError {
//!     #[taut(http = 404)]
//!     NotFound,
//!     #[taut(http = 409)]
//!     Conflict { detail: String },
//! }
//! ```
//!
//! and have it expand to an impl equivalent to the hand-written one for
//! [`StandardError`] in this module. The derive does not yet exist.

use serde::Serialize;

/// Procedure-level error type. Implementations give every variant a stable string `code`
/// and a `Serialize` payload that ends up in the JSON wire format as
/// `{ "err": { "code": "...", "payload": ... } }`.
pub trait TautError: Serialize + Sized {
    /// Stable, machine-readable code. SHOULD be lowercase snake_case.
    fn code(&self) -> &'static str;

    /// HTTP status code this error maps to. Default `400`.
    fn http_status(&self) -> u16 {
        400
    }
}

/// Built-in standard errors. Procedures may use these directly or wrap them.
///
/// Per SPEC §8 the `unauthenticated` discriminant is reserved.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum StandardError {
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("forbidden: {reason}")]
    Forbidden { reason: String },
    #[error("not found")]
    NotFound,
    #[error("rate limited (retry after {retry_after_seconds}s)")]
    RateLimited { retry_after_seconds: u32 },
    #[error("internal error")]
    Internal,
}

impl TautError for StandardError {
    fn code(&self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden { .. } => "forbidden",
            Self::NotFound => "not_found",
            Self::RateLimited { .. } => "rate_limited",
            Self::Internal => "internal",
        }
    }

    fn http_status(&self) -> u16 {
        match self {
            Self::Unauthenticated => 401,
            Self::Forbidden { .. } => 403,
            Self::NotFound => 404,
            Self::RateLimited { .. } => 429,
            Self::Internal => 500,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_unauthenticated() {
        assert_eq!(StandardError::Unauthenticated.code(), "unauthenticated");
    }

    #[test]
    fn code_forbidden() {
        assert_eq!(
            StandardError::Forbidden {
                reason: "x".into()
            }
            .code(),
            "forbidden"
        );
    }

    #[test]
    fn code_not_found() {
        assert_eq!(StandardError::NotFound.code(), "not_found");
    }

    #[test]
    fn code_rate_limited() {
        assert_eq!(
            StandardError::RateLimited {
                retry_after_seconds: 5
            }
            .code(),
            "rate_limited"
        );
    }

    #[test]
    fn code_internal() {
        assert_eq!(StandardError::Internal.code(), "internal");
    }

    #[test]
    fn http_status_unauthenticated() {
        assert_eq!(StandardError::Unauthenticated.http_status(), 401);
    }

    #[test]
    fn http_status_forbidden() {
        assert_eq!(
            StandardError::Forbidden {
                reason: "x".into()
            }
            .http_status(),
            403
        );
    }

    #[test]
    fn http_status_not_found() {
        assert_eq!(StandardError::NotFound.http_status(), 404);
    }

    #[test]
    fn http_status_rate_limited() {
        assert_eq!(
            StandardError::RateLimited {
                retry_after_seconds: 5
            }
            .http_status(),
            429
        );
    }

    #[test]
    fn http_status_internal() {
        assert_eq!(StandardError::Internal.http_status(), 500);
    }

    #[test]
    fn serialize_forbidden_contains_code_and_payload() {
        let err = StandardError::Forbidden {
            reason: "test".into(),
        };
        let json = serde_json::to_string(&err).expect("serialize StandardError");
        assert!(
            json.contains("\"code\":\"forbidden\""),
            "expected code field in {json}"
        );
        assert!(
            json.contains("\"reason\":\"test\""),
            "expected payload reason in {json}"
        );
    }
}

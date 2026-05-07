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

use crate::validate::ValidationError;

/// Procedure-level error type. Implementations give every variant a stable string `code`
/// and a `Serialize` payload that ends up in the JSON wire format as
/// `{ "err": { "code": "...", "payload": ... } }`.
///
/// # Examples
///
/// The recommended way to define a domain-specific error is via the
/// `#[derive(taut_rpc::TautError)]` macro:
///
/// ```rust,ignore
/// use taut_rpc::TautError;
///
/// #[derive(serde::Serialize, taut_rpc::TautError, Debug)]
/// #[serde(tag = "code", content = "payload", rename_all = "snake_case")]
/// pub enum AddError {
///     #[taut(status = 400)]
///     Overflow,
/// }
/// ```
///
/// A hand-written impl looks the same as what the derive expands to —
/// match each variant to its stable `code` and HTTP status:
///
/// ```rust,ignore
/// use taut_rpc::TautError;
///
/// #[derive(serde::Serialize)]
/// #[serde(tag = "code", content = "payload", rename_all = "snake_case")]
/// pub enum AddError { Overflow }
///
/// impl TautError for AddError {
///     fn code(&self) -> &'static str { match self { Self::Overflow => "overflow" } }
///     fn http_status(&self) -> u16 { 400 }
/// }
/// ```
///
/// For errors that map cleanly onto common HTTP semantics, prefer the built-in
/// [`StandardError`] taxonomy.
pub trait TautError: Serialize + Sized {
    /// Stable, machine-readable code. SHOULD be lowercase `snake_case`.
    fn code(&self) -> &'static str;

    /// HTTP status code this error maps to. Default `400`.
    fn http_status(&self) -> u16 {
        400
    }
}

/// Built-in standard errors. Procedures may use these directly or wrap them.
///
/// This is a curated set of "common" RPC errors that map cleanly onto well-known
/// HTTP status codes. The full taxonomy is:
///
/// | Variant                | Code                    | HTTP |
/// |------------------------|-------------------------|------|
/// | `BadRequest`           | `bad_request`           | 400  |
/// | `ValidationFailed`     | `validation_error`      | 400  |
/// | `Unauthenticated`      | `unauthenticated`       | 401  |
/// | `Forbidden`            | `forbidden`             | 403  |
/// | `NotFound`             | `not_found`             | 404  |
/// | `Conflict`             | `conflict`              | 409  |
/// | `UnprocessableEntity`  | `unprocessable_entity`  | 422  |
/// | `RateLimited`          | `rate_limited`          | 429  |
/// | `Internal`             | `internal`              | 500  |
/// | `ServiceUnavailable`   | `service_unavailable`   | 503  |
/// | `Timeout`              | `timeout`               | 504  |
///
/// Note: `ValidationFailed` carries a list of [`ValidationError`] entries and
/// is emitted by the server router when input validation rejects a request
/// before the procedure runs. Its discriminant is `validation_error` (not
/// `validation_failed`) to match the wire contract.
///
/// # Design principle
///
/// `StandardError` is intentionally narrow: it covers the cross-cutting concerns
/// every RPC stack tends to hit (auth, rate limiting, transport-shaped failures)
/// and nothing else. Anything domain-specific — business-rule violations,
/// per-procedure failure modes, structured validation results — should be its
/// own error enum with `#[derive(taut_rpc::TautError)]`. Reaching for
/// `StandardError` to model domain errors collapses meaningful distinctions
/// into a single bucket and is an anti-pattern.
///
/// Per SPEC §8 the `unauthenticated` discriminant is reserved.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum StandardError {
    /// 400 — Malformed or syntactically invalid request.
    #[error("bad request: {message}")]
    BadRequest {
        /// Human-readable description of why the request was rejected.
        message: String,
    },
    /// 400 — Server-side input validation rejected the request before the
    /// procedure ran. Carries the per-field failures that the validator
    /// collected. Serializes with the `validation_error` discriminant.
    #[error("validation failed")]
    #[serde(rename = "validation_error")]
    ValidationFailed {
        /// Per-field validation failures collected by the validator.
        errors: Vec<ValidationError>,
    },
    /// 401 — Caller is not authenticated.
    #[error("unauthenticated")]
    Unauthenticated,
    /// 403 — Caller is authenticated but not permitted.
    #[error("forbidden: {reason}")]
    Forbidden {
        /// Human-readable explanation of why the caller was denied.
        reason: String,
    },
    /// 404 — Target resource does not exist.
    #[error("not found")]
    NotFound,
    /// 409 — State conflict (e.g. unique-key violation, optimistic-lock failure).
    #[error("conflict: {message}")]
    Conflict {
        /// Human-readable description of the conflict.
        message: String,
    },
    /// 422 — Request was syntactically valid but failed semantic validation.
    #[error("unprocessable entity: {message}")]
    UnprocessableEntity {
        /// Human-readable description of the semantic failure.
        message: String,
    },
    /// 429 — Caller is being rate limited.
    #[error("rate limited (retry after {retry_after_seconds}s)")]
    RateLimited {
        /// Suggested delay before the caller retries, in seconds.
        retry_after_seconds: u32,
    },
    /// 500 — Unexpected server-side failure.
    #[error("internal error")]
    Internal,
    /// 503 — Service is temporarily unavailable (graceful degradation, deploys, etc.).
    #[error("service unavailable (retry after {retry_after_seconds}s)")]
    ServiceUnavailable {
        /// Suggested delay before the caller retries, in seconds.
        retry_after_seconds: u32,
    },
    /// 504 — Upstream or internal operation timed out.
    #[error("timeout")]
    Timeout,
}

impl TautError for StandardError {
    fn code(&self) -> &'static str {
        match self {
            Self::BadRequest { .. } => "bad_request",
            Self::ValidationFailed { .. } => "validation_error",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden { .. } => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict { .. } => "conflict",
            Self::UnprocessableEntity { .. } => "unprocessable_entity",
            Self::RateLimited { .. } => "rate_limited",
            Self::Internal => "internal",
            Self::ServiceUnavailable { .. } => "service_unavailable",
            Self::Timeout => "timeout",
        }
    }

    #[allow(clippy::match_same_arms)] // arms kept distinct for variant-to-status traceability
    fn http_status(&self) -> u16 {
        match self {
            Self::BadRequest { .. } => 400,
            Self::ValidationFailed { .. } => 400,
            Self::Unauthenticated => 401,
            Self::Forbidden { .. } => 403,
            Self::NotFound => 404,
            Self::Conflict { .. } => 409,
            Self::UnprocessableEntity { .. } => 422,
            Self::RateLimited { .. } => 429,
            Self::Internal => 500,
            Self::ServiceUnavailable { .. } => 503,
            Self::Timeout => 504,
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
            StandardError::Forbidden { reason: "x".into() }.code(),
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
            StandardError::Forbidden { reason: "x".into() }.http_status(),
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

    #[test]
    fn code_bad_request() {
        assert_eq!(
            StandardError::BadRequest {
                message: "x".into()
            }
            .code(),
            "bad_request"
        );
    }

    #[test]
    fn code_conflict() {
        assert_eq!(
            StandardError::Conflict {
                message: "x".into()
            }
            .code(),
            "conflict"
        );
    }

    #[test]
    fn code_unprocessable_entity() {
        assert_eq!(
            StandardError::UnprocessableEntity {
                message: "x".into()
            }
            .code(),
            "unprocessable_entity"
        );
    }

    #[test]
    fn code_service_unavailable() {
        assert_eq!(
            StandardError::ServiceUnavailable {
                retry_after_seconds: 5
            }
            .code(),
            "service_unavailable"
        );
    }

    #[test]
    fn code_timeout() {
        assert_eq!(StandardError::Timeout.code(), "timeout");
    }

    #[test]
    fn http_status_bad_request() {
        assert_eq!(
            StandardError::BadRequest {
                message: "x".into()
            }
            .http_status(),
            400
        );
    }

    #[test]
    fn http_status_conflict() {
        assert_eq!(
            StandardError::Conflict {
                message: "x".into()
            }
            .http_status(),
            409
        );
    }

    #[test]
    fn http_status_unprocessable_entity() {
        assert_eq!(
            StandardError::UnprocessableEntity {
                message: "x".into()
            }
            .http_status(),
            422
        );
    }

    #[test]
    fn http_status_service_unavailable() {
        assert_eq!(
            StandardError::ServiceUnavailable {
                retry_after_seconds: 5
            }
            .http_status(),
            503
        );
    }

    #[test]
    fn http_status_timeout() {
        assert_eq!(StandardError::Timeout.http_status(), 504);
    }

    #[test]
    fn serialize_bad_request_contains_code_and_message() {
        let err = StandardError::BadRequest {
            message: "x".into(),
        };
        let json = serde_json::to_string(&err).expect("serialize StandardError");
        assert!(
            json.contains("\"code\":\"bad_request\""),
            "expected code field in {json}"
        );
        assert!(
            json.contains("\"message\":\"x\""),
            "expected payload message in {json}"
        );
    }

    #[test]
    fn code_validation_failed() {
        assert_eq!(
            StandardError::ValidationFailed { errors: vec![] }.code(),
            "validation_error"
        );
    }

    #[test]
    fn http_status_validation_failed() {
        assert_eq!(
            StandardError::ValidationFailed { errors: vec![] }.http_status(),
            400
        );
    }

    #[test]
    fn serialize_validation_failed_with_errors() {
        let err = StandardError::ValidationFailed {
            errors: vec![ValidationError {
                path: "name".into(),
                constraint: "length".into(),
                message: "too short".into(),
            }],
        };
        let json = serde_json::to_string(&err).expect("serialize StandardError");
        assert!(
            json.contains("\"code\":\"validation_error\""),
            "expected code field in {json}"
        );
        assert!(
            json.contains("\"errors\":[{"),
            "expected errors array in {json}"
        );
        assert!(
            json.contains("\"path\":\"name\""),
            "expected path in {json}"
        );
        assert!(
            json.contains("\"constraint\":\"length\""),
            "expected constraint in {json}"
        );
        assert!(
            json.contains("\"message\":\"too short\""),
            "expected message in {json}"
        );
    }
}

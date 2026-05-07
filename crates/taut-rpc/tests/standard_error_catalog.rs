//! Pin the wire contract of every `StandardError` variant per the error-codes
//! reference doc.

use taut_rpc::{StandardError, TautError};

#[test]
fn standard_error_catalog_pins_codes() {
    let cases: &[(StandardError, &str, u16)] = &[
        (StandardError::Unauthenticated, "unauthenticated", 401),
        (
            StandardError::Forbidden { reason: "x".into() },
            "forbidden",
            403,
        ),
        (StandardError::NotFound, "not_found", 404),
        (
            StandardError::RateLimited {
                retry_after_seconds: 60,
            },
            "rate_limited",
            429,
        ),
        (StandardError::Internal, "internal", 500),
        (
            StandardError::BadRequest {
                message: "x".into(),
            },
            "bad_request",
            400,
        ),
        (
            StandardError::Conflict {
                message: "x".into(),
            },
            "conflict",
            409,
        ),
        (
            StandardError::UnprocessableEntity {
                message: "x".into(),
            },
            "unprocessable_entity",
            422,
        ),
        (
            StandardError::ServiceUnavailable {
                retry_after_seconds: 60,
            },
            "service_unavailable",
            503,
        ),
        (StandardError::Timeout, "timeout", 504),
    ];

    for (err, expected_code, expected_status) in cases {
        assert_eq!(err.code(), *expected_code, "code mismatch for {err:?}");
        assert_eq!(
            err.http_status(),
            *expected_status,
            "status mismatch for {err:?}"
        );
    }
}

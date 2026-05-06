//! Validation bridge for taut-rpc. See SPEC §7.
//!
//! Per SPEC §7, `#[derive(Validate)]` on input/output types records a per-field
//! schema description into the IR; codegen then emits a Valibot (or, opt-in, Zod)
//! schema on the TypeScript side. Generated clients validate inputs *before*
//! sending and outputs *after* receiving by default; both checks can be disabled
//! per-call.
//!
//! This module defines the public surface of that bridge:
//!
//! - The [`Validate`] trait that the derive (Phase 4) will implement.
//! - The [`ValidationError`] type returned by failed checks.
//! - The [`Constraint`] vocabulary recorded into the IR. SPEC §7 fixes the 0.1
//!   set as `min`, `max`, `length`, `pattern`, `email`, `url`, plus opaque
//!   `custom` predicates that require user-supplied schema fragments.
//! - A [`check`] sub-module of stand-alone validators that don't need a derive.
//!
//! # Status
//!
//! Day-0 stub for ROADMAP Phase 4. The trait, error type, constraint enum, and
//! free-standing checkers are stable; the `#[derive(Validate)]` proc-macro is
//! not yet implemented.

use serde::{Deserialize, Serialize};

/// User-facing validation trait.
///
/// `#[derive(Validate)]` (Phase 4) will implement this by walking the type's
/// fields and dispatching to the [`check`] helpers in this module. Hand-written
/// impls are also supported.
pub trait Validate {
    /// Validate `self`, returning every collected failure.
    ///
    /// Returns `Ok(())` if the value is valid. Otherwise returns a non-empty
    /// `Vec<ValidationError>`; implementations should accumulate all failures
    /// rather than short-circuiting on the first one.
    fn validate(&self) -> Result<(), Vec<ValidationError>>;
}

/// One reported validation failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{path}: {message}")]
pub struct ValidationError {
    /// Dotted path to the field (e.g. `"user.email"`). Empty string for
    /// root-level errors.
    pub path: String,
    /// Constraint name that was violated (matches [`Constraint`] variants below,
    /// in `snake_case`).
    pub constraint: String,
    /// Human-readable message.
    pub message: String,
}

impl ValidationError {
    /// Construct a `ValidationError`. All fields accept anything `Into<String>`.
    pub fn new(
        path: impl Into<String>,
        constraint: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            constraint: constraint.into(),
            message: message.into(),
        }
    }
}

/// Constraint vocabulary recorded into the IR. SPEC §7 lists the supported set
/// for 0.1.
///
/// The `serde` representation is the externally-tagged form
/// `{ "kind": "<snake_case>", "value": ... }` so that codegen on the
/// TypeScript side can pattern-match cleanly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Constraint {
    /// Numeric lower bound (inclusive).
    Min(f64),
    /// Numeric upper bound (inclusive).
    Max(f64),
    /// String length range: `length(min, max?)`.
    Length {
        min: Option<u32>,
        max: Option<u32>,
    },
    /// Regex pattern (uncompiled — codegen forwards to JS `RegExp` /
    /// Valibot `regex`).
    Pattern(String),
    /// RFC-5322-ish email check. The runtime check is deliberately permissive;
    /// the canonical validation happens in the codegen schema.
    Email,
    /// `http://` or `https://` URL check.
    Url,
    /// Opaque tag for user-supplied predicates; codegen requires a
    /// user-supplied schema fragment to honour the constraint.
    Custom(String),
}

/// Built-in validators that don't need a derive.
///
/// Each function returns `Ok(())` on success or a [`ValidationError`] whose
/// `constraint` field is the name of the failed check (matching the
/// `snake_case` [`Constraint`] tag). The `path` argument is forwarded
/// verbatim into the error so callers can plumb through dotted field paths.
pub mod check {
    use super::ValidationError;

    /// Numeric lower bound (inclusive). `value == min` passes.
    pub fn min(path: &str, value: f64, min: f64) -> Result<(), ValidationError> {
        if value < min {
            Err(ValidationError::new(
                path,
                "min",
                format!("must be >= {min}, got {value}"),
            ))
        } else {
            Ok(())
        }
    }

    /// Numeric upper bound (inclusive). `value == max` passes.
    pub fn max(path: &str, value: f64, max: f64) -> Result<(), ValidationError> {
        if value > max {
            Err(ValidationError::new(
                path,
                "max",
                format!("must be <= {max}, got {value}"),
            ))
        } else {
            Ok(())
        }
    }

    /// String length range. Bounds are character counts (Unicode scalar values),
    /// not bytes. Either bound may be omitted.
    pub fn length(
        path: &str,
        s: &str,
        min: Option<u32>,
        max: Option<u32>,
    ) -> Result<(), ValidationError> {
        let len = s.chars().count() as u64;
        if let Some(min) = min {
            if len < u64::from(min) {
                return Err(ValidationError::new(
                    path,
                    "length",
                    format!("length must be >= {min}, got {len}"),
                ));
            }
        }
        if let Some(max) = max {
            if len > u64::from(max) {
                return Err(ValidationError::new(
                    path,
                    "length",
                    format!("length must be <= {max}, got {len}"),
                ));
            }
        }
        Ok(())
    }

    /// Permissive email check: requires an `@`, with at least one character
    /// before it and a `.` somewhere after it (also with at least one character
    /// after the dot). The canonical validation is done by the generated
    /// TypeScript schema.
    pub fn email(path: &str, s: &str) -> Result<(), ValidationError> {
        let bad = || {
            ValidationError::new(
                path,
                "email",
                format!("must be a valid email, got {s:?}"),
            )
        };
        let at = s.find('@').ok_or_else(bad)?;
        if at == 0 {
            return Err(bad());
        }
        let after_at = &s[at + 1..];
        let dot = after_at.find('.').ok_or_else(bad)?;
        if dot == 0 || dot + 1 >= after_at.len() {
            return Err(bad());
        }
        Ok(())
    }

    /// URL check: must start with `http://` or `https://`.
    pub fn url(path: &str, s: &str) -> Result<(), ValidationError> {
        if s.starts_with("http://") || s.starts_with("https://") {
            Ok(())
        } else {
            Err(ValidationError::new(
                path,
                "url",
                format!("must be an http(s) URL, got {s:?}"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- check::min / check::max ---------------------------------------------

    #[test]
    fn check_min_below_fails() {
        let err = check::min("x", 0.5, 1.0).expect_err("0.5 < 1.0 should fail");
        assert_eq!(err.path, "x");
        assert_eq!(err.constraint, "min");
    }

    #[test]
    fn check_min_at_boundary_ok() {
        check::min("x", 1.0, 1.0).expect("value == min should pass");
    }

    #[test]
    fn check_min_above_ok() {
        check::min("x", 2.0, 1.0).expect("value > min should pass");
    }

    #[test]
    fn check_max_above_fails() {
        let err = check::max("x", 1.5, 1.0).expect_err("1.5 > 1.0 should fail");
        assert_eq!(err.path, "x");
        assert_eq!(err.constraint, "max");
    }

    #[test]
    fn check_max_at_boundary_ok() {
        check::max("x", 1.0, 1.0).expect("value == max should pass");
    }

    #[test]
    fn check_max_below_ok() {
        check::max("x", 0.5, 1.0).expect("value < max should pass");
    }

    // --- check::length -------------------------------------------------------

    #[test]
    fn check_length_max_only_ok() {
        check::length("name", "hi", None, Some(5)).expect("len 2 <= 5");
    }

    #[test]
    fn check_length_max_only_fails() {
        let err = check::length("name", "hello!", None, Some(5))
            .expect_err("len 6 > 5 should fail");
        assert_eq!(err.constraint, "length");
        assert_eq!(err.path, "name");
    }

    #[test]
    fn check_length_min_and_max_ok() {
        check::length("name", "hey", Some(2), Some(5)).expect("2 <= 3 <= 5");
    }

    #[test]
    fn check_length_below_min_fails() {
        let err = check::length("name", "x", Some(2), Some(5))
            .expect_err("len 1 < 2 should fail");
        assert_eq!(err.constraint, "length");
    }

    #[test]
    fn check_length_empty_with_min_fails() {
        let err = check::length("name", "", Some(1), None)
            .expect_err("empty string fails min(1)");
        assert_eq!(err.constraint, "length");
    }

    #[test]
    fn check_length_empty_no_min_ok() {
        check::length("name", "", None, Some(5)).expect("empty allowed when no min");
    }

    #[test]
    fn check_length_empty_no_bounds_ok() {
        check::length("name", "", None, None).expect("no bounds always passes");
    }

    #[test]
    fn check_length_counts_chars_not_bytes() {
        // "é" is 2 bytes UTF-8 but 1 char.
        check::length("name", "é", Some(1), Some(1)).expect("counts chars, not bytes");
    }

    // --- check::email --------------------------------------------------------

    #[test]
    fn check_email_accepts_simple() {
        check::email("e", "a@b.co").expect("a@b.co is valid");
    }

    #[test]
    fn check_email_rejects_no_dot() {
        check::email("e", "a@b").expect_err("a@b has no dot after @");
    }

    #[test]
    fn check_email_rejects_empty() {
        check::email("e", "").expect_err("empty string is not an email");
    }

    #[test]
    fn check_email_rejects_no_domain() {
        check::email("e", "a.b@").expect_err("a.b@ has nothing after @");
    }

    #[test]
    fn check_email_rejects_leading_at() {
        check::email("e", "@b.co").expect_err("nothing before @");
    }

    #[test]
    fn check_email_error_carries_constraint() {
        let err = check::email("user.email", "nope").expect_err("nope is not an email");
        assert_eq!(err.path, "user.email");
        assert_eq!(err.constraint, "email");
    }

    // --- check::url ----------------------------------------------------------

    #[test]
    fn check_url_accepts_https() {
        check::url("u", "https://x").expect("https://x is allowed");
    }

    #[test]
    fn check_url_accepts_http() {
        check::url("u", "http://x").expect("http://x is allowed");
    }

    #[test]
    fn check_url_rejects_ftp() {
        let err = check::url("u", "ftp://x").expect_err("ftp scheme not allowed");
        assert_eq!(err.constraint, "url");
        assert_eq!(err.path, "u");
    }

    #[test]
    fn check_url_rejects_empty() {
        check::url("u", "").expect_err("empty is not a URL");
    }

    // --- Constraint serde roundtrip -----------------------------------------

    fn roundtrip(c: &Constraint) -> Constraint {
        let json = serde_json::to_string(c).expect("serialize Constraint");
        serde_json::from_str(&json).expect("deserialize Constraint")
    }

    #[test]
    fn constraint_min_roundtrip() {
        let c = Constraint::Min(1.5);
        assert_eq!(roundtrip(&c), c);
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains("\"kind\":\"min\""), "got {json}");
    }

    #[test]
    fn constraint_max_roundtrip() {
        let c = Constraint::Max(10.0);
        assert_eq!(roundtrip(&c), c);
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains("\"kind\":\"max\""), "got {json}");
    }

    #[test]
    fn constraint_length_roundtrip() {
        let c = Constraint::Length {
            min: Some(1),
            max: Some(64),
        };
        assert_eq!(roundtrip(&c), c);
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains("\"kind\":\"length\""), "got {json}");
    }

    #[test]
    fn constraint_length_max_only_roundtrip() {
        let c = Constraint::Length {
            min: None,
            max: Some(64),
        };
        assert_eq!(roundtrip(&c), c);
    }

    #[test]
    fn constraint_pattern_roundtrip() {
        let c = Constraint::Pattern(r"^\d+$".to_string());
        assert_eq!(roundtrip(&c), c);
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains("\"kind\":\"pattern\""), "got {json}");
    }

    #[test]
    fn constraint_email_roundtrip() {
        let c = Constraint::Email;
        assert_eq!(roundtrip(&c), c);
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains("\"kind\":\"email\""), "got {json}");
    }

    #[test]
    fn constraint_url_roundtrip() {
        let c = Constraint::Url;
        assert_eq!(roundtrip(&c), c);
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains("\"kind\":\"url\""), "got {json}");
    }

    #[test]
    fn constraint_custom_roundtrip() {
        let c = Constraint::Custom("must_be_prime".to_string());
        assert_eq!(roundtrip(&c), c);
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains("\"kind\":\"custom\""), "got {json}");
    }

    // --- ValidationError surface --------------------------------------------

    #[test]
    fn validation_error_display_uses_path_and_message() {
        let err = ValidationError::new("user.email", "email", "bad");
        assert_eq!(format!("{err}"), "user.email: bad");
    }

    #[test]
    fn validation_error_serde_roundtrip() {
        let err = ValidationError::new("a.b", "min", "too small");
        let json = serde_json::to_string(&err).expect("serialize");
        let back: ValidationError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, err);
    }
}

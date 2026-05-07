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
//! - The [`Validate`] trait that the derive (Phase 4) implements.
//! - The [`ValidationError`] type returned by failed checks.
//! - The [`Constraint`] vocabulary recorded into the IR. SPEC §7 fixes the 0.1
//!   set as `min`, `max`, `length`, `pattern`, `email`, `url`, plus opaque
//!   `custom` predicates that require user-supplied schema fragments.
//! - A [`check`] sub-module of stand-alone validators that don't need a derive.
//! - The [`run`], [`collect`], and [`nested`] glue helpers that the
//!   `#[derive(Validate)]` macro lowers its generated code to. The macro emits
//!   one `validate::run(|errors| { ... })` per impl, with a chain of
//!   `validate::collect(errors, || check::xxx(...))` calls inside, plus
//!   `validate::nested(errors, "field", &self.field)` for nested types that
//!   themselves implement [`Validate`]. Keeping this glue in the runtime crate
//!   instead of inlining it into each derive output keeps macro emissions
//!   small, easier to read, and easier to evolve.
//!
//! # Status
//!
//! Phase 4 of the ROADMAP. The trait, error type, constraint enum, and
//! free-standing checkers are stable; the `#[derive(Validate)]` proc-macro
//! lowers to the helpers in this module.

use serde::{Deserialize, Serialize};

/// User-facing validation trait.
///
/// `#[derive(Validate)]` (Phase 4) implements this by walking the type's
/// fields and dispatching to the [`check`] helpers in this module via
/// [`run`] + [`collect`] + [`nested`]. Hand-written impls are also supported.
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
    Length { min: Option<u32>, max: Option<u32> },
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

    /// Truncate a user-supplied string for inclusion in error messages.
    ///
    /// Long inputs (think pasted blobs) would otherwise blow up the
    /// `message` field. Take the first 20 chars and append `…` if the input
    /// is longer. Counts Unicode scalar values, not bytes, so we never split
    /// a multibyte codepoint.
    fn truncate_for_message(s: &str) -> String {
        const LIMIT: usize = 20;
        let mut iter = s.chars();
        let head: String = iter.by_ref().take(LIMIT).collect();
        if iter.next().is_some() {
            format!("{head}…")
        } else {
            head
        }
    }

    /// Numeric lower bound (inclusive). `value == min` passes.
    ///
    /// Generic over any numeric primitive that lossslessly converts into `f64`
    /// so that the `#[derive(Validate)]` macro can emit
    /// `check::min("x", self.x, 1.0)` regardless of whether `self.x` is an
    /// `i32`, `u8`, `u64`, `f32`, ...
    ///
    /// On failure the error `message` is `"value {v} is less than minimum
    /// {min}"`, where `{v}` is the offending value formatted as `f64`. The
    /// field path is *not* embedded in the message: the `path` field already
    /// holds it, and UIs are expected to render `path: message`.
    pub fn min<T>(path: &str, value: T, min: f64) -> Result<(), ValidationError>
    where
        T: Into<f64> + Copy,
    {
        let v: f64 = value.into();
        if v < min {
            Err(ValidationError::new(
                path,
                "min",
                format!("value {v} is less than minimum {min}"),
            ))
        } else {
            Ok(())
        }
    }

    /// Numeric upper bound (inclusive). `value == max` passes.
    ///
    /// Generic over any numeric primitive that lossslessly converts into `f64`
    /// (see [`min`] for the rationale).
    ///
    /// On failure the error `message` is `"value {v} is greater than maximum
    /// {max}"`, formatted as `f64`. The field path is not embedded.
    pub fn max<T>(path: &str, value: T, max: f64) -> Result<(), ValidationError>
    where
        T: Into<f64> + Copy,
    {
        let v: f64 = value.into();
        if v > max {
            Err(ValidationError::new(
                path,
                "max",
                format!("value {v} is greater than maximum {max}"),
            ))
        } else {
            Ok(())
        }
    }

    /// String length range. Bounds are character counts (Unicode scalar values),
    /// not bytes. Either bound may be omitted.
    ///
    /// On failure the error `message` is `"length {n} is outside [{lo},
    /// {hi}]"`, with `{lo}` rendered as the integer minimum or `"no
    /// minimum"`, and `{hi}` rendered as the integer maximum or `"no
    /// maximum"`.
    pub fn length(
        path: &str,
        s: &str,
        min: Option<u32>,
        max: Option<u32>,
    ) -> Result<(), ValidationError> {
        let len = s.chars().count() as u64;
        let out_of_range = match (min, max) {
            (Some(lo), _) if len < u64::from(lo) => true,
            (_, Some(hi)) if len > u64::from(hi) => true,
            _ => false,
        };
        if !out_of_range {
            return Ok(());
        }
        let lo = min.map_or_else(|| "no minimum".to_string(), |n| n.to_string());
        let hi = max.map_or_else(|| "no maximum".to_string(), |n| n.to_string());
        Err(ValidationError::new(
            path,
            "length",
            format!("length {len} is outside [{lo}, {hi}]"),
        ))
    }

    /// Permissive email check: requires an `@`, with at least one character
    /// before it and a `.` somewhere after it (also with at least one character
    /// after the dot). The canonical validation is done by the generated
    /// TypeScript schema.
    ///
    /// On failure the error `message` is `"not a valid email: {value}"`,
    /// where `{value}` is the offending input truncated to 20 chars.
    pub fn email(path: &str, s: &str) -> Result<(), ValidationError> {
        let bad = || {
            ValidationError::new(
                path,
                "email",
                format!("not a valid email: {}", truncate_for_message(s)),
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
    ///
    /// On failure the error `message` is `"not a valid url: {value}"`, with
    /// the offending input truncated to 20 chars.
    pub fn url(path: &str, s: &str) -> Result<(), ValidationError> {
        if s.starts_with("http://") || s.starts_with("https://") {
            Ok(())
        } else {
            Err(ValidationError::new(
                path,
                "url",
                format!("not a valid url: {}", truncate_for_message(s)),
            ))
        }
    }

    /// Regex pattern check.
    ///
    /// Compiles `regex_src` against the `regex` crate and tests whether `s`
    /// matches anywhere in the input (i.e. uses `Regex::is_match`, not a
    /// fully-anchored match — anchor explicitly with `^...$` if required).
    ///
    /// On failure the error `message` is `"does not match pattern
    /// /{regex_src}/"`. If `regex_src` itself fails to compile, the failure
    /// is surfaced as a [`ValidationError`] with `constraint = "pattern"`
    /// and a message beginning with `"invalid regex pattern: "`. We
    /// deliberately do not panic: the regex source comes from a
    /// `#[validate(pattern = "...")]` attribute supplied by user code, and a
    /// panic at validation time would be a poor failure mode for an HTTP
    /// server.
    pub fn pattern(path: &str, s: &str, regex_src: &str) -> Result<(), ValidationError> {
        let re = match regex::Regex::new(regex_src) {
            Ok(re) => re,
            Err(e) => {
                return Err(ValidationError::new(
                    path,
                    "pattern",
                    format!("invalid regex pattern: {e}"),
                ));
            }
        };
        if re.is_match(s) {
            Ok(())
        } else {
            // The offending input is deliberately omitted: the regex itself
            // is the more informative bit, and quoting a long string risks
            // blowing past the 80-char target.
            Err(ValidationError::new(
                path,
                "pattern",
                format!("does not match pattern /{regex_src}/"),
            ))
        }
    }
}

/// Run a single check, pushing any error into `out` instead of bubbling it.
///
/// Used by `#[derive(Validate)]`-emitted code so that successive checks all run
/// and accumulate errors rather than short-circuiting on the first failure.
///
/// # Example (mirrors what the derive emits)
///
/// ```
/// use taut_rpc::validate::{self, check};
///
/// let mut errors = Vec::new();
/// validate::collect(&mut errors, || check::min("x", 0.5, 1.0));
/// validate::collect(&mut errors, || check::max("x", 0.5, 1.0));
/// // First check failed, second passed: one error collected.
/// assert_eq!(errors.len(), 1);
/// assert_eq!(errors[0].constraint, "min");
/// ```
pub fn collect<F>(out: &mut Vec<ValidationError>, f: F)
where
    F: FnOnce() -> Result<(), ValidationError>,
{
    if let Err(e) = f() {
        out.push(e);
    }
}

/// Run a closure that accumulates errors into a fresh `Vec`, folding it into a
/// `Result<(), Vec<ValidationError>>`.
///
/// This is the entry point the `#[derive(Validate)]` macro lowers each
/// generated `Validate::validate` body to. Compare:
///
/// ```ignore
/// // Generated impl:
/// fn validate(&self) -> Result<(), Vec<ValidationError>> {
///     taut_rpc::validate::run(|errors| {
///         taut_rpc::validate::collect(errors, || check::min("age", self.age, 0.0));
///         taut_rpc::validate::collect(errors, || check::max("age", self.age, 150.0));
///         taut_rpc::validate::nested(errors, "address", &self.address);
///     })
/// }
/// ```
///
/// Returns `Ok(())` if the closure pushed nothing, otherwise `Err(errors)`.
pub fn run<F>(checks: F) -> Result<(), Vec<ValidationError>>
where
    F: FnOnce(&mut Vec<ValidationError>),
{
    let mut errors = Vec::new();
    checks(&mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Run a nested type's [`Validate`] impl and re-prefix any errors so their
/// `path` is rooted at `path_prefix`.
///
/// If the inner error has an empty `path`, the outer path becomes
/// `path_prefix`; otherwise it becomes `<path_prefix>.<inner.path>`. This
/// matches the dotted-path convention used by the derive when walking nested
/// fields.
///
/// # Example
///
/// ```
/// use taut_rpc::validate::{self, Validate, ValidationError, check};
///
/// struct Address { city: String }
/// impl Validate for Address {
///     fn validate(&self) -> Result<(), Vec<ValidationError>> {
///         validate::run(|errors| {
///             validate::collect(errors, || check::length("city", &self.city, Some(1), None));
///         })
///     }
/// }
///
/// struct User { address: Address }
/// impl Validate for User {
///     fn validate(&self) -> Result<(), Vec<ValidationError>> {
///         validate::run(|errors| {
///             validate::nested(errors, "address", &self.address);
///         })
///     }
/// }
///
/// let u = User { address: Address { city: String::new() } };
/// let errs = u.validate().expect_err("city is empty");
/// assert_eq!(errs[0].path, "address.city");
/// ```
pub fn nested<V>(out: &mut Vec<ValidationError>, path_prefix: &str, value: &V)
where
    V: Validate + ?Sized,
{
    if let Err(inner) = value.validate() {
        for mut e in inner {
            e.path = if e.path.is_empty() {
                path_prefix.to_string()
            } else {
                format!("{path_prefix}.{}", e.path)
            };
            out.push(e);
        }
    }
}

// --- Blanket `Validate` impls for primitives and standard containers ---------
//
// Phase 4 (and Agent 6's `#[rpc]` macro) emit unconditional
// `<I as Validate>::validate(&input)` calls after deserialization. For input
// types like `u32`, `String`, or `(u32, String)`, that won't compile unless the
// types implement `Validate`. The impls below are trivial pass-throughs:
// primitives, `&'static str`, and the unit type `()` always validate; nested
// containers (`Option`, `Vec`, `Box`, `HashMap`, tuples) recurse so that
// user-defined types embedded inside them still get checked.
//
// Real per-field constraints live on user-defined types via
// `#[derive(Validate)]`.

/// No-op `Validate` impls for types whose values cannot themselves carry
/// constraints — primitives, `&'static str`, and `()`.
macro_rules! noop_validate {
    ($($t:ty),* $(,)?) => {
        $(
            impl Validate for $t {
                fn validate(&self) -> Result<(), Vec<ValidationError>> { Ok(()) }
            }
        )*
    };
}

noop_validate!(
    bool,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    f32,
    f64,
    char,
    String,
    &'static str,
    (),
);

impl<T: Validate> Validate for Option<T> {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        match self {
            Some(v) => v.validate(),
            None => Ok(()),
        }
    }
}

impl<T: Validate> Validate for Vec<T> {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();
        for (i, v) in self.iter().enumerate() {
            if let Err(mut errs) = v.validate() {
                for e in errs.iter_mut() {
                    e.path = if e.path.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("[{i}].{}", e.path)
                    };
                }
                errors.append(&mut errs);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

impl<T: Validate> Validate for Box<T> {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        (**self).validate()
    }
}

impl<K, V: Validate> Validate for std::collections::HashMap<K, V> {
    fn validate(&self) -> Result<(), Vec<ValidationError>> {
        let mut errors = Vec::new();
        for v in self.values() {
            if let Err(mut errs) = v.validate() {
                errors.append(&mut errs);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Tuple `Validate` impls (arity 1..=4). Each arm runs every element's
/// `validate()` and accumulates failures rather than short-circuiting.
macro_rules! tuple_validate {
    ($($name:ident),+) => {
        impl<$($name: Validate),+> Validate for ($($name,)+) {
            #[allow(non_snake_case)]
            fn validate(&self) -> Result<(), Vec<ValidationError>> {
                let ($($name,)+) = self;
                let mut errors = Vec::new();
                $(
                    if let Err(mut errs) = $name.validate() { errors.append(&mut errs); }
                )+
                if errors.is_empty() { Ok(()) } else { Err(errors) }
            }
        }
    };
}
tuple_validate!(A);
tuple_validate!(A, B);
tuple_validate!(A, B, C);
tuple_validate!(A, B, C, D);

#[cfg(test)]
mod tests {
    use super::*;

    // --- check::min / check::max ---------------------------------------------

    #[test]
    fn check_min_below_fails() {
        let err = check::min("x", 0.5_f64, 1.0).expect_err("0.5 < 1.0 should fail");
        assert_eq!(err.path, "x");
        assert_eq!(err.constraint, "min");
    }

    #[test]
    fn check_min_at_boundary_ok() {
        check::min("x", 1.0_f64, 1.0).expect("value == min should pass");
    }

    #[test]
    fn check_min_above_ok() {
        check::min("x", 2.0_f64, 1.0).expect("value > min should pass");
    }

    #[test]
    fn check_max_above_fails() {
        let err = check::max("x", 1.5_f64, 1.0).expect_err("1.5 > 1.0 should fail");
        assert_eq!(err.path, "x");
        assert_eq!(err.constraint, "max");
    }

    #[test]
    fn check_max_at_boundary_ok() {
        check::max("x", 1.0_f64, 1.0).expect("value == max should pass");
    }

    #[test]
    fn check_max_below_ok() {
        check::max("x", 0.5_f64, 1.0).expect("value < max should pass");
    }

    // --- broadened numeric inputs --------------------------------------------

    #[test]
    fn check_min_accepts_i32() {
        let v: i32 = -3;
        let err = check::min("age", v, 0.0).expect_err("-3 < 0 should fail");
        assert_eq!(err.constraint, "min");
        check::min("age", 0_i32, 0.0).expect("0 == 0 passes");
        check::min("age", 5_i32, 0.0).expect("5 > 0 passes");
    }

    #[test]
    fn check_min_accepts_u64() {
        // u64 doesn't impl Into<f64> — only u32 / u16 / u8 do losslessly.
        // Use u32 here as the "wide unsigned" representative; pick u8 for the
        // narrow path.
        let v: u32 = 0;
        let err = check::min("count", v, 1.0).expect_err("0 < 1 should fail");
        assert_eq!(err.constraint, "min");
        check::min("count", 1_u32, 1.0).expect("1 == 1 passes");
        check::min("count", 100_u8, 1.0).expect("u8 100 > 1 passes");
    }

    #[test]
    fn check_max_accepts_i32() {
        let v: i32 = 200;
        let err = check::max("age", v, 150.0).expect_err("200 > 150 should fail");
        assert_eq!(err.constraint, "max");
        check::max("age", 150_i32, 150.0).expect("150 == 150 passes");
        check::max("age", -3_i32, 150.0).expect("-3 < 150 passes");
    }

    #[test]
    fn check_max_accepts_u32() {
        let v: u32 = 1000;
        let err = check::max("count", v, 500.0).expect_err("1000 > 500 should fail");
        assert_eq!(err.constraint, "max");
        check::max("count", 0_u32, 500.0).expect("0 < 500 passes");
        check::max("count", 5_u8, 10.0).expect("u8 5 < 10 passes");
    }

    #[test]
    fn check_min_max_message_includes_value() {
        let err = check::min("x", -2_i32, 0.0).expect_err("-2 < 0");
        assert!(err.message.contains("-2"), "got {}", err.message);
        let err = check::max("x", 999_u32, 10.0).expect_err("999 > 10");
        assert!(err.message.contains("999"), "got {}", err.message);
    }

    // --- normalized message text --------------------------------------------
    //
    // SPEC §7 / Phase 4: every check::* message must be sentence-case (no
    // leading capital, no trailing period), include the offending value,
    // omit the field path, and stay under 80 chars. UIs render `path:
    // message` themselves, so duplicating the path inside `message` would
    // double up.

    /// Helper: shared invariants every `check::*` message must satisfy.
    fn assert_message_shape(message: &str) {
        assert!(message.len() <= 80, "message > 80 chars: {message:?}");
        assert!(
            !message.ends_with('.'),
            "message ends with a period: {message:?}"
        );
        let first = message.chars().next().expect("non-empty message");
        assert!(
            !first.is_uppercase(),
            "message starts with uppercase: {message:?}"
        );
    }

    #[test]
    fn check_min_message_exact_text() {
        let err = check::min("x", -2_i32, 0.0).expect_err("-2 < 0");
        assert_eq!(err.message, "value -2 is less than minimum 0");
        assert_message_shape(&err.message);
        // Path must NOT appear inside message — UIs prepend it themselves.
        assert!(
            !err.message.contains("x:"),
            "path leaked into message: {}",
            err.message
        );
    }

    #[test]
    fn check_max_message_exact_text() {
        let err = check::max("y", 5_i32, 1.0).expect_err("5 > 1");
        assert_eq!(err.message, "value 5 is greater than maximum 1");
        assert_message_shape(&err.message);
    }

    #[test]
    fn check_length_message_both_bounds() {
        let err =
            check::length("name", "hello!", Some(2), Some(5)).expect_err("len 6 outside [2, 5]");
        assert_eq!(err.message, "length 6 is outside [2, 5]");
        assert_message_shape(&err.message);
    }

    #[test]
    fn check_length_message_no_min() {
        let err = check::length("name", "hello!", None, Some(5)).expect_err("len 6 outside [-, 5]");
        assert_eq!(err.message, "length 6 is outside [no minimum, 5]");
        assert_message_shape(&err.message);
    }

    #[test]
    fn check_length_message_no_max() {
        let err = check::length("name", "", Some(1), None).expect_err("len 0 outside [1, -]");
        assert_eq!(err.message, "length 0 is outside [1, no maximum]");
        assert_message_shape(&err.message);
    }

    #[test]
    fn check_email_message_exact_text() {
        let err = check::email("e", "nope").expect_err("not an email");
        assert_eq!(err.message, "not a valid email: nope");
        assert_message_shape(&err.message);
    }

    #[test]
    fn check_email_message_truncates_long_input() {
        // 30-char input — must be truncated to 20 chars + '…'.
        let long = "x".repeat(30) + "@nope";
        let err = check::email("e", &long).expect_err("malformed");
        let head: String = "x".repeat(20);
        assert_eq!(err.message, format!("not a valid email: {head}…"));
        assert_message_shape(&err.message);
    }

    #[test]
    fn check_url_message_exact_text() {
        let err = check::url("u", "ftp://x").expect_err("ftp not allowed");
        assert_eq!(err.message, "not a valid url: ftp://x");
        assert_message_shape(&err.message);
    }

    #[test]
    fn check_url_message_truncates_long_input() {
        let long = "g".repeat(40);
        let err = check::url("u", &long).expect_err("not a url");
        let head: String = "g".repeat(20);
        assert_eq!(err.message, format!("not a valid url: {head}…"));
        assert_message_shape(&err.message);
    }

    #[test]
    fn check_pattern_message_exact_text() {
        let err = check::pattern("x", "abc", r"^\d+$").expect_err("no digits");
        assert_eq!(err.message, r"does not match pattern /^\d+$/");
        assert_message_shape(&err.message);
        // Path must not be embedded.
        assert!(!err.message.contains("x:"), "got {}", err.message);
    }

    // --- check::length -------------------------------------------------------

    #[test]
    fn check_length_max_only_ok() {
        check::length("name", "hi", None, Some(5)).expect("len 2 <= 5");
    }

    #[test]
    fn check_length_max_only_fails() {
        let err =
            check::length("name", "hello!", None, Some(5)).expect_err("len 6 > 5 should fail");
        assert_eq!(err.constraint, "length");
        assert_eq!(err.path, "name");
    }

    #[test]
    fn check_length_min_and_max_ok() {
        check::length("name", "hey", Some(2), Some(5)).expect("2 <= 3 <= 5");
    }

    #[test]
    fn check_length_below_min_fails() {
        let err = check::length("name", "x", Some(2), Some(5)).expect_err("len 1 < 2 should fail");
        assert_eq!(err.constraint, "length");
    }

    #[test]
    fn check_length_empty_with_min_fails() {
        let err = check::length("name", "", Some(1), None).expect_err("empty string fails min(1)");
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

    // --- check::pattern ------------------------------------------------------

    #[test]
    fn check_pattern_matches() {
        check::pattern("x", "abc123", r"\d+").expect("contains digits");
    }

    #[test]
    fn check_pattern_anchored_full_match() {
        check::pattern("x", "12345", r"^\d+$").expect("all digits");
    }

    #[test]
    fn check_pattern_does_not_match() {
        let err = check::pattern("x", "abc", r"^\d+$").expect_err("no digits");
        assert_eq!(err.constraint, "pattern");
        assert_eq!(err.path, "x");
        assert!(err.message.contains(r"^\d+$"), "got {}", err.message);
    }

    #[test]
    fn check_pattern_invalid_regex_returns_error_not_panic() {
        // Unbalanced bracket: regex compile fails.
        let err = check::pattern("x", "abc", r"[unclosed")
            .expect_err("invalid regex source must surface as ValidationError");
        assert_eq!(err.constraint, "pattern");
        assert_eq!(err.path, "x");
        assert!(
            err.message.starts_with("invalid regex pattern:"),
            "got {}",
            err.message
        );
    }

    #[test]
    fn check_pattern_empty_input_against_optional_pattern() {
        // The pattern `^$` matches the empty string only.
        check::pattern("x", "", r"^$").expect("empty matches ^$");
        check::pattern("x", "x", r"^$").expect_err("non-empty does not match ^$");
    }

    // --- collect -------------------------------------------------------------

    #[test]
    fn collect_pushes_on_err() {
        let mut errors = Vec::new();
        collect(&mut errors, || check::min("x", 0_i32, 1.0));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].constraint, "min");
        assert_eq!(errors[0].path, "x");
    }

    #[test]
    fn collect_skips_on_ok() {
        let mut errors = Vec::new();
        collect(&mut errors, || check::min("x", 5_i32, 1.0));
        assert!(errors.is_empty());
    }

    #[test]
    fn collect_accumulates_multiple_failures() {
        let mut errors = Vec::new();
        collect(&mut errors, || check::min("x", 0_i32, 1.0));
        collect(&mut errors, || check::max("x", 100_i32, 10.0));
        collect(&mut errors, || check::min("y", 5_i32, 1.0)); // ok
        collect(&mut errors, || check::email("e", "nope"));
        assert_eq!(errors.len(), 3);
        assert_eq!(errors[0].constraint, "min");
        assert_eq!(errors[1].constraint, "max");
        assert_eq!(errors[2].constraint, "email");
    }

    // --- run -----------------------------------------------------------------

    #[test]
    fn run_returns_ok_when_no_errors_pushed() {
        let result = run(|_errors| {});
        assert!(result.is_ok());
    }

    #[test]
    fn run_returns_ok_when_all_checks_pass() {
        let result = run(|errors| {
            collect(errors, || check::min("x", 5_i32, 1.0));
            collect(errors, || check::max("x", 5_i32, 10.0));
        });
        assert!(result.is_ok());
    }

    #[test]
    fn run_returns_err_with_all_collected_failures() {
        let result = run(|errors| {
            collect(errors, || check::min("x", 0_i32, 1.0));
            collect(errors, || check::max("y", 100_i32, 10.0));
        });
        let errs = result.expect_err("two failures should make Err");
        assert_eq!(errs.len(), 2);
        assert_eq!(errs[0].path, "x");
        assert_eq!(errs[0].constraint, "min");
        assert_eq!(errs[1].path, "y");
        assert_eq!(errs[1].constraint, "max");
    }

    #[test]
    fn run_does_not_short_circuit() {
        // Confirm every check inside `run`'s closure executes, even after an
        // earlier one fails.
        let mut counter = 0;
        let result = run(|errors| {
            counter += 1;
            collect(errors, || check::min("x", 0_i32, 1.0));
            counter += 1;
            collect(errors, || check::max("x", 100_i32, 10.0));
            counter += 1;
        });
        assert_eq!(counter, 3);
        assert_eq!(result.expect_err("two failures").len(), 2);
    }

    // --- nested --------------------------------------------------------------

    struct Inner {
        a: i32,
    }
    impl Validate for Inner {
        fn validate(&self) -> Result<(), Vec<ValidationError>> {
            run(|errors| {
                collect(errors, || check::min("a", self.a, 0.0));
            })
        }
    }

    struct Outer {
        inner: Inner,
    }
    impl Validate for Outer {
        fn validate(&self) -> Result<(), Vec<ValidationError>> {
            run(|errors| {
                nested(errors, "inner", &self.inner);
            })
        }
    }

    #[test]
    fn nested_prefixes_inner_path() {
        let outer = Outer {
            inner: Inner { a: -1 },
        };
        let errs = outer.validate().expect_err("a < 0 should fail");
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].path, "inner.a");
        assert_eq!(errs[0].constraint, "min");
    }

    #[test]
    fn nested_passes_through_when_inner_ok() {
        let outer = Outer {
            inner: Inner { a: 5 },
        };
        outer.validate().expect("inner is valid");
    }

    struct RootError;
    impl Validate for RootError {
        fn validate(&self) -> Result<(), Vec<ValidationError>> {
            Err(vec![ValidationError::new("", "custom", "root-level fail")])
        }
    }

    #[test]
    fn nested_uses_prefix_alone_when_inner_path_empty() {
        let mut errors = Vec::new();
        nested(&mut errors, "field", &RootError);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "field");
        assert_eq!(errors[0].constraint, "custom");
    }

    #[test]
    fn nested_collects_multiple_inner_errors() {
        struct Multi;
        impl Validate for Multi {
            fn validate(&self) -> Result<(), Vec<ValidationError>> {
                run(|errors| {
                    collect(errors, || check::min("a", 0_i32, 1.0));
                    collect(errors, || check::max("b", 100_i32, 10.0));
                })
            }
        }
        let mut errors = Vec::new();
        nested(&mut errors, "wrap", &Multi);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].path, "wrap.a");
        assert_eq!(errors[1].path, "wrap.b");
    }

    #[test]
    fn nested_pushes_nothing_when_inner_ok() {
        struct AlwaysOk;
        impl Validate for AlwaysOk {
            fn validate(&self) -> Result<(), Vec<ValidationError>> {
                Ok(())
            }
        }
        let mut errors = Vec::new();
        nested(&mut errors, "x", &AlwaysOk);
        assert!(errors.is_empty());
    }

    #[test]
    fn nested_double_nesting_dotted_path() {
        // Outer wraps Outer wraps Inner: path should become "a.b.a".
        struct Deeper {
            outer: Outer,
        }
        impl Validate for Deeper {
            fn validate(&self) -> Result<(), Vec<ValidationError>> {
                run(|errors| {
                    nested(errors, "outer", &self.outer);
                })
            }
        }
        let d = Deeper {
            outer: Outer {
                inner: Inner { a: -1 },
            },
        };
        let errs = d.validate().expect_err("inner a < 0");
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].path, "outer.inner.a");
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

    // --- Blanket Validate impls (primitives / Option / Vec / tuples) --------

    #[test]
    fn validate_for_unit_returns_ok() {
        ().validate().expect("unit always validates");
    }

    #[test]
    fn validate_for_primitives_all_return_ok() {
        true.validate().expect("bool always ok");
        42_u32.validate().expect("u32 always ok");
        "hello".to_string().validate().expect("String always ok");
    }

    #[test]
    fn validate_for_option_t_calls_inner_when_some() {
        // Inner type that fails validation unless its value is >= 0.
        struct Field(i32);
        impl Validate for Field {
            fn validate(&self) -> Result<(), Vec<ValidationError>> {
                run(|errors| {
                    collect(errors, || check::min("v", self.0, 0.0));
                })
            }
        }

        // None: skips inner — passes.
        let none: Option<Field> = None;
        none.validate().expect("None passes");

        // Some(ok): inner ok, outer ok.
        Some(Field(5))
            .validate()
            .expect("Some with valid inner passes");

        // Some(bad): inner fails, outer surfaces the failure.
        let errs = Some(Field(-1))
            .validate()
            .expect_err("Some with -1 should fail inner check");
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].constraint, "min");
        assert_eq!(errs[0].path, "v");
    }

    #[test]
    fn validate_for_vec_indexes_path() {
        // Inner type that fails when value < 0, with path "v".
        struct Field(i32);
        impl Validate for Field {
            fn validate(&self) -> Result<(), Vec<ValidationError>> {
                run(|errors| {
                    collect(errors, || check::min("v", self.0, 0.0));
                })
            }
        }

        let v = vec![Field(5), Field(-1), Field(10), Field(-2)];
        let errs = v.validate().expect_err("indices 1 and 3 should fail");
        assert_eq!(errs.len(), 2);
        assert_eq!(errs[0].path, "[1].v");
        assert_eq!(errs[0].constraint, "min");
        assert_eq!(errs[1].path, "[3].v");
        assert_eq!(errs[1].constraint, "min");

        // Empty Vec passes.
        let empty: Vec<Field> = Vec::new();
        empty.validate().expect("empty Vec passes");

        // All-ok Vec passes.
        let ok = vec![Field(0), Field(1), Field(2)];
        ok.validate().expect("all-valid Vec passes");

        // Inner with empty path: index alone, no trailing dot.
        struct RootFail;
        impl Validate for RootFail {
            fn validate(&self) -> Result<(), Vec<ValidationError>> {
                Err(vec![ValidationError::new("", "custom", "boom")])
            }
        }
        let v = vec![RootFail, RootFail];
        let errs = v.validate().expect_err("both fail at root");
        assert_eq!(errs.len(), 2);
        assert_eq!(errs[0].path, "[0]");
        assert_eq!(errs[1].path, "[1]");
    }

    #[test]
    fn validate_for_tuple_runs_all_arms() {
        // Inner that fails iff its value is negative, recording path "v".
        struct Field(i32);
        impl Validate for Field {
            fn validate(&self) -> Result<(), Vec<ValidationError>> {
                run(|errors| {
                    collect(errors, || check::min("v", self.0, 0.0));
                })
            }
        }

        // (A,) — single arm.
        let one = (Field(-1),);
        let errs = one.validate().expect_err("single-arm tuple fails");
        assert_eq!(errs.len(), 1);

        // (A, B) — both arms run, both fail.
        let two = (Field(-1), Field(-2));
        let errs = two.validate().expect_err("both arms fail");
        assert_eq!(errs.len(), 2, "tuple must not short-circuit");

        // (A, B, C) — mixed.
        let three = (Field(-1), Field(0), Field(-3));
        let errs = three.validate().expect_err("two of three fail");
        assert_eq!(errs.len(), 2);

        // (A, B, C, D) — all ok.
        let four = (Field(0), Field(1), Field(2), Field(3));
        four.validate().expect("all-valid 4-tuple passes");

        // Mixed primitives + user types compile and run.
        let mixed: (u32, String, Field) = (1, "x".into(), Field(5));
        mixed.validate().expect("primitives + ok user type pass");
    }
}

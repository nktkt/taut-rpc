# Constraint reference

This is a quick mapping of every `#[taut(...)]` constraint supported in v0.1
to (a) the runtime check it lowers to in Rust, (b) the Valibot expression
emitted by `cargo taut gen --validator valibot`, and (c) the Zod expression
emitted by `cargo taut gen --validator zod`.

For when each constraint applies, see [the validation concept](../concepts/validation.md).

## Numeric

| Attribute | Rust | Valibot | Zod |
|---|---|---|---|
| `#[taut(min = 0)]` | `check::min(path, v, 0.0)` | `v.minValue(0)` | `.min(0)` |
| `#[taut(max = 100)]` | `check::max(path, v, 100.0)` | `v.maxValue(100)` | `.max(100)` |
| `#[taut(min = 0, max = 100)]` | both | both | both |

## String length

| Attribute | Rust | Valibot | Zod |
|---|---|---|---|
| `#[taut(length(min = 3))]` | `check::length(path, s, Some(3), None)` | `v.minLength(3)` | `.min(3)` |
| `#[taut(length(max = 32))]` | `check::length(path, s, None, Some(32))` | `v.maxLength(32)` | `.max(32)` |
| `#[taut(length(min = 3, max = 32))]` | both | both | both |

## Format

| Attribute | Rust | Valibot | Zod |
|---|---|---|---|
| `#[taut(email)]` | `check::email(path, s)` | `v.email()` | `.email()` |
| `#[taut(url)]` | `check::url(path, s)` | `v.url()` | `.url()` |
| `#[taut(pattern = "regex")]` | `check::pattern(path, s, "regex")` | `v.regex(/regex/)` | `.regex(/regex/)` |

## Custom

| Attribute | Rust | Valibot | Zod |
|---|---|---|---|
| `#[taut(custom = "name")]` | no-op (user supplies) | `/* custom:name */` comment | `/* custom:name */` |

## Notes

- All constraints are field-level. Type-level constraints (e.g. "this struct
  is valid only if A = B") are not supported in v0.1.
- For numeric constraints (`min`, `max`), the value is coerced to `f64` for
  comparison. Integer overflow is not a concern at validation time because
  the value has already been parsed.
- For `length`, the count is **characters** for strings (Rust `str::chars()`
  / TS `Array.from(str).length`), not bytes. For Vec, the count is `len()` /
  `Array.length`.
- The `email` and `url` checks are deliberately weak (substring of `@` and
  `.` for email; `http(s)://` prefix for url). Use `pattern` or `custom` for
  RFC-strict validation.
- Patterns are compiled by the `regex` crate (Rust) and the JS regex engine
  (TS). The set of supported features differs slightly — RE2 (regex crate)
  doesn't support backreferences or lookahead. If a pattern is rejected, it
  surfaces as a runtime ValidationError with a "pattern" constraint.

## Adding new constraints

The constraint vocabulary is closed in v0.1. Adding new built-ins requires:

1. A new variant on `taut_rpc::Constraint` (an IR shape change → IR_VERSION bump).
2. A `check::<name>` helper in `taut_rpc::validate::check`.
3. Macro support in `#[derive(Validate)]` and `#[derive(Type)]`.
4. Codegen wiring in both Valibot and Zod renderers.
5. Documentation in this page and the concepts/validation.md.

Until the vocabulary stabilises, prefer `#[taut(custom = "...")]` and supply your
own predicate.

# Type mapping

> Placeholder chapter. The canonical, exhaustive table lives in
> [SPEC §3.1](../reference/spec.md). What follows is an excerpt; do not
> treat it as authoritative. **TODO:** keep this file as prose and link
> out — the table belongs in one place.

## A handful of representative rows

| Rust | TypeScript | Notes |
|---|---|---|
| `bool` | `boolean` | |
| `u8`–`u32`, `i8`–`i32`, `f32`, `f64` | `number` | |
| `u64`, `i64`, `u128`, `i128` | `bigint` | configurable: `as_string` mode emits `string` |
| `String`, `&str`, `Cow<str>` | `string` | |
| `Option<T>` | `T \| null` | configurable: `T \| undefined` per-field |
| `Vec<T>`, `&[T]`, `[T; N]` | `T[]` / tuple for fixed-size | |

The full table covers maps, time types, UUIDs, and the discriminated-union
encoding of Rust enums. See SPEC §3.1 and §3.2.

## Things worth flagging up front

**64-bit integers are `bigint` by default.** JavaScript's `number` cannot
represent the full range of `u64` / `i64`. The default mapping is
correctness-first: `bigint`. An `as_string` per-field override exists for
APIs that have to interoperate with JSON consumers that don't speak
`bigint` literals.

**`Option<T>` is `T | null`, not `T | undefined`.** This matches `serde`'s
default JSON shape (an absent field deserializes to `None` only when the
field is `#[serde(default)]`; otherwise the JSON `null` is what `Option`
emits). A per-field `#[taut(undefined)]` override is provided for
ergonomics on the TypeScript side.

**Fixed-size arrays become tuples.** `[u8; 4]` is `[number, number,
number, number]`, not `number[]`. Length is preserved in the type system.

**Time and UUID mappings are feature-gated.** `chrono::DateTime`,
`time::OffsetDateTime`, and `uuid::Uuid` are recognized only when the
relevant feature is enabled. UUIDs are branded as `Uuid` on the TS side
to keep them distinguishable from arbitrary strings.

## User-defined types

Structs become TypeScript interfaces (or type aliases for tuple structs).
Enums become discriminated unions; the default discriminant key is
`"type"`, configurable via `#[taut(tag = "...")]`. The `tag = "type"`
default has a known collision risk with user fields literally named
`type` — see SPEC §8 (open questions) for the planned escape hatch.

Generics in 0.1 are restricted to lifetime-erased monomorphic forms; the
macro records only types that are reachable from a `#[rpc]` function. This
is intentional scope: generic procedures are deferred to post-0.1.

## See also

- [SPEC §3.1 — Primitive type mapping](../reference/spec.md)
- [SPEC §3.2 — User-defined types](../reference/spec.md)
- [Errors](./errors.md)

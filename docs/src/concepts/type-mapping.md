# Type mapping

This page is the working reference for how Rust types cross the wire into
TypeScript in **v0.1**. It mirrors [SPEC §3](../reference/spec.md), expanded
with examples and the rules that govern derives, attributes, and serde
interop.

If something here disagrees with the SPEC, the SPEC wins — file an issue.

## 1. Primitive types

Every supported primitive lands as one of three TS shapes: `boolean`,
`number`, `string`, or `bigint` (plus `void` for unit). The full table:

| Rust | TypeScript | Notes |
|---|---|---|
| `bool` | `boolean` | |
| `u8`, `u16`, `u32` | `number` | safe — fits in IEEE 754 mantissa. |
| `i8`, `i16`, `i32` | `number` | safe. |
| `f32`, `f64` | `number` | NaN / Infinity follow JSON's usual caveats. |
| `u64`, `i64`, `u128`, `i128` | `bigint` | configurable: `as_string` mode emits `string`. See §5. |
| `String`, `&str`, `Cow<str>` | `string` | UTF-8 in, UTF-8 out. |
| `char` | `string` | always a single-grapheme string at runtime. |
| `()` | `void` | only valid as a return type. |
| `Result<T, E>` (empty `E`) | `T` (no error union) | per-procedure narrowing — see [errors](./errors.md). |

```rust
#[rpc]
async fn ping() -> &'static str { "pong" }       // -> () => Promise<string>

#[rpc]
async fn add(input: AddInput) -> i64 { /* */ }   // i64 -> bigint
```

## 2. Composite types

Composites are pure structural translations — no nominal wrapper appears in
the generated `.ts`.

| Rust | TypeScript |
|---|---|
| `Option<T>` | `T \| null` (default) — see §4 for `T \| undefined`. |
| `Vec<T>`, `&[T]` | `T[]` |
| `[T; N]` (fixed-size) | `[T, T, ..., T]` — tuple of length `N`. |
| `(A, B)`, `(A, B, C)`, `(A, B, C, D)` | `[A, B]`, `[A, B, C]`, `[A, B, C, D]` |
| `HashMap<String, V>` | `Record<string, V>` |
| `HashMap<K, V>` (non-string `K`) | `Array<[K, V]>` |
| `Box<T>` | same as `T` (transparent) |

```rust
struct Page {
    items: Vec<Item>,         // Item[]
    next:  Option<String>,    // string | null
    rgb:   [u8; 3],           // [number, number, number]
    meta:  HashMap<String, String>, // Record<string, string>
}
```

Fixed-size arrays preserve their length in the type system: passing a
4-element array where a 3-tuple is expected fails to type-check on the TS
side, not at runtime.

## 3. User-defined types

`#[derive(Type)]` is the single entry point.

### 3.1 Structs

Named structs map to TypeScript **interfaces**:

```rust
#[derive(Type, Serialize, Deserialize)]
struct User {
    id: u32,
    name: String,
    email: Option<String>,
}
```

```ts
export interface User {
  id: number;
  name: string;
  email: string | null;
}
```

Tuple structs map to **tuple type aliases**:

```rust
#[derive(Type, Serialize, Deserialize)]
struct Coord(f64, f64);
```

```ts
export type Coord = [number, number];
```

Newtype structs (single-field tuple structs) collapse to the inner type by
default, matching serde's transparent encoding:

```rust
#[derive(Type, Serialize, Deserialize)]
struct UserId(u32);
// -> export type UserId = number;
```

Unit structs map to the empty tuple type:

```rust
#[derive(Type, Serialize, Deserialize)]
struct Marker;
// -> export type Marker = [];
```

### 3.2 Enums (discriminated unions)

Enums become **internally tagged** discriminated unions on the TS side. The
default tag key is `"type"` and is configurable via `#[taut(tag = "...")]`:

```rust
#[derive(Type, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Joined { user_id: u32 },
    Left,
    Message(String),
}
```

```ts
export type Event =
  | { type: "joined"; user_id: number }
  | { type: "left" }
  | { type: "message"; value: string };
```

The `tag = "type"` default has a known collision risk with user fields
literally named `type` — see SPEC §8 for the escape-hatch tracking issue.

## 4. The `#[taut(...)]` attributes

Field- and container-level attributes from the macro shape both the IR and
the emitted `.ts`.

| Attribute | Where | Effect |
|---|---|---|
| `#[taut(rename = "...")]` | field or variant | Renames the field/variant on the wire and in the generated TS. |
| `#[taut(tag = "...")]` | enum container | Sets the discriminant key (default `"type"`). |
| `#[taut(optional)]` | field | Marks the field optional in the TS interface (`field?: T`). The Rust type does **not** have to be `Option<T>`. |
| `#[taut(undefined)]` | field on `Option<T>` | Emits `T \| undefined` instead of `T \| null`. |

```rust
#[derive(Type, Serialize, Deserialize)]
struct Search {
    #[taut(rename = "q")]
    query: String,

    #[taut(optional)]
    limit: Option<u32>,            // limit?: number | null

    #[taut(undefined)]
    cursor: Option<String>,        // cursor: string | undefined
}
```

```ts
export interface Search {
  q: string;
  limit?: number | null;
  cursor: string | undefined;
}
```

## 5. Bigint strategy

64- and 128-bit integers cannot be represented losslessly by JavaScript's
`number`. v0.1 ships two strategies:

- **Native** (default): emit `bigint`. The generated client uses `JSON.parse`
  reviver hooks so values round-trip as `bigint`.
- **AsString**: emit `string`. Values are encoded as decimal strings on the
  wire; consumers parse them with `BigInt(...)` themselves.

Selection is per-field via the macro and globally via `cargo taut gen
--bigint <native|as-string>`:

```rust
#[derive(Type, Serialize, Deserialize)]
struct Account {
    id: u64,                                // bigint
    #[taut(bigint = "as_string")]
    big_counter: u128,                      // string
}
```

Choose AsString when consumers can't write `bigint` literals (older
runtimes, JSON consumers in other languages bridged through the same TS
types). Choose Native when staying within a modern TS ecosystem — the
type-level guarantee is much stronger.

## 6. Special cases (feature-gated)

These mappings are recognized only when the relevant cargo feature is
enabled on the `taut-rpc` crate:

| Rust | TS | Feature | Notes |
|---|---|---|---|
| `chrono::DateTime<Utc>` | `string` | `chrono` | ISO-8601, with `Z` suffix on the wire. |
| `time::OffsetDateTime` | `string` | `time` | ISO-8601. |
| `uuid::Uuid` | `string` (branded `Uuid`) | `uuid` | Compile-time distinguishable from arbitrary strings. |

```rust
#[cfg(feature = "uuid")]
#[derive(Type, Serialize, Deserialize)]
struct Session {
    id: uuid::Uuid,                         // Uuid (branded string)
    started_at: chrono::DateTime<chrono::Utc>, // string
}
```

The `Uuid` brand is a TS technique (`string & { readonly __brand: "Uuid" }`)
— it costs nothing at runtime, but `function f(u: Uuid)` rejects a raw
`string` argument.

## 7. Serde compatibility

taut-rpc's macro layer is built to coexist with serde, not replace it.
Where a serde attribute changes the **wire shape**, taut-rpc respects it
when generating TS.

Respected serde attributes (v0.1):

- `#[serde(rename = "...")]` — same effect as `#[taut(rename)]`.
- `#[serde(rename_all = "...")]` on enums and structs — taut-rpc applies the
  same casing transform to the emitted TS, so a `rename_all = "snake_case"`
  enum lines up with the wire.
- `#[serde(tag = "...")]` — when present on an enum **and** a `#[taut(tag)]`
  is *not* set, taut-rpc honors serde's tag for the TS discriminant.
- `#[serde(transparent)]` — recognized on newtype structs.
- `#[serde(skip)]` — fields are omitted from the IR and from emitted TS.

The biggest one is **`rename_all` on a tagged enum** — keeping that single
attribute in sync with `#[taut(tag)]` is enough to make the TS variant tags
match the JSON tags:

```rust
#[derive(Type, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[taut(tag = "kind")]
enum Status {
    InProgress,
    Done,
    NeedsReview,
}
// -> kind: "in-progress" | "done" | "needs-review"
```

Not respected in v0.1: `#[serde(default)]`, `#[serde(flatten)]`,
`#[serde(with = "...")]`, custom (de)serializers. Most of these change
runtime behavior without a corresponding change in the static type, so
they're tracked as future work rather than silently miscompiled.

## 8. Limitations (v0.1)

- **No generics.** `#[derive(Type)]` does not support generic structs or
  enums. The IR is a closed-world snapshot reachable from the `#[rpc]`
  surface; only **lifetime-erased monomorphic forms** are recorded.
  Workaround: write per-instantiation wrapper types. Generic procedures
  are explicitly deferred (SPEC §8).
- **No lifetimes in the IR.** A `&'a str` argument lands in the IR exactly
  the same as `String`. Borrowed-vs-owned distinctions are erased.
- **No trait objects.** `Box<dyn SomeTrait>` is not recognized; the macro
  emits a clear error.
- **No `#[serde(flatten)]` or untagged enums.** The discriminated-union
  encoding is the only enum encoding v0.1 supports on the TS side.

## 9. Extending the built-in mapping

To teach taut-rpc about a new built-in type — say a third-party numeric
type that should map to `bigint` — add a `TautType` impl in
`taut_rpc::types`:

```rust
// crates/taut-rpc/src/types.rs
impl TautType for my_crate::BigDecimal {
    fn ir_type_ref() -> IrTypeRef { IrTypeRef::Primitive("string") }
    fn ir_type_def() -> Option<IrTypeDef> { None }
    fn collect_type_defs(_out: &mut Vec<IrTypeDef>) {}
}
```

The three methods are:

- `ir_type_ref` — how the type is **referenced** from another type.
- `ir_type_def` — the type's **definition** when it's a user-defined struct
  or enum (returns `None` for primitives).
- `collect_type_defs` — walks transitive `TypeDef`s reachable through
  generic parameters (e.g. `Vec<T>` calls into `T`'s `collect_type_defs`).

For anything beyond a built-in shim — new constraint vocabulary, new
codegen pathways, new transports — see
[CONTRIBUTING.md](https://github.com/.../CONTRIBUTING.md) for the macro,
IR, and codegen layout.

## See also

- [SPEC §3](../reference/spec.md) — the canonical type-mapping table.
- [Errors](./errors.md) — how `Result<T, E>` becomes a per-procedure
  narrowed error union.
- [Validation](./validation.md) — `#[taut(...)]` constraint attributes
  layered on top of the type mapping.
- [IR](./ir.md) — the on-disk JSON the macros and codegen exchange.

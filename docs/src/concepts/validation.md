# Validation

Phase 4 ships taut-rpc's *validation bridge*: a single Rust-side declaration of
input constraints that becomes both server-side enforcement and a
codegen-emitted TypeScript schema. This chapter is the user-facing reference
for what the bridge does, what attributes it understands, and how its two
sides (Rust enforcement, TS pre-flight) interact.

For the normative one-paragraph summary see
[SPEC §7](../reference/spec.md#7-validation-bridge); for the runnable demo
see [`examples/phase4-validate/`](https://github.com/anthropics/taut-rpc/tree/main/examples/phase4-validate).

## What the validation bridge is

The validation bridge is **server-authoritative input validation that the
codegen step also emits as a Valibot/Zod schema for client-side pre-flight
checks**. Rust is the source of truth: a constraint declared on a Rust input
type is *both* enforced by the server at request time *and* mirrored into the
generated `api.gen.ts` so the client can reject bad input before it leaves the
browser.

The contract is a one-way shape arrow:

> A constraint added on the Rust side fails the TS build (when downstream
> callers can't satisfy it) or, at minimum, fails fast at runtime before the
> network call.

In other words: the server is the only thing that *must* validate. The client
schema exists to surface mistakes earlier (in the editor, at unit-test time,
or at the call site) — not to relax server checks. If you remove client
validation, the server still rejects bad payloads with HTTP 400 and the
canonical error envelope.

## The `Validate` trait and `#[derive(Validate)]` macro

### Trait signature

```rust
pub trait Validate {
    fn validate(&self) -> Result<(), ValidationErrors>;
}

pub struct ValidationErrors {
    pub errors: Vec<ValidationError>,
}

pub struct ValidationError {
    pub path: String,        // e.g. "username" or "items[3].name"
    pub constraint: String,  // e.g. "length", "email", "min"
    pub message: String,     // human-readable
}
```

A blanket `impl Validate for T where T: !HasConstraints` provides a no-op
default, so types without `#[taut(...)]` attributes pass trivially.

### What the derive emits

`#[derive(Validate)]` walks each field, looks for `#[taut(...)]` attributes,
and emits a per-field check. Fields without attributes are skipped (their
inner type's `Validate` impl is *not* recursively invoked in 0.1 — flat
checks only). Multiple attributes on one field are AND-composed in source
order.

The derive also writes a `Constraint` entry per field into the IR
(`target/taut/ir.json`), keyed by the input type's canonical path. This is
what `cargo taut gen` reads later.

### Canonical example

```rust
#[derive(serde::Serialize, serde::Deserialize, taut_rpc::Type, taut_rpc::Validate)]
pub struct CreateUser {
    #[taut(length(min = 3, max = 32))]
    username: String,
    #[taut(email)]
    email: String,
    #[taut(min = 18, max = 120)]
    age: u8,
}
```

This single declaration produces:

- `impl Validate for CreateUser` on the Rust side (called by the procedure
  handler before your function body runs).
- A `CreateUserSchema` export in `api.gen.ts` (Valibot or Zod, depending on
  the `--validator` flag).
- An IR entry recording the three constraints, so any future code generator
  (other languages, other frameworks) can reproduce the same checks.

## Constraint vocabulary

The following attributes are recognised in 0.1. They mirror one-to-one with
the `Constraint` enum in the IR; codegen lowers each to the equivalent
Valibot/Zod combinator.

| Attribute | Applies to | Lowering |
|---|---|---|
| `#[taut(min = N)]` | numeric | `value >= N` |
| `#[taut(max = N)]` | numeric | `value <= N` |
| `#[taut(length(min = a, max = b))]` | `String` / `&str` / `Vec<T>` | char count or `len()` |
| `#[taut(pattern = "regex")]` | `String` | regex match |
| `#[taut(email)]` | `String` | basic syntactic check |
| `#[taut(url)]` | `String` | http/https prefix |
| `#[taut(custom = "name")]` | any | opaque — user supplies validator/codegen-time predicate |

A few notes on the lowerings:

- `length` on `String` measures **Unicode scalar values** (`char` count), not
  bytes. The TS side uses `.length` on the string's code-unit representation
  in 0.1 — close enough for ASCII and most BMP text; surrogate pairs and
  combining marks may differ at the margins. The quirks section calls this
  out for the SPEC backlog.
- `length` on `Vec<T>` and `&[T]` measures element count.
- `min`/`max` on numerics use `<=`/`>=` (inclusive bounds).
- `pattern` is anchored or unanchored exactly as the user wrote it; we don't
  silently insert `^…$`.

## Server-side enforcement

The `#[rpc]` macro wires `Validate` into the request lifecycle. After axum
deserializes the JSON body into the input type, the generated handler calls:

```rust
Validate::validate(&input)?;
```

before invoking the user's function body. On failure, the generated handler
short-circuits with the canonical envelope:

```json
{
  "err": {
    "code": "validation_error",
    "payload": { "errors": [ /* ValidationError[] */ ] }
  }
}
```

…with HTTP status **400**. Each error has the three fields shown earlier:
`path`, `constraint`, `message`.

### Subscriptions

Streaming procedures marked `#[rpc(stream)]` validate input the **same way**
as queries and mutations. On failure, the SSE stream emits a single
`event: error` frame carrying the validation envelope, followed immediately
by `event: end`. The client's `AsyncIterable<T>` throws on the next iteration
attempt with the same `TautError("validation_error", …)` shape it would have
thrown synchronously for a non-streaming call.

WebSocket transport (when `ws` is enabled) frames the failure as a single
`{ type: "error", payload }` message followed by `{ type: "end" }`.

## Client-side enforcement (codegen-driven)

### `--validator valibot` (default)

Run `cargo taut gen --validator valibot` (or just `cargo taut gen`, since
Valibot is the default). The generated `api.gen.ts` will then export:

- `<Type>Schema` for every input/output type that has at least one
  constraint or that contains a field whose type has constraints.
- `procedureSchemas`, a record keyed by procedure name with `{ input, output }`
  schema pairs.

Wire the schemas into the runtime by passing `procedureSchemas` to
`createApi` (or `createClient`):

```ts
import { createApi } from "taut-rpc/client";
import { procedureSchemas, type Procedures } from "./api.gen";

const api = createApi<Procedures>({
    url: "/rpc",
    schemas: procedureSchemas,
});
```

With schemas attached, the runtime calls `schema.parse(input)` **before**
sending the request and `schema.parse(output)` **after** receiving the
response. The first failure on either side throws a
`TautError("validation_error", { errors: ValidationError[] })`.

#### Per-call opt-out

Hot paths can suppress one or both sides:

```ts
await api.createUser.mutate(input, {
    validate: { send: false, recv: true },
});
```

You can also set `validate: { send: false, recv: false }` globally on
`createApi(...)` / `createClient(...)`, then re-enable per call. The default
is `{ send: true, recv: true }`.

#### Mismatch fail-fast

A failing input throws **before** the network call. The thrown
`TautError("validation_error", ...)` carries the same `errors` array shape
the server would have returned, so you can render one UI for both
client-side and server-side failures.

A failing output throws **after** the response lands but before it's handed
to your `await`/`for await` consumer — same envelope, same shape.

## `--validator zod`

```bash
cargo taut gen --validator zod
```

Same flow as Valibot: `<Type>Schema` exports, `procedureSchemas` record, same
`createApi({ schemas })` wiring, same per-call opt-out, same throw semantics.
The only difference is the emitted import (`zod` instead of `valibot`) and
the combinator names. Pick whichever your project already depends on; there
is no functional difference at runtime.

## `--validator none`

```bash
cargo taut gen --validator none
```

Codegen emits **no** schemas and no `procedureSchemas` record. The TS
runtime, with no schemas attached, performs no client-side validation.

Server-side validation is **unchanged** — the Rust `Validate` impl still
runs and still rejects bad payloads with HTTP 400. The only thing you lose
is the pre-flight check.

This mode is useful in the rare case where the client SDK consumer is
hand-rolled (e.g. a non-TypeScript caller, or a TS codebase that is already
running its inputs through an unrelated schema layer).

## Custom predicates

```rust
#[taut(custom = "checkNoBadwords")]
display_name: String,
```

`#[taut(custom = "name")]` records the tag in the IR but **emits no runtime
check**. The Rust derive does not generate any predicate call — you are
expected to implement the check by hand inside your procedure body, or to
wrap the type in a newtype whose `Validate` impl does the work.

On the codegen side, the generator leaves a comment:

```ts
display_name: v.string(), /* custom:checkNoBadwords */
```

…so users can grep for `custom:` and wire their own validator chain
externally (e.g. a `.pipe(...)` step that runs the named predicate). The
codegen does not import or invoke the named function for you; it's a tag,
not a hook.

## Quirks (SPEC backlog)

A few rough edges to be aware of. These are intentional 0.1 simplifications,
flagged here so they don't surprise users and so the SPEC can address them
in a later version:

- **`min`/`max` are numeric only.** They do not accept strings. For string
  length bounds, use `length(min = …, max = …)`. The macro emits a clear
  compile error if you put `min`/`max` on a `String` field.
- **Patterns are passed verbatim.** They're compiled by Valibot/Zod on the
  TS side and by the [`regex`](https://docs.rs/regex) crate on the Rust
  side. JS regex flavor differs slightly from Rust's RE2-ish flavor —
  lookahead/lookbehind, named groups, and Unicode classes don't always
  round-trip. Stick to the common subset (character classes, anchors,
  quantifiers) for portable patterns.
- **`email` and `url` are deliberately weak.** `email` is a syntactic
  "has-an-`@`-and-a-dot" check, not RFC 5322. `url` is a
  "starts with `http://` or `https://`" check, not RFC 3986. For stricter
  validation, layer `pattern` or `custom` on top.
- **Length on strings counts characters, not bytes or grapheme clusters.**
  See the constraint vocabulary section for the cross-language gotcha.
- **No nested-struct recursion in 0.1.** A field whose type is itself a
  `Validate`-deriving struct does **not** automatically run that inner
  type's checks. Flatten or call `inner.validate()?` manually until 0.2.

## Pointer

A runnable end-to-end demo lives at
[`examples/phase4-validate/`](https://github.com/anthropics/taut-rpc/tree/main/examples/phase4-validate).
It contains a `CreateUser` procedure with the canonical example above, the
generated `api.gen.ts` for both Valibot and Zod, and a small TS test that
exercises the fail-fast and per-call opt-out paths.

## See also

- [SPEC §7 — Validation bridge](../reference/spec.md#7-validation-bridge)
- [Roadmap — Phase 4](../reference/roadmap.md)
- [Type mapping](./type-mapping.md)
- [Errors](./errors.md)

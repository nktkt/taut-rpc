# Validation

`#[derive(Validate)]` is the per-field constraint bridge from
[SPEC §7](../reference/spec.md). Constraints attached to an input
struct's fields are recorded into the IR, lowered to a Valibot (or Zod)
schema by codegen, and enforced **on both sides** — the client parses
inputs before sending, the server parses inputs before the procedure
body runs.

This guide is a cookbook of patterns. For the conceptual chapter — why
Valibot is the default, what's in scope for v0.1 — see
[Validation (concepts)](../concepts/validation.md).

## Quick start

The minimum end-to-end validation surface: a struct with one constraint,
a procedure that consumes it, and a client that calls it.

```rust
// server/src/main.rs
use serde::{Deserialize, Serialize};
use taut_rpc::{dump_if_requested, rpc, Router, Type, Validate};

#[derive(Serialize, Deserialize, Type, Validate)]
pub struct Greet {
    #[taut(length(min = 1, max = 64))]
    pub name: String,
}

#[rpc]
async fn greet(input: Greet) -> String {
    format!("hello, {}", input.name)
}

#[tokio::main]
async fn main() {
    let router = Router::new().procedure(__taut_proc_greet());
    dump_if_requested(&router);
    let app = router.into_axum();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:7710")
        .await
        .expect("bind");
    axum::serve(listener, app).await.expect("server crashed");
}
```

Run sequence:

```sh
# 1. Build the server so the IR-dump binary exists.
cd server && cargo build && cd ..

# 2. Run codegen against the binary; the IR never escapes target/taut/.
cargo run -p taut-rpc-cli -- taut gen \
  --from-binary server/target/debug/your-server \
  --out         client/src/api.gen.ts

# 3. Start the server and the client in two terminals.
cd server && cargo run                           # terminal A
cd client && npm install && npm run start        # terminal B
```

The generated `api.gen.ts` exports a `procedureSchemas` map alongside
`createApi`. Wire it in once and validation is on by default:

```ts
import { createApi, procedureSchemas } from "./api.gen.js";

const client = createApi({
  url: "http://127.0.0.1:7710",
  schemas: procedureSchemas,
});

await client.greet({ name: "" });
// throws TautError(code = "validation_error") before any network call
```

## Pattern: form-style fields

A contact / signup / feedback form: each field has a length window and
one or two format constraints.

```rust
use serde::{Deserialize, Serialize};
use taut_rpc::{Type, Validate};

#[derive(Serialize, Deserialize, Type, Validate)]
pub struct ContactForm {
    #[taut(length(min = 1, max = 100))]
    pub name: String,
    #[taut(email)]
    pub email: String,
    #[taut(length(max = 5_000))]
    pub message: String,
}
```

Two things to notice:

1. **Client-side schema emission.** Codegen produces a `ContactForm`
   entry inside `procedureSchemas` whose `input` slot is a Valibot pipe
   (`v.object({ name: v.pipe(v.string(), v.minLength(1),
   v.maxLength(100)), email: v.pipe(v.string(), v.email()), ... })`).
   Whichever procedure consumes `ContactForm` picks it up automatically.
2. **`validate.send` defaults to `true`.** When `schemas` is supplied,
   inputs are parsed before sending unless you explicitly set
   `validate.send: false`. The `email` field above rejects `"not an
   email"` *before* the request hits the network.

## Pattern: numeric ranges (price / quantity)

Bounds-checking — order line items, ages, page sizes. `min` / `max`
compare the field's *value*, not its length, and work on any
integer or float primitive in [the type mapping](../concepts/type-mapping.md).

```rust
use serde::{Deserialize, Serialize};
use taut_rpc::{Type, Validate};

#[derive(Serialize, Deserialize, Type, Validate)]
pub struct OrderItem {
    #[taut(length(min = 1, max = 64))]
    pub sku: String,
    #[taut(min = 0.01, max = 99_999.99)]
    pub price_usd: f64,
    #[taut(min = 1, max = 1000)]
    pub quantity: u32,
}
```

`length` on `sku` is character count; `min` / `max` on the numerics
is value. Mixing the two on a single field is a compile error — see
[Pitfalls](#pitfalls).

## Pattern: enum-style strings via `pattern`

When a field has to be one of a small fixed set of strings, `pattern`
with an alternation is one option:

```rust
use serde::{Deserialize, Serialize};
use taut_rpc::{Type, Validate};

#[derive(Serialize, Deserialize, Type, Validate)]
pub struct UserRole {
    #[taut(pattern = "^(admin|user|guest)$")]
    pub role: String,
}
```

But a Rust enum is almost always better:

```rust
#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Admin,
    User,
    Guest,
}

#[derive(Serialize, Deserialize, Type, Validate)]
pub struct UserRoleTyped {
    pub role: Role,
}
```

The enum form is preferable because:

- **The compiler enforces it.** Adding a fourth variant on the server
  forces every Rust call site to update, and codegen propagates the
  change to the TS side as a discriminated string union.
- **Better TS narrowing.** TS sees `"admin" | "user" | "guest"`, not
  `string`. Switch statements are exhaustive.
- **No regex flavor mismatch.** The `pattern` form ships a regex to
  the client; the enum form has no regex to ship.

Reach for `pattern` when the set is genuinely string-shaped (e.g.
`country: ISO 3166-1 alpha-2`) and a Rust enum would be unwieldy.

## Pattern: custom predicates

Anything outside the v0.1 set (`min`, `max`, `length`, `pattern`,
`email`, `url`) is recorded into the IR as an opaque `custom` tag.
Codegen leaves the schema slot empty — it can't know what
`"isAvailable"` means — so the user supplies a fragment.

```rust
use serde::{Deserialize, Serialize};
use taut_rpc::{Type, Validate};

#[derive(Serialize, Deserialize, Type, Validate)]
pub struct Username {
    #[taut(custom = "isAvailable")]
    pub handle: String,
}
```

On the TS side, wrap `procedureSchemas` to splice your check in:

```ts
import * as v from "valibot";
import { createApi, procedureSchemas, type Procedures } from "./api.gen.js";

// Wrap procedureSchemas with your custom validator:
const enrichedSchemas = {
  ...procedureSchemas,
  create_user: {
    ...procedureSchemas.create_user,
    input: v.pipe(
      procedureSchemas.create_user.input,
      v.check(
        (u: { handle: string }) => isAvailable(u.handle),
        "handle is taken",
      ),
    ),
  },
};

const client = createApi({
  url: "http://127.0.0.1:7710",
  schemas: enrichedSchemas,
});
```

The Rust-side `Validate::validate` impl runs the same predicate on the
server, so the constraint is enforced even if the client is bypassed.
Custom predicates that need server-only context (a database lookup, a
feature flag) are usually better expressed as a typed `TautError`
variant in the procedure body — see [Errors](../concepts/errors.md).

## Disabling validation per call

When you trust the input — server-internal calls, replay-from-log,
known-good fixtures — pass `validate.send: false`:

```ts
import { createApi, procedureSchemas } from "./api.gen.js";

const internalClient = createApi({
  url: "http://internal-services/rpc",
  schemas: procedureSchemas,
  validate: { send: false },
});
```

A few notes:

- `validate.recv` is independent. The example above leaves output
  validation on; set both to `false` to disable both directions.
- **The server still validates.** `validate.send: false` only skips the
  *client-side* parse; the server's `Validate::validate` runs on every
  request. Malformed input still comes back as a `validation_error`
  envelope — just from the server, not the client.
- Per-call toggles are not in v0.1; the flag is client-wide. For
  per-call disablement, keep two `createApi` instances and pick at the
  call site.

## Switching to Zod

Valibot is the default. Zod is a drop-in alternative: re-run codegen
with `--validator zod`, install `zod`, no client code changes.

```sh
# Re-run codegen with the Zod target.
cargo run -p taut-rpc-cli -- taut gen \
  --from-binary server/target/debug/your-server \
  --out         client/src/api.gen.ts \
  --validator   zod

# Install Zod alongside (or instead of) Valibot.
cd client && npm install zod
```

The generated `api.gen.ts` switches its imports from `valibot` to
`zod` and emits `z.object({ ... })` instead of `v.object({ ... })`.
Both expose `.parse(value)` — the runtime's `SchemaLike` duck-type —
so `createApi` wiring is identical and no caller code changes.

Hand-written custom-predicate fragments need to be ported from
`v.check(...)` to `z.refine(...)`. Otherwise it's a one-flag flip.

## Surfacing validation errors in UI

Validation rejections reach the caller as a `TautError` with `code =
"validation_error"`. The payload is `{ direction, issues: [{ path,
message }, ...] }` — the same shape whether the rejection happened
client-side (before the network call) or server-side (before the
procedure body), so user code only has one parser path.

```ts
import { isTautError } from "taut-rpc";

interface ValidationPayload {
  direction: "input" | "output";
  issues: { path: string; message: string }[];
}

try {
  await client.create_user(form);
} catch (e) {
  if (isTautError(e, "validation_error")) {
    const payload = e.payload as ValidationPayload;
    for (const issue of payload.issues) {
      showFieldError(issue.path, issue.message);
    }
  } else {
    throw e;
  }
}
```

`path` strings are dotted/bracketed (`"email"`, `"items[0].sku"`) so
they match form-field names directly. `direction` lets logging
distinguish `"input"` (the user sent invalid data) from `"output"` (the
server sent the client something it can't parse). The latter is almost
always a schema drift bug — surface it loudly in dev builds.

## Pitfalls

- **`min` / `max` operate on numbers; for character count on strings
  use `length`.** Annotating `name: String` with `#[taut(min = 1)]` is
  a compile error — `min` requires a numeric field. Use
  `#[taut(length(min = 1))]`. `length` on a numeric field is also
  rejected.

- **`email` is intentionally lax.** Valibot / Zod `email` is a basic
  format check, not an RFC 5321 / 5322 pass. For RFC-strict checks use
  `pattern` with a stricter regex, or — preferably — a `custom`
  predicate that DNS-checks the domain.

- **The `regex` crate's flavor is RE2-like.** Some JS regex features
  (lookbehind, backreferences, possessive quantifiers) aren't
  supported. Patterns like `(?<=foo)bar` may compile on the client but
  the server-side `Validate::validate` rejects them at startup. Stick
  to the RE2 subset.

- **`pattern` is unanchored on the client.** Valibot's `regex` matches
  anywhere in the string by default; anchor with `^...$` for a
  full-string match. The phase-4 example uses `^[a-z0-9_]+$` for
  exactly this reason.

- **Validation rejection vs application error.** A `validation_error`
  means "input malformed at the schema level"; an application error
  like `username_taken` means "input well-formed, business logic says
  no". Use different `code`s on purpose — don't shoehorn "username
  already exists" into a `custom` predicate.

## Working with Valibot directly

The generated `procedureSchemas` map is the standard wiring for the
runtime, but the per-struct schemas are also exported by name. That
makes it cheap to reuse a schema in a form library, in a custom
pipeline, or to extend it with checks the IR can't express.

### Importing the generated schemas

Every input struct gets its own named export alongside the
`procedureSchemas` map:

```ts
import { CreateUserSchema, procedureSchemas } from "./api.gen";
```

`CreateUserSchema` is the same Valibot object referenced by
`procedureSchemas.create_user.input` — they're the same instance, so
extending one extends both as long as you wire the result back through
the schema map.

### Using a generated schema with React Hook Form (Valibot)

`@hookform/resolvers/valibot` accepts any Valibot schema:

```tsx
import { valibotResolver } from "@hookform/resolvers/valibot";
import { useForm } from "react-hook-form";
import { CreateUserSchema } from "./api.gen";

const form = useForm({ resolver: valibotResolver(CreateUserSchema) });
```

The form now produces validation errors with the same paths and
messages the runtime would emit on `client.create_user(...)`. No
duplicated rules between the form and the wire schema.

### Composing custom checks (Valibot)

Pipe the generated schema through extra `v.check(...)` calls when you
need a constraint the IR can't carry — typically a cross-field rule:

```ts
import * as v from "valibot";
import { CreateUserSchema } from "./api.gen";

const StrongerCreateUser = v.pipe(
  CreateUserSchema,
  v.check(u => u.username !== u.password, "username and password must differ"),
);
```

To make `StrongerCreateUser` the schema actually used at the wire,
splice it back into a wrapped `procedureSchemas` (see [Pattern: custom
predicates](#pattern-custom-predicates) above).

### The same with Zod

If you ran codegen with `--validator zod`, the named export is a Zod
object. Resolver wiring and composition are equivalent, just in Zod's
idiom:

```tsx
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { CreateUserSchema } from "./api.gen";

const form = useForm({ resolver: zodResolver(CreateUserSchema) });
```

```ts
import { CreateUserSchema, ExtraFieldsSchema } from "./api.gen";

const StrongerCreateUser = CreateUserSchema.refine(
  u => u.username !== u.password,
  { message: "username and password must differ" },
);

// Merge in additional fields from another generated struct:
const Combined = CreateUserSchema.merge(ExtraFieldsSchema);
```

`.refine` plays the role of `v.check`; `.merge` composes objects when
you want to extend the wire shape locally (e.g. carrying a
client-only `confirmPassword` field that never reaches the server).

## Patterns for cross-field validation

Per-field constraints handle "this string is at most 64 chars". They
don't handle "field A and field B must differ" or "if `kind = paid`
then `price > 0`". Cross-field rules need a hook on both sides.

### 1. Record the intent in the IR

Use `#[taut(custom = "…")]` to tag the field whose validity depends on
the rest of the struct. The tag is opaque to codegen — it's a name —
but it's recorded in the IR so consumers know "this struct has a
named custom rule attached":

```rust
use serde::{Deserialize, Serialize};
use taut_rpc::{Type, Validate};

#[derive(Serialize, Deserialize, Type, Validate)]
pub struct CreateUser {
    #[taut(length(min = 3, max = 32))]
    pub username: String,
    #[taut(length(min = 8))]
    #[taut(custom = "matches_password")]
    pub password: String,
    pub password_confirm: String,
}
```

The IR now carries `custom = "matches_password"` on the `password`
field. The TS-side schema slot for that check is empty — codegen
doesn't know what `"matches_password"` should *do*.

### 2. Wire the predicate on the TS side

Use the schema-merge pattern from above to attach the actual check:

```ts
import * as v from "valibot";
import { CreateUserSchema, procedureSchemas } from "./api.gen";

const CreateUserWithMatch = v.pipe(
  CreateUserSchema,
  v.check(
    u => u.password === u.password_confirm,
    "passwords must match",
  ),
);

const enrichedSchemas = {
  ...procedureSchemas,
  create_user: {
    ...procedureSchemas.create_user,
    input: CreateUserWithMatch,
  },
};
```

Pass `enrichedSchemas` to `createApi` and the cross-field rule fires
client-side before the request leaves the browser.

### 3. Enforce the same rule server-side

The auto-derived `Validate::validate` on `CreateUser` only knows about
the per-field tags. To enforce the cross-field rule, override it. Two
options:

**Option A — manual `Validate` impl on the same struct.** Drop the
`Validate` from `derive(...)` and write the impl yourself, calling
into the generated per-field checks (or rebuilding them) and adding
the cross-field branch:

```rust
use taut_rpc::{Validate, ValidationError};

impl Validate for CreateUser {
    fn validate(&self) -> Result<(), ValidationError> {
        // …per-field checks here…
        if self.password != self.password_confirm {
            return Err(ValidationError::field(
                "password_confirm",
                "passwords must match",
            ));
        }
        Ok(())
    }
}
```

**Option B — wrapper struct.** Keep the auto-derive and add an outer
type that runs both:

```rust
#[derive(Serialize, Deserialize, Type)]
pub struct CreateUserChecked(pub CreateUser);

impl Validate for CreateUserChecked {
    fn validate(&self) -> Result<(), ValidationError> {
        self.0.validate()?;
        if self.0.password != self.0.password_confirm {
            return Err(ValidationError::field(
                "password_confirm",
                "passwords must match",
            ));
        }
        Ok(())
    }
}
```

Option A is shorter; Option B is preferable when several procedures
share a base struct but only some need the extra rule.

Either way the server is the security boundary — even if a caller
bypasses the client, the cross-field rule still runs.

## Performance

Validation is cheap, but worth knowing the shape of:

- **Typical overhead is on the order of microseconds.** A struct with
  ~10 fields and a handful of length / range / pattern constraints
  parses in single-digit microseconds on both sides. For request rates
  in the thousands per second this is in the noise.

- **Hot endpoints can disable client-side validation.** When a path is
  on a tight latency budget — analytics ingestion, an autocomplete
  hot loop — set `validate.send: false` to skip the client-side parse.
  Server-side validation is the security boundary; the client parse
  is a developer-experience layer that catches mistakes early. Turning
  it off on a known-good code path is safe:

  ```ts
  const fastClient = createApi({
    url: "http://127.0.0.1:7710",
    schemas: procedureSchemas,
    validate: { send: false },
  });
  ```

- **Server-side regex patterns are compiled per request in v0.1.**
  `pattern = "..."` constraints currently re-compile their regex on
  every `Validate::validate` call. For typical patterns this is still
  microseconds, but if you have a hot endpoint with several `pattern`
  fields and profiling shows regex compilation in the flame graph,
  file an issue — caching compiled patterns in a `OnceLock` is on the
  v0.2 list and we'd prioritize it with a real workload to point at.

## See also

- [Validation (concepts)](../concepts/validation.md) — what's in v0.1
  and why.
- [Errors](../concepts/errors.md) — narrowing typed errors with
  `isTautError` / `errorMatch`.
- [SPEC §7 — Validation bridge](../reference/spec.md)
- `examples/phase4-validate/` — runnable end-to-end example exercising
  every v0.1 constraint kind.

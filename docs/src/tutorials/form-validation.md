# Tutorial: Form with validation

This tutorial walks end-to-end through a small but realistic feature: a
**signup form** that runs the same validation rules in the browser and
on the server, surfaces per-field errors in the UI, and falls back
cleanly when the business layer rejects an otherwise well-formed
request.

By the end you will have:

1. A `SignupInput` struct deriving `Validate`, with length, format,
   numeric, and custom-predicate constraints.
2. A `signup` mutation returning a typed `SignupError` for the cases
   validation can't catch (`UsernameTaken`, `EmailExists`).
3. A generated `api.gen.ts` containing per-procedure Valibot schemas.
4. A React signup form that displays per-field error messages from
   either side of the wire.
5. A test that disables client-side validation to prove the server
   enforces the same rules.

The runnable analogue lives at `examples/phase4-validate/` — same
shape, fewer features. Read this chapter first; lift code from there.

## 1. Define the input type

`SignupInput` carries every Phase 4 constraint kind plus one custom
predicate. The constraints are recorded into the IR by the `rpc`
attribute machinery and are lowered to a Valibot pipe by codegen — see
[Validation (concepts)](../concepts/validation.md) for the full chain.

```rust
// server/src/main.rs
use serde::{Deserialize, Serialize};
use taut_rpc::{rpc, dump_if_requested, Router, Type, Validate};

#[derive(Serialize, Deserialize, Type, Validate)]
pub struct SignupInput {
    /// 3..=32 chars, lowercase alnum + underscore. Two constraints on
    /// one field stack into a single Valibot pipe.
    #[taut(length(min = 3, max = 32))]
    #[taut(pattern = "^[a-z0-9_]+$")]
    pub username: String,

    /// Basic format check. Use `pattern` or a `custom` predicate if
    /// you need RFC-strict validation.
    #[taut(email)]
    pub email: String,

    /// COPPA floor + sane upper bound.
    #[taut(min = 13, max = 120)]
    pub age: u8,

    /// Length floor only — open-ended is fine. The custom strength
    /// check runs alongside (and after) the length check.
    #[taut(length(min = 8))]
    #[taut(custom = "strong_password")]
    pub password: String,
}
```

A few callouts that bite first-time users:

- **`min` / `max` operate on numbers**, not character counts. The
  `username` field uses `length(min = 3, max = 32)` for that reason.
  Mixing the two on one field is a compile error.
- **`pattern` is unanchored** on the Valibot side; the `^...$` is
  load-bearing. Without anchors, `"alice!"` matches `"^[a-z0-9_]+$"`
  because the regex finds `"alice"` inside it.
- **The `custom = "strong_password"` slot is opaque to codegen.** It
  records the name into the IR but emits no schema fragment; we wire
  the TS-side check in [§6](#6-custom-predicate-wiring) and the Rust
  impl runs server-side as part of `Validate::validate`.

## 2. The `signup` procedure

Validation runs *before* the procedure body — by the time `signup` is
called, every field has already passed every constraint. So
`SignupError` covers only the cases that survive a well-formed input:
the username or email is taken.

```rust
use thiserror::Error;
use taut_rpc::TautError;

pub type UserId = u32;

#[derive(Serialize, Type, TautError, Error, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum SignupError {
    #[error("username already taken")]
    UsernameTaken,
    #[error("an account with that email already exists")]
    EmailExists,
}

#[rpc(mutation)]
async fn signup(input: SignupInput) -> Result<UserId, SignupError> {
    if input.username == "taken" {
        return Err(SignupError::UsernameTaken);
    }
    if input.email == "dup@example.com" {
        return Err(SignupError::EmailExists);
    }
    Ok(42)
}

#[tokio::main]
async fn main() {
    let router = Router::new().procedure(__taut_proc_signup());
    dump_if_requested(&router);
    let app = router.into_axum();
    let listener = tokio::net::TcpListener::bind("0.0.0.0:7720")
        .await
        .expect("bind 0.0.0.0:7720");
    axum::serve(listener, app).await.expect("server crashed");
}
```

Why a typed enum and not just a string? The TS side gets a
discriminated union (`{ code: "username_taken" } | { code:
"email_exists" }`), which the form below narrows on with
`isTautError`. See [Errors](../concepts/errors.md) for the full
contract.

## 3. Generate the client

Build the server, dump the IR, and run codegen. The IR never leaves
`target/taut/` — it's an internal handoff, not a public artifact.

```sh
cd server && cargo build && cd ..

cargo run -p taut-rpc-cli -- taut gen \
  --from-binary server/target/debug/signup-server \
  --out         client/src/api.gen.ts \
  --validator   valibot
```

`--validator valibot` is the default; spelling it out makes the
intent obvious in CI scripts. Switching to Zod is a one-flag flip — see
[the validation guide](../guides/validation.md#switching-to-zod).

The generated `api.gen.ts` exports two things you'll touch:

- `createApi(opts)` — the typed client constructor.
- `procedureSchemas` — a `{ signup: { input, output } }` map of Valibot
  schemas, one entry per procedure.

## 4. Wire up the React form

Pass `procedureSchemas` to `createApi`; that turns on client-side
validation by default (`validate.send` defaults to `true` whenever
`schemas` is set).

```tsx
// client/src/SignupForm.tsx
import { useState } from "react";
import { createApi, procedureSchemas } from "./api.gen.js";
import { isTautError } from "taut-rpc";

const client = createApi({
  url: "http://127.0.0.1:7720",
  schemas: procedureSchemas,
});

interface FieldErrors {
  username?: string;
  email?: string;
  age?: string;
  password?: string;
  _form?: string; // top-level error (e.g. UsernameTaken)
}

export function SignupForm() {
  const [form, setForm] = useState({
    username: "",
    email: "",
    age: 18,
    password: "",
  });
  const [errors, setErrors] = useState<FieldErrors>({});
  const [userId, setUserId] = useState<number | null>(null);

  async function onSubmit(e: React.FormEvent) {
    e.preventDefault();
    setErrors({});
    try {
      const id = await client.signup(form);
      setUserId(id);
    } catch (err) {
      setErrors(toFieldErrors(err));
    }
  }

  return (
    <form onSubmit={onSubmit}>
      <Field
        label="Username"
        value={form.username}
        error={errors.username}
        onChange={v => setForm({ ...form, username: v })}
      />
      <Field
        label="Email"
        value={form.email}
        error={errors.email}
        onChange={v => setForm({ ...form, email: v })}
      />
      <Field
        label="Age"
        type="number"
        value={String(form.age)}
        error={errors.age}
        onChange={v => setForm({ ...form, age: Number(v) })}
      />
      <Field
        label="Password"
        type="password"
        value={form.password}
        error={errors.password}
        onChange={v => setForm({ ...form, password: v })}
      />
      {errors._form && <p className="form-error">{errors._form}</p>}
      {userId != null && <p>Welcome, user #{userId}!</p>}
      <button type="submit">Sign up</button>
    </form>
  );
}
```

`Field` is a thin wrapper that renders a label, input, and per-field
error message. The interesting code is `toFieldErrors` below.

## 5. Surface errors per field

Validation rejections — from either side — arrive as `TautError` with
`code = "validation_error"` and a payload of `{ direction, issues:
[{ path, message }, ...] }`. The `path` strings match field names, so
populating per-field error state is a one-liner.

```ts
interface ValidationPayload {
  direction: "input" | "output";
  issues: { path: string; message: string }[];
}

function toFieldErrors(err: unknown): FieldErrors {
  if (isTautError(err, "validation_error")) {
    const payload = err.payload as ValidationPayload;
    const fields: FieldErrors = {};
    for (const { path, message } of payload.issues) {
      // path is "username", "email", etc — the field name verbatim.
      (fields as Record<string, string>)[path] = message;
    }
    return fields;
  }
  if (isTautError(err, "username_taken")) {
    return { username: "That username is already taken." };
  }
  if (isTautError(err, "email_exists")) {
    return { email: "An account with that email already exists." };
  }
  // Network failure, server crash, etc — surface generically.
  return { _form: "Something went wrong. Try again." };
}
```

Three things to notice:

1. **One parser path covers both sides.** The `validation_error`
   envelope is identical whether it came from the client-side parse
   (before the request hit the network) or the server-side parse
   (before the procedure body). User code doesn't branch on
   `direction` unless logging needs to.
2. **Application errors map to fields too.** `UsernameTaken` is *not*
   a validation error — the input was well-formed; the business layer
   said no — but the UI still wants it under the `username` field. The
   narrowing on `isTautError(err, "username_taken")` handles that.
3. **`isTautError` is the discriminator.** Don't reach into `e.code`
   directly; the helper narrows the TS type to the right payload
   shape.

## 6. Custom predicate wiring

`#[taut(custom = "strong_password")]` is recorded into the IR but
codegen leaves the schema slot empty — it has no idea what
"strong_password" means. You supply both sides:

**Server-side**: implement the predicate as part of `Validate`. The
derive macro lifts `custom = "name"` into a call to a function the
caller defines in scope:

```rust
// server/src/main.rs
fn strong_password(s: &str) -> Result<(), &'static str> {
    let has_upper = s.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    if has_upper && has_digit {
        Ok(())
    } else {
        Err("password must contain an uppercase letter and a digit")
    }
}
```

**Client-side**: wrap `procedureSchemas` with a Valibot `check` that
mirrors the same rule:

```ts
// client/src/api.ts
import * as v from "valibot";
import { createApi, procedureSchemas } from "./api.gen.js";

const strongPassword = (s: string): boolean =>
  /[A-Z]/.test(s) && /[0-9]/.test(s);

const enrichedSchemas = {
  ...procedureSchemas,
  signup: {
    ...procedureSchemas.signup,
    input: v.pipe(
      procedureSchemas.signup.input,
      v.check(
        (u: { password: string }) => strongPassword(u.password),
        "password must contain an uppercase letter and a digit",
      ),
    ),
  },
};

export const client = createApi({
  url: "http://127.0.0.1:7720",
  schemas: enrichedSchemas,
});
```

The two implementations are obligated to agree. If they drift, the
client-side check passes a password the server then rejects — the user
sees a delayed error after a network round-trip. For predicates that
*must* be server-only (database lookups, feature flags), express them
as a typed `TautError` variant in the procedure body instead of a
`custom` predicate; that way the client never claims to validate them.

If you switch to Zod (`--validator zod`), translate `v.check(...)` to
`z.refine(...)`. The schema slot wrapping is otherwise identical.

## 7. Disable client validation in tests

The contract says the **server enforces validation regardless of the
client**. To prove it, run a client with `validate.send: false`: the
input skips the client-side parse, hits the wire as-is, and the
server's `Validate::validate` impl rejects it with the same
`validation_error` envelope.

```ts
// client/src/server-enforcement.test.ts
import { test, expect } from "vitest";
import { createApi, procedureSchemas } from "./api.gen.js";
import { isTautError } from "taut-rpc";

const noClientCheck = createApi({
  url: "http://127.0.0.1:7720",
  schemas: procedureSchemas,
  validate: { send: false },
});

test("server rejects short username even with client-side parse off", async () => {
  try {
    await noClientCheck.signup({
      username: "ab", // length 2 — fails the min(3) check
      email: "ok@ok.co",
      age: 30,
      password: "Password1",
    });
    throw new Error("server accepted invalid input");
  } catch (e) {
    expect(isTautError(e, "validation_error")).toBe(true);
  }
});
```

`validate.send: false` only disables the **client-side** parse;
`validate.recv` is independent and still validates response payloads.
The flag is client-wide — there's no per-call toggle in v0.1, so keep a
second `createApi` instance for the bypass path (see also [Disabling
validation per call](../guides/validation.md#disabling-validation-per-call)).

This pattern is what `examples/phase4-validate/client/src/main.ts`
uses to demonstrate server enforcement; lift it verbatim for your own
test suite.

## What you built

- **One source of truth.** `SignupInput`'s constraints define the
  schema once; codegen ships them to the client and the derive macro
  enforces them on the server.
- **Symmetric error envelope.** `validation_error` looks identical
  from either side; the form's per-field error map needs only one
  parser path.
- **Typed business errors.** `UsernameTaken` and `EmailExists` are
  distinct `code` values, narrowed with `isTautError`, displayed under
  the right field.
- **Provable server enforcement.** A bypass-client test confirms the
  server rejects what the client would have caught.

## See also

- [Validation (guide)](../guides/validation.md) — full cookbook of
  constraint patterns and pitfalls.
- [Validation (concepts)](../concepts/validation.md) — what's in v0.1
  and why Valibot is the default.
- [Errors](../concepts/errors.md) — typed-error narrowing in detail.
- [Constraints reference](../reference/constraints.md) — per-attribute
  table.
- `examples/phase4-validate/` — the runnable shorter version of this
  tutorial.

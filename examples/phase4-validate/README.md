# phase4-validate — `#[derive(Validate)]` + Valibot bridge end-to-end

This example demonstrates the Phase 4 exit criterion straight from SPEC §7:

> `#[derive(Validate)]` on input types emits a per-field schema description
> into the IR. Codegen produces a Valibot schema. The generated client
> validates inputs *before* sending and validates outputs *after* receiving
> by default; both can be disabled per-call.

The server defines a single mutation `create_user(CreateUser) -> User` whose
input has every supported v0.1 constraint (`length`, `email`, `min`, `max`,
`pattern`, `url`) attached as `#[taut(...)]` attributes. The codegen lowers
those constraints to a Valibot schema in `api.gen.ts`, and the generated
`createApi` helper wires the schema map into the runtime so invalid inputs
fail *before* a network round-trip.

If this round-trips end-to-end — Rust constraints → IR → Valibot schema →
client-side rejection — Phase 4's promise is delivered: the validation
contract has the same end-to-end safety as the type contract.

The example lives outside the cargo workspace (`exclude = ["examples"]` in
the root `Cargo.toml`) so it resolves its own dependencies through path
entries and never piggy-backs on workspace machinery.

## What it covers

- `ping() -> &'static str` — sanity-check unary query, no input, no
  validation. Confirms validation didn't regress the unconstrained path.
- `create_user(CreateUser) -> Result<User, CreateUserError>` — the headline
  mutation. `CreateUser` carries every v0.1 constraint:
  - `username: String` with `length(min = 3, max = 32)`
  - `email: String` with `email`
  - `age: u8` with `min = 18, max = 120`
  - `handle: String` with `pattern = "^[a-z0-9_]+$"`
  - `homepage: String` with `url`

  The procedure also surfaces a typed application-layer error
  (`CreateUserError::UsernameTaken`) so the client demonstrates the full
  three-way distinction between client-side validation rejection,
  server-side validation rejection, and server-side application error.

## Run sequence

The pipeline mirrors earlier phases (build server → `cargo taut gen` → run
client) with one extra step: the npm runtime needs to be built once so the
client can resolve `taut-rpc` from its file: dependency.

### 1. Build the npm runtime (once per checkout)

```sh
cd npm/taut-rpc
npm install
npm run build
```

### 2. Build the server in IR-dump mode and run codegen

From the repository root:

```sh
cd examples/phase4-validate/server && cargo build && cd ../../..
cargo run -p taut-rpc-cli -- taut gen \
  --from-binary examples/phase4-validate/server/target/debug/phase4-validate-server \
  --out         examples/phase4-validate/client/src/api.gen.ts
```

`--from-binary` re-spawns the server binary with `TAUT_DUMP_IR` set, captures
its IR, then runs the codegen step — see `dump_if_requested(&router)` in
`server/src/main.rs`. The IR never escapes `target/taut/ir.json` and no port
is ever bound during the dump.

The generated `api.gen.ts` will export `procedureSchemas` alongside
`createApi`; the client wires both into `createClient` so validation is on
by default.

### 3. Install the client and start the server

```sh
# terminal A
cd examples/phase4-validate/server
cargo run
```

The server binds `0.0.0.0:7705` (note the different port from previous
examples — Phase 0 smoke uses 7700, Phase 1 uses 7701, Phase 2 uses
7702/7703, Phase 3 uses 7704, so all five can run side-by-side) and prints:

```
phase4-validate-server listening on http://127.0.0.1:7705
```

CORS is permissive — this is a local-only example, not a deployment template.

### 4. Run the client

```sh
# terminal B, with the server running and api.gen.ts generated
cd examples/phase4-validate/client
npm install
npm run start
```

You should see roughly:

```
ping: pong
created: { id: 1n, username: 'alice' }
server rejected: username_taken
client rejected: <validation issues array — invalid email>
server rejected: <validation issues array — username too short>
```

The four `create_user` calls demonstrate the four interesting cases:

1. **Success.** A fully valid input round-trips and returns the new `User`.
2. **Server-side application error.** The input passes both client- and
   server-side validation, but the application logic rejects the username
   `"taken"` with `CreateUserError::UsernameTaken`. The client narrows the
   thrown `TautError` via `isTautError(e, "username_taken")`.
3. **Client-side validation rejection.** An invalid `email` is caught
   *before* the request leaves the client, surfaced as a `TautError` with
   `code = "validation_error"`. There is no network round-trip; the server
   never sees this request.
4. **Server-side validation rejection.** A second client is constructed
   with `validate.send: false`, bypassing the client-side parse. The
   too-short `username` (`"ab"`) reaches the server, fails the same
   `Validate` impl, and comes back as a `TautError` with `code =
   "validation_error"` — same envelope shape as the client-side rejection,
   so user code only has one parser path.

## A note on the wire format

Validation errors use the standard SPEC §4.1 error envelope with `code =
"validation_error"`. The payload is a flat `[{ path, message }]` array, the
same shape on the client side and the server side. The server's
`Validate::validate` runs *before* the procedure body — see the
`validate_input` step inside the macro-generated handler — and short-circuits
to the same envelope shape if any field fails.

## What this is not

- It is **not** a deployment template. The server has permissive CORS, no
  auth, and binds `0.0.0.0`.
- It is **not** wired into any test harness yet. The exit criterion for
  Phase 4 is "a constraint violation on the client side fails before the
  network call, while the same constraint violation on the server side
  surfaces as a structured error" — running this end-to-end by hand is the
  test for now.

# phase2-auth — `tower::Layer` + `#[derive(TautError)]` end-to-end

This example demonstrates how Phase 2 of taut-rpc composes with the standard
axum/tower middleware ecosystem to deliver SPEC-shaped typed errors:

- An `axum::middleware::from_fn` auth layer inspects
  `Authorization: Bearer <token>` and short-circuits unauthenticated requests
  with the SPEC §4.1 envelope `{"err":{"code":"unauthenticated","payload":null}}`
  and HTTP `401`. The procedure handler is never invoked.
- A typed `AuthError` enum derives `taut_rpc::TautError` (SPEC §3.3) so any
  procedure-level failure (e.g. `Forbidden { required_role }`) flows through
  the same envelope with the correct HTTP status.
- The TS client narrows on `e.code` via `isTautError(e, "unauthenticated" | "forbidden")`,
  exactly as called out in SPEC §3.3's `ApiError<C, P>` type.

If this round-trips, Phase 2 is exercised end-to-end: middleware-level
rejection, procedure-level typed error, and the TS-side narrowing all share
one wire shape.

The example lives outside the cargo workspace (`exclude = ["examples"]` in
the root `Cargo.toml`) so it resolves its own dependencies through path
entries and never piggy-backs on workspace machinery.

## What it covers

- `ping() -> &'static str` — public, bypasses the auth layer.
- `whoami() -> Result<User, AuthError>` — gated; unauthenticated callers are
  short-circuited at the layer with HTTP 401 + code `"unauthenticated"`.
- `get_secret() -> Result<String, AuthError>` — gated AND role-checked at the
  layer; non-admin callers see HTTP 403 + code `"forbidden"` with payload
  `{ "required_role": "admin" }`.

The `AuthError` enum uses the canonical Phase 2 derive stack:

```rust
#[derive(serde::Serialize, taut_rpc::Type, taut_rpc::TautError, thiserror::Error, Debug)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum AuthError {
    #[taut(status = 401)]
    #[error("unauthenticated")]
    Unauthenticated,
    #[taut(status = 403)]
    #[error("forbidden: {required_role}")]
    Forbidden { required_role: String },
}
```

`#[derive(TautError)]` supplies `code()` (snake_case of the variant name) and
`http_status()` (overridable via `#[taut(status = ...)]`); serde supplies the
on-wire `tag`/`content` shape per SPEC §3.3.

### A note on state plumbing

Phase 2 does **not** add `State<S>` extractor support to `#[rpc]` — that
lands with Phase 3. The "current user" is therefore decided at the layer
level: the auth layer makes the access-control decision and short-circuits,
and procedure bodies are stubs that return canned responses. The point of
this example is the **layer + typed error pipeline**, not full state
plumbing. See `server/src/main.rs` for the full reasoning.

## Run the server

The example is outside the workspace, so the usual `cargo run -p
phase2-auth-server` **will not work**. Run it from its own directory:

```sh
cd examples/phase2-auth/server
cargo run
```

The server binds `0.0.0.0:7702` (Phase 0 smoke uses 7700, Phase 1 uses 7701,
Phase 2 uses 7702 — all three can run side-by-side) and prints:

```
phase2-auth-server listening on http://127.0.0.1:7702
```

CORS is permissive — this is a local-only example, not a deployment template.

## Generate the typed client

The npm runtime needs to be built once before the client can resolve it:

```sh
cd npm/taut-rpc
npm install
npm run build
```

Then build the server in IR-dump mode and run codegen against the binary.
From the repository root:

```sh
cd examples/phase2-auth/server && cargo build && cd ../../..
cargo run -p taut-rpc-cli -- taut gen \
  --from-binary examples/phase2-auth/server/target/debug/phase2-auth-server \
  --out         examples/phase2-auth/client/src/api.gen.ts
```

`--from-binary` re-spawns the server binary with `TAUT_DUMP_IR` set, captures
its IR, then runs the codegen step — see `dump_if_requested(&router)` in
`server/src/main.rs`. The IR never escapes `target/taut/ir.json` and no port
is ever bound during the dump.

## Run the client

In a second terminal, with the server running and `api.gen.ts` generated:

```sh
cd examples/phase2-auth/client
npm install
npm run start
```

`npm run build-api` is also wired up — it shells out to `cargo run -p
taut-rpc-cli -- taut gen ...` from the client directory.

Expected output (paraphrased):

```
ping: pong
whoami (anonymous) rejected: unauthenticated
whoami (alpha): { id: 1, role: 'user' }
get_secret (alpha) rejected: forbidden { required_role: 'admin' }
get_secret (admin): the cake is a lie
```

Each line corresponds to one of the demo's five steps:

1. **anonymous → ping**: public route, succeeds.
2. **anonymous → whoami**: layer short-circuits with `unauthenticated` (401).
3. **alpha → whoami**: token authenticates a regular user, succeeds.
4. **alpha → get_secret**: layer enforces admin role, rejects with
   `forbidden` (403) and a structured payload.
5. **admin → get_secret**: token authenticates an admin, succeeds.

## What this is not

- It is **not** a deployment template. Tokens are hardcoded and CORS is
  permissive.
- It does **not** propagate the authenticated `User` into procedure bodies
  via a typed extractor. That requires `State<S>` / extension support on
  `#[rpc]`, which is Phase 3.
- Phase 4's input validation (`Validate`) also runs on this example's
  procedures — no-op here, since all three procedures take zero input fields.

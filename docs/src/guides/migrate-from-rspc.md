# Migrating from rspc

If you're coming to `taut-rpc` from [rspc](https://rspc.dev/), most of
the mental model carries over: define procedures in Rust, generate
TypeScript bindings, call them from the client. This guide walks
through the differences that actually matter in practice and gives a
ten-step plan for moving an existing rspc service over.

## Why migrate?

The two projects sit in the same niche, but the design priorities
diverge in three places that show up immediately in real codebases:

1. **Validation is first-class.** `taut-rpc` ships a `Validate` trait
   bridge so input types can carry `garde` / `validator` / `nutype`
   annotations and have constraints surface in the IR — and therefore
   in the generated TS client. rspc treats validation as a
   user-defined concern that lives inside each procedure body.
2. **The IR is on-disk JSON, not in-memory Rust.** `cargo taut gen`
   writes a stable JSON schema artifact at a known path; the codegen
   stage consumes that file. rspc's pipeline assembles a router in-
   memory and emits TS in the same process, which couples codegen to
   a working build of the server crate.
3. **axum is the substrate.** A `taut_rpc::Router` exposes
   `.into_axum()` and inherits everything `axum::Router` does —
   `.layer(...)`, `.nest(...)`, fallbacks, the lot. rspc has its own
   integration crate and its own middleware story.

None of these are inherent rspc limitations; they're tradeoffs. If
you're already happy with rspc, the case for migrating is mostly
"validation in the contract" plus "I want my Rust crate and my TS
codegen step to be decoupled."

## Concept mapping

| rspc                              | taut-rpc                                                |
| --------------------------------- | ------------------------------------------------------- |
| `Router::new()`                   | `taut_rpc::Router::new()`                               |
| `.query("name", \|c, i\| ...)`    | `#[rpc] async fn name(input: I) -> O`                   |
| `.mutation("name", ...)`          | `#[rpc(mutation)] async fn name(...)`                   |
| `.subscription("name", ...)`      | `#[rpc(subscription)] async fn name(...) -> impl Stream` |
| `rspc::Error`                     | `TautError` trait + per-procedure error enums           |
| `specta::Type` derive             | `taut_rpc::Type` derive                                 |
| `rspc_typescript::TypeScript`     | `cargo taut gen`                                        |
| `createClient<Procedures>()`      | `createApi({ url, schemas })`                           |
| Context (`Ctx`) parameter         | axum extractors / middleware-set state                  |

The biggest shape change is the last row: rspc threads a context
through every procedure as the first argument, while `taut-rpc`
follows axum's extractor model. Per-request data lives in request
extensions or `task_local`s, not a positional parameter — see
[Authentication — Where to put state](./auth.md#where-to-put-state).

## Procedure migration

A typical rspc procedure looks like this:

```rust
// rspc
router
    .query("listPosts", |ctx: Ctx, filter: PostFilter| async move {
        ctx.db.find_posts(filter).await
            .map_err(|e| rspc::Error::new(rspc::ErrorCode::InternalServerError, e.to_string()))
    });
```

The same procedure in `taut-rpc`:

```rust
use taut_rpc::{rpc, TautError};

#[derive(Debug, serde::Serialize, TautError)]
#[serde(tag = "code", content = "payload", rename_all = "snake_case")]
pub enum ListPostsError {
    #[taut(status = 500)]
    Database { message: String },
}

#[rpc]
async fn list_posts(filter: PostFilter) -> Result<Vec<Post>, ListPostsError> {
    db()
        .find_posts(filter)
        .await
        .map_err(|e| ListPostsError::Database { message: e.to_string() })
}
```

Three things changed:

- The procedure name moves from a string literal to the function name
  itself. The macro derives the wire path (`/rpc/list_posts`) from
  the identifier.
- The context disappears from the signature. Reach for shared state
  via the patterns in the auth guide.
- The error type is a domain enum, not a stringly-typed `rspc::Error`.
  The TS client narrows on it by `code`.

Mutations get `#[rpc(mutation)]`; subscriptions get
`#[rpc(subscription)]` and return `impl Stream<Item = ...>`. See
[Subscriptions](./subscriptions.md).

## Type migration

rspc inherits its derive from [specta](https://github.com/oscartbeaumont/specta);
`taut-rpc` ships its own `Type` derive in the `taut_rpc` crate. They
serve the same purpose — emit type information for codegen — but they
are not interchangeable, and you should not depend on both.

```rust
// rspc
#[derive(serde::Serialize, serde::Deserialize, specta::Type)]
pub struct Post { id: u64, title: String }

// taut-rpc
#[derive(serde::Serialize, serde::Deserialize, taut_rpc::Type)]
pub struct Post { id: u64, title: String }
```

Most of the time the migration is a literal find-and-replace from
`specta::Type` to `taut_rpc::Type`. Container attributes that are
serde-flavored (`#[serde(rename_all = "...")]`, `#[serde(tag = "...")]`)
are honored by both, so no changes are needed there. Specta-specific
attributes (`#[specta(...)]`) need to be reviewed against the
[Type mapping reference](../concepts/type-mapping.md); most have a
direct equivalent, a few do not.

## Codegen migration

rspc emits TypeScript by running a builder in your `build.rs` or a
small `bin/`:

```rust
// rspc
let router = build_router();
router.export_ts("../client/src/bindings.ts").unwrap();
```

This couples codegen to a successful build of the server crate. If
the server doesn't compile, the client doesn't get fresh types.

`taut-rpc` splits this in two. `cargo taut gen` reads the IR JSON
that the macro emits at compile time and writes TS from it:

```sh
cargo taut gen --out client/src/api/
```

The IR file is a stable artifact you can commit, diff in PRs, and
regenerate from CI. It also means the TS codegen stage doesn't need
the server crate to compile end-to-end — only the IR write step does.
For larger codebases this matters: a failing migration in one
unrelated procedure no longer blocks the whole client codegen.

## Client side

rspc's client is generic over the procedure map:

```ts
// rspc
import { createClient } from "@rspc/client";
import type { Procedures } from "./bindings";

const client = createClient<Procedures>({ url: "/rpc" });
const posts = await client.query(["listPosts", { tag: "rust" }]);
```

`taut-rpc`'s client takes the generated schemas as a value — no
single procedure-map generic to thread:

```ts
// taut-rpc
import { createApi } from "@taut-rpc/client";
import * as schemas from "./api/schemas";

const api = createApi({ url: "/rpc", schemas });
const posts = await api.listPosts({ tag: "rust" });
```

Procedure names become methods on the returned object, so call sites
read like normal function calls instead of tuple-keyed dispatch. If
you've built rspc helpers around the tuple form (a custom React
Query wrapper, say), expect to rewrite them — the per-procedure
shape is genuinely different.

## Error handling

rspc's `rspc::Error` is a single struct with an error code enum and a
message. Everything that goes wrong in a procedure flattens into that
shape, and the TS side gets it as a thrown exception with a string
code.

`taut-rpc` inverts this: each procedure can declare its own error
enum via `#[derive(TautError)]`, and the IR records the variants. The
TS client gets a tagged union per procedure and can narrow on the
`code` discriminant exhaustively. The runtime helpers `isTautError`
and `errorMatch` make the narrowing ergonomic — see
[Errors](../concepts/errors.md).

If you don't care about the per-procedure detail, `StandardError`
ships a small set of common variants (`Unauthenticated`, `NotFound`,
…) that maps closely onto rspc's `ErrorCode` set.

## Step-by-step migration plan

The shape that works for most teams is **one procedure at a time**,
with both stacks running in parallel until the last rspc procedure
flips over. Don't try a big-bang rewrite.

1. **Add `taut-rpc` to the server crate** alongside `rspc`. Both
   can coexist; they're just different `axum::Router` mounts.
2. **Pick a leaf procedure to migrate first.** A read-only query
   with simple input is ideal — small surface, easy to verify.
3. **Convert the input/output types** by changing `specta::Type` to
   `taut_rpc::Type`. Run `cargo check`; resolve any specta-specific
   attribute mismatches against the type mapping reference.
4. **Rewrite the procedure** as `#[rpc] async fn`. Drop the context
   parameter; replace its uses with whatever state pattern you
   landed on (extensions, `OnceCell`, etc.).
5. **Define the per-procedure error enum** with
   `#[derive(TautError)]`. Map each `rspc::Error` you used to a
   variant; pick HTTP statuses with `#[taut(status = ...)]`.
6. **Mount the new procedure** on a `taut_rpc::Router`, then merge
   it into the axum app via `.into_axum()` next to the existing
   rspc router. They serve different paths, so they don't collide.
7. **Run `cargo taut gen`** to produce the TS schemas for the
   migrated procedure. Wire it into your client build alongside the
   existing rspc bindings.
8. **Update one call site** on the client to use `createApi` for
   that procedure. Leave every other call site on rspc. Verify
   the round trip end-to-end against a running server.
9. **Repeat steps 2–8** for each remaining procedure. The pattern
   becomes mechanical after the first few; resist the urge to
   batch them — single-procedure PRs review faster and catch
   regressions earlier.
10. **Remove rspc** once the last procedure is migrated: drop the
    dependency, delete the rspc router, delete the old bindings
    file, and delete the rspc client. The PR that does this should
    contain no behavior changes — only deletions.

## See also

- [Getting started](./getting-started.md) — fresh-project tutorial.
- [Type mapping](../concepts/type-mapping.md) — Rust → TS rules.
- [Errors](../concepts/errors.md) — typed-error narrative on both sides.
- [The IR](../concepts/ir.md) — what `cargo taut gen` consumes.

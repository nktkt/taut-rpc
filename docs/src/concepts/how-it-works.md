# How it works

Phase 5 is a good moment to step back and explain how the moving parts of
`taut-rpc` actually fit together. The earlier concept chapters describe each
seam in isolation — the IR, the wire format, the validation bridge — but a
new reader is usually better served by a single end-to-end walkthrough that
follows one request from a `#[rpc]` annotation to a typed TypeScript call
site and back. That is the goal of this chapter.

For the normative description of each seam, see
[SPEC §2](../reference/spec.md#2-architecture). This page is the
prose tour.

## The three pieces

`taut-rpc` is intentionally not a monolith. The work is split across three
distinct *phases of execution*, each owned by a different crate or tool,
and each producing a different artifact for the next phase to consume.

| Phase | When it runs | Owner | Produces |
|---|---|---|---|
| **Macro** | At Rust compile time | `taut-rpc-macros` | An axum-shaped handler stub + an IR fragment |
| **Runtime** | At request time, in your binary | `taut-rpc` | An HTTP response in the SPEC envelope |
| **Codegen** | At your build time, before bundling | `taut-rpc-cli` (`cargo taut gen`) | One `api.gen.ts` per project |

The arrow between them is the IR — a JSON file at
`target/taut/ir.json` that is *the only thing* the codegen phase reads.
Schematically:

```
   Rust source ──► proc-macro ──► IR fragments ──► target/taut/ir.json
                                                          │
                                                          ▼
                                                   cargo taut gen
                                                          │
                                                          ▼
                                                     api.gen.ts
```

Three phases, three artifacts, three crates. Macros never read the
network. The runtime never reads `syn`. The CLI never links rustc. Each
piece has a job, and they communicate through *files*, not function calls.

## Why an IR?

Decoupling emission from consumption is the central design choice. The
proc-macro half writes IR; downstream tools read IR. The IR is the
contract — and because it is JSON on disk, anything that can parse JSON
can become a downstream tool.

Concretely, today the IR drives:

- **TypeScript codegen** (`cargo taut gen`) — types, schemas, and the
  `createApi` helper.
- **MCP manifest emission** (`cargo taut mcp`) — turning your RPC surface
  into a Model Context Protocol tools manifest.
- **Validation schema emission** — Valibot or Zod, picked at codegen time
  via `--validator`.

Tomorrow, the same IR can drive an OpenAPI emitter, a documentation site,
a devtools panel, or a second-language client (Python, Swift, Kotlin)
without anyone touching the proc-macro crate. That is the payoff for
paying the file-on-disk overhead: every new consumer is additive, not
invasive.

The IR also makes the project *debuggable*. When codegen produces
unexpected TypeScript, you do not have to expand macros or re-run rustc:
you `cat target/taut/ir.json` and see exactly what the macro saw.

## The macro

The proc-macro half is small by design. `#[rpc]` parses your `async fn`,
verifies the supported subset (zero or one input argument, a return type
the macro can lower, optional `stream`/`method` arguments), and emits two
things in the same scope:

1. The **axum handler** — a function with the right signature for
   `axum::routing::post(...)` to accept, that decodes JSON into your
   input type, runs validation, calls your function, and re-wraps the
   result into the SPEC envelope.
2. A **sibling constructor** named `__taut_proc_<name>()` that returns a
   `ProcedureDescriptor` — a small struct with the procedure's name,
   HTTP method, transport (query/mutation/stream), and the handler from
   step 1.

A first-time reader sometimes asks "where does the IR get written?" The
answer is: the macro stamps an IR fragment via a build-script hook, and
`build.rs` collects fragments into the canonical
`target/taut/ir.json`. The macro itself does not touch the filesystem
during expansion — that would be a footgun for incremental builds.

The constructor pattern (`__taut_proc_<name>()`) is what lets you write:

```rust
let app = Router::new()
    .procedure(__taut_proc_ping())
    .procedure(__taut_proc_get_user())
    .into_axum();
```

`#[rpc]` does not register anything globally; it just hands you a
constructor and lets you decide where to mount it.

## The Router

The runtime crate's job is to take a list of `ProcedureDescriptor`s and
turn them into a working `axum::Router`. There are three things it adds
on top of axum:

1. **The wire envelope.** Every successful response is wrapped as
   `{ "ok": ... }`; every error becomes `{ "err": { "code", "payload" } }`
   with the correct HTTP status from `TautError::http_status()`. The
   user's handler returns plain `Result<T, E>`; the wrapping happens at
   the request boundary, not in your code.
2. **Built-in error envelopes.** A `JsonRejection` from axum becomes
   `code = "decode_error"`. An unregistered procedure becomes
   `code = "not_found"`. A `Validate::validate()` failure becomes
   `code = "validation_error"`. Every error a client can see comes
   through the same envelope shape, so clients have one parser path.
3. **Layer composition.** `Router::layer<L>(layer)` accepts the same
   `tower::Layer<axum::routing::Route>` bound that `axum::Router::layer`
   does, and composes in onion order. Auth, tracing, CORS — none of
   these are reinvented; you reach for the standard tower ecosystem and
   the runtime stays out of the way.

The `Router` is the only piece of `taut-rpc` that has to *know* about
axum. The macros emit axum-shaped handlers, but the IR is
transport-agnostic; in principle, a different runtime crate could drive
a non-axum backend off the same proc-macro output.

## Codegen

`cargo taut gen` is the build-time half. It reads
`target/taut/ir.json`, refuses any IR with a version it doesn't
recognise, and emits one TypeScript file — by default
`src/api.gen.ts` — containing:

- **Type aliases** for every Rust type reachable from a `#[rpc]`
  function (structs become interfaces, enums become discriminated
  unions, primitives lower per [Type mapping](./type-mapping.md)).
- **Validation schemas** — `<Type>Schema` constants and a
  `procedureSchemas` record — emitted as Valibot (default), Zod
  (`--validator zod`), or omitted entirely (`--validator none`).
- **A `Procedures` type** mapping each procedure name to its
  `{ input, output, error }` shape.
- **A `createApi` helper** that, given the runtime client and the
  schemas, returns a typed object with one method per procedure.

The output is **pure types and string constants**. There are no runtime
imports of your Rust crate, no schema-fetch round-trip, and no
build-time dependency on `taut-rpc-macros`. The `.gen.ts` file lives
alongside your source, is checked in, and is the only artifact your
TypeScript compiler sees from `taut-rpc`.

CI uses the same machinery in reverse: `cargo taut check` (Phase 5)
re-runs codegen against the current IR and diffs the result against the
checked-in `.gen.ts`. A drift means someone forgot to commit a
regenerated client.

## Type flow: one request, end to end

To make this concrete, follow a single request through the system. The
example procedure:

```rust
#[derive(serde::Deserialize, taut_rpc::Type, taut_rpc::Validate)]
struct CreateUserInput {
    #[taut(length(min = 3, max = 32))]
    username: String,
}

#[rpc]
async fn create_user(input: CreateUserInput) -> Result<u64, StandardError> {
    /* ... */
}
```

The data flows like this:

1. **Rust struct.** `CreateUserInput` is a normal serde-derived struct
   with one validated field.
2. **IR Field.** At compile time, `#[derive(Type)]` emits a type
   descriptor with one `Field { name: "username", ty: String,
   constraints: [Length { min: 3, max: 32 }] }`. `#[derive(Validate)]`
   emits the matching `impl Validate`. `#[rpc]` records a procedure
   descriptor pointing at the input type.
3. **TS interface.** `cargo taut gen` reads the descriptor and emits
   `interface CreateUserInput { username: string }` plus a
   `CreateUserInputSchema` Valibot constant carrying the length check.
4. **Wire JSON.** The TS client validates the input against
   `CreateUserInputSchema` (pre-send), then `POST /rpc/create_user`
   with body `{ "input": { "username": "ada" } }`.
5. **Server validation.** axum decodes the JSON into `CreateUserInput`;
   the generated handler calls `Validate::validate(&input)?` *before*
   your function body runs. A failure here returns the
   `validation_error` envelope with HTTP 400, and your function never
   executes.
6. **Handler call.** On success, your `async fn create_user` runs with a
   value that has already been shape-checked and constraint-checked.
7. **Response.** Your `Result<u64, StandardError>` is wrapped into
   `{ "ok": 42 }` and sent back. The TS client (with schemas attached)
   optionally re-validates the output, then yields the typed value to
   the awaiting caller.

At every step, the same Rust struct definition is the source of truth.
Nothing along the chain has its own private notion of what a
`CreateUserInput` is.

## Where state lives (deferred)

A natural follow-up question is: how does a procedure reach a database
handle, or a logger, or a configuration object? In v0.1, the answer is
deliberately minimal. axum's `State<S>` extractor is **not** supported
on `#[rpc]` functions; procedures are free `async fn`s, and shared
state must be reached through `OnceCell`, a `static`, or a closure
captured in a `tower::Layer`.

This is a known gap, scheduled for **Phase 6 or later**. The Phase 2
middleware story (`Router::layer`, plus the standard tower ecosystem)
covers most operational needs in the meantime: auth and tracing
compose as layers, and per-request data can ride in axum's request
extensions. Full `State<S>` ergonomics — with the type machinery to
keep the codegen surface clean — is a design exercise we have chosen
not to rush.

## See also

- [SPEC §2 — Architecture](../reference/spec.md#2-architecture)
- [Architecture](./architecture.md)
- [The IR](./ir.md)
- [Wire format](./wire-format.md)
- [Validation](./validation.md)
- [Roadmap — Phase 5](../reference/roadmap.md)

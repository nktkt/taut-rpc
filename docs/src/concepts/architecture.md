# Architecture

This page is the orientation map for `taut-rpc`. It covers the three time
domains the project lives in, the artefacts that flow between them, and the
deliberate choices behind what `taut-rpc` is *not*. The canonical normative
description lives in [SPEC §2](../reference/spec.md); this chapter exists to
give you the shape of the system before you read the spec.

## The three-layer split

`taut-rpc` is best understood as three execution domains that never run at
the same time:

| Layer | When | Who runs it | Output |
|---|---|---|---|
| Macro time | `cargo build` | `taut-rpc-macros` proc-macros | axum handlers + IR fragments |
| Codegen time | `cargo taut gen` (build script or CI) | `taut-rpc-cli` | `api.gen.ts` |
| Runtime | request time | `taut-rpc::Router` mounted in axum | HTTP responses |

The split is deliberate. Macro time has access to Rust types but not to
the network or the user's TypeScript project. Runtime has access to the
network but not to Rust types as Rust types — they have already been
compiled away. Codegen time has access to neither; it only sees the IR.

Keeping the three apart means each layer fails loudly. A macro bug
shows up at `cargo build`; a codegen bug shows up at `cargo taut gen`; a
runtime bug shows up under load. There is no single "RPC framework" object
that links all three at once, and that absence is the point.

## The IR as the contract

The intermediate representation — `target/taut/ir.json` — is the seam
between macro time and codegen time, and it is the single source of truth
for what the client sees. Macros write fragments of IR; the CLI reads the
merged IR; the generated TypeScript is a pure function of that JSON.

Three properties make the IR load-bearing:

1. **It is plain JSON.** No proc-macro re-execution, no `cargo expand`, no
   linkage against rustc internals. `cargo taut gen` is a JSON-to-string
   transform that can run anywhere Node or Rust runs.
2. **It is schema-versioned.** The top-level `ir_version` field gates
   compatibility. If a new codegen reads an IR it doesn't understand, it
   refuses with both versions named — see [SPEC §9.1](../reference/spec.md).
3. **It is the most stable surface in the project.** Once 0.1 ships, the
   IR shape is harder to change than the public Rust API, because every
   generated client on disk depends on it.

That last property is why `cargo taut check` exists. Pinning `ir.json`
into a repository and running `check` in CI catches accidental API drift
before it reaches a generated TypeScript client. A renamed Rust field, a
re-ordered enum variant, a new procedure: all show up as a diff in the
checked-in IR, and the CI step fails before the client lies about it.

## The crate layout

`taut-rpc` ships as three Cargo crates plus one npm package, all
coordinated through [SPEC.md](../reference/spec.md):

| Artefact | Role |
|---|---|
| `taut-rpc` (crate) | Public API: `Router`, runtime wiring, re-exports. |
| `taut-rpc-macros` (crate) | `#[rpc]`, `#[derive(Type)]`, `#[derive(Validate)]`. |
| `taut-rpc-cli` (crate) | `cargo taut` with `gen`, `check`, `inspect`. |
| `taut-rpc` (npm) | Tiny runtime client: fetch + parse + decode. |

The top-level `taut-rpc` crate is the only one a server author touches
directly. It re-exports the macros so users add one dependency, not three.
The CLI crate is a separate binary because most CI pipelines do not want a
full proc-macro toolchain just to run codegen. The npm package is hand-
written, not generated, and stays small enough that a generated client
stays readable.

SPEC.md is the contract between these four pieces. Any change that crosses
a crate boundary — IR shape, wire format, error envelope — has to land in
SPEC.md first; the crates implement it second.

## Runtime structure

At runtime, the data flow is short:

```
HTTP request
    │
    ▼
taut_rpc::Router  ─── matches path → procedure
    │
    ▼
procedure handler  ─── deserialize args, run, serialize result
    │
    ▼
axum::Router       ─── tower::Layer stack wraps each procedure
    │
    ▼
HTTP response
```

`taut_rpc::Router` is a thin builder over `axum::Router`. Each registered
procedure becomes a route; each route is wrapped by whatever
`tower::Layer` stack the user mounted. The framework adds nothing to the
hot path that axum does not already provide — tracing, auth, CORS, and
rate limiting are layers, not built-ins.

This means a `taut-rpc` server *is* an axum server. You can mount
non-RPC routes alongside it, share state through `axum::extract::State`,
and run it under any axum-compatible runtime (tokio, hyper, lambda,
wasm).

## Codegen structure

Codegen is even shorter:

```
ir.json  ─→  render_ts  ─→  api.gen.ts
                 │
                 └── validator backend (Valibot | Zod | none)
```

`render_ts` is a pure function: same IR in, same TypeScript out. The only
configurable axis is the validator backend. By default, a v0.1 codegen
emits Valibot schemas because they tree-shake to roughly half the bundle
size of Zod; passing `--validator zod` swaps in a Zod renderer. The
validator backend is a trait, so a future `--validator none` (decode-only)
or `--validator effect` is a contained change, not a rewrite.

The IR carries enough to render type aliases, runtime validators, and
fully-typed RPC method signatures. There is no `.d.ts` file separate from
the implementation; both come from the same render.

## Architecture diagram

```
                      ┌───────────────────────┐
   #[rpc] fn / trait ──→  proc-macro emits     │
                      │  - axum handler        │
                      │  - IR entry (JSON)     │
                      └──────────┬─────────────┘
                                 │  build script writes
                                 │  target/taut/ir.json
                                 ▼
                      ┌───────────────────────┐
                      │  cargo taut gen       │  reads IR, emits .ts
                      └──────────┬─────────────┘
                                 │
                                 ▼
                      ┌───────────────────────┐
                      │  api.gen.ts           │  imported by app code,
                      │  + Valibot/Zod        │  calls runtime client
                      └───────────────────────┘
```

This is the SPEC §2 diagram, which has not changed since Phase 0.

## Observability

Observability is delegated, not invented. On the server, anything that
implements `tower::Layer` works: `tower-http::trace::TraceLayer` for
request spans, `tracing` for structured logs, `metrics` for counters.
The `Router` exposes the layered axum router, so the user mounts whatever
stack they want.

On the client, `ClientOptions` carries optional hooks: `onRequest`,
`onResponse`, `onError`. These are intentionally minimal — enough to wire
up a logger or a span propagator, not enough to become a plugin system.
A client that wants OpenTelemetry passes a header-injecting `fetch`; the
runtime does not ship its own tracer.

## Why not GraphQL?

GraphQL is schema-first: the schema is the contract, and Rust types
follow. `taut-rpc` is Rust-first: Rust types are the contract, and the
schema (the IR) follows. This is a deliberate non-goal, not a missing
feature.

The Rust-first stance trades GraphQL's query flexibility for two
properties: the server author cannot accidentally expose a type that does
not round-trip, and the client never asks for a field the server cannot
produce. There is no resolver layer to keep in sync with the type system,
and no N+1 problem to design around, because procedures are explicit RPCs
rather than fields on a graph.

If your problem is "many clients want different views of the same data,"
GraphQL is the right tool and `taut-rpc` is not. If your problem is "one
or two clients want exactly the data the server has, with no drift,"
`taut-rpc` is the right tool.

## Why not gRPC?

gRPC's wire format is protobuf: dense, binary, and optimised for service
mesh traffic. `taut-rpc` uses JSON over HTTP/1.1 (with HTTP/2 fallout
free): less dense, but readable in a browser DevTools panel, debuggable
with `curl`, and serializable by every TypeScript runtime without a
codegen step on the wire format itself.

This is also a deliberate non-goal. The target deployment is a web app
talking to its own backend — a domain where bundle size, debuggability,
and edge-runtime compatibility matter more than bytes-on-the-wire. A
service mesh between Rust microservices is a different domain and gRPC
serves it well.

## Compatibility

[SPEC §9](../reference/spec.md) is normative; the short version:

- The `ir_version` field gates IR compatibility. Codegen refuses
  mismatches with a clear error naming both versions.
- The wire format carries a `v` field on subscription frames; missing
  means v0.
- The `taut-rpc` crate and the npm runtime are versioned together; the
  npm major tracks the crate major.
- `cargo taut check` detects IR drift in CI by diffing the freshly-built
  IR against a checked-in copy.

Backwards-incompatible IR changes are rare and require simultaneous semver
bumps across crate and npm package. Backwards-compatible additions
(new field, default-able) bump the minor and the IR version where the
on-disk shape changes; runtime feature additions that do not change
JSON shape do not bump the IR version. See
[SPEC §9.1](../reference/spec.md) for the full IR_VERSION transition
table.

## See also

- [SPEC §2 — Architecture](../reference/spec.md)
- [SPEC §9 — Compatibility & versioning](../reference/spec.md)
- [The IR](./ir.md)
- [Wire format](./wire-format.md)

# FAQ

A collection of common questions about the project. If your question is not
answered here, check the [SPEC](./spec.md), [Constraints](./constraints.md), or
[Roadmap](./roadmap.md) — and otherwise, please open a discussion on the issue
tracker.

---

## Why Rust + TypeScript? Why not Rust + Rust client?

The boundary this project addresses is the one between a Rust HTTP server and a
TypeScript SPA — the most common shape for product teams today. A Rust-to-Rust
RPC layer is a different problem (different runtime, different serialization
trade-offs, different deployment shape) and is solved well by tools like
[tarpc](https://github.com/google/tarpc). Conflating the two would compromise
both. If you need Rust-to-Rust, you almost certainly want a different tool.

---

## Can I use it with frameworks other than axum?

Not in v0.1. The handler-extraction macros emit code that depends on axum's
extractor traits and `Router` API. Pluggable adapters for `actix-web`, `poem`,
and `rocket` are tracked under **Phase 6** of the [roadmap](./roadmap.md). If
you need this sooner, the IR is framework-agnostic and you can write a custom
emitter today — see [SPEC §9](./spec.md#9-ir-schema).

---

## Does it support GraphQL?

No, by design. GraphQL solves a different problem (declarative client-driven
selection over a graph of related entities) and brings its own runtime
complexity (resolvers, dataloaders, query depth limits, persisted queries). The
RPC model here is intentionally simpler: typed function calls over HTTP. If you
need GraphQL, use [`async-graphql`](https://github.com/async-graphql/async-graphql).

---

## Is it production-ready?

v0.1 is **pre-1.0**. The wire format and IR schema are stable enough to commit
to (both are versioned — see [SPEC §4](./spec.md#4-wire-format) and
[§9](./spec.md#9-ir-schema)), but the Rust API surface will see breaking
changes before 1.0 as the ergonomics settle. Battle-tested example
deployments — including a hosted reference SaaS — land in **Phase 6**. Adopters
running it in production today should pin exact versions and read the
[CHANGELOG](./changelog.md) before each upgrade.

---

## Performance vs tRPC?

Faster, in the typical case. tRPC handlers run inside Node's single-threaded
event loop and incur a JS-side serialization hop per call. Here, each handler
is a native Rust function dispatched directly from axum's router — no event
loop hop, no V8, and `serde_json` deserialization runs at native speed. For
CPU-bound handlers the gap is large; for I/O-bound handlers (where you're
waiting on a database) the gap shrinks but Rust still wins on tail latency.
Concrete benchmarks land alongside the Phase 6 examples.

---

## What about HTTP/3?

axum supports HTTP/3 transitively through hyper's optional `http3` feature, so
in principle you can serve this stack over QUIC today. It is **not** officially
tested in CI, and the wire format makes no assumptions about the underlying
transport, so anything that works for axum should work here. Treat it as
experimental until Phase 6.

---

## Is the wire format pinnable?

Yes. The wire format carries an explicit `wireVersion` field and a stable
content-type (`application/x.rpc+json; v=1`). Servers reject requests with an
unrecognized version; clients reject responses with one. See
[SPEC §4](./spec.md#4-wire-format) for the full negotiation rules. You can
safely pin a server and client to the same major version and upgrade
independently within that major.

---

## What about gRPC?

Use [`tonic`](https://github.com/hyperium/tonic). gRPC is a non-goal for this
project: it targets a different audience (service-to-service, polyglot
backends) with a different transport (HTTP/2 framing, protobuf) and a
different code-gen story. Trying to be both an RPC framework for browser
clients *and* a gRPC peer would dilute both. The IR is expressive enough that
a third-party `tonic` emitter could be written, but none ships in-tree.

---

## How big is the binary?

About **5 MB stripped** for the minimal example (one handler, axum, tokio,
serde, no TLS). Adding TLS via `rustls` brings it to roughly 8 MB; adding a
database driver typically brings it to 12–15 MB. These numbers are for
`cargo build --release` with `strip = true` and `lto = "thin"` in
`Cargo.toml`. Linking against the system OpenSSL instead of `rustls` shaves
another ~1.5 MB at the cost of a runtime dependency.

---

## Can I serve a SPA from the same binary?

Yes. The recommended pattern is to embed the built SPA assets into the binary
(e.g. with [`rust-embed`](https://crates.io/crates/rust-embed)) or to serve
them from disk via
[`tower_http::services::ServeDir`](https://docs.rs/tower-http/latest/tower_http/services/struct.ServeDir.html).
Mount it as a fallback service on the axum `Router` after your RPC routes. See
the deployment guide for a complete example, including SPA history-mode
fallback to `index.html`.

---

## Is the IR schema versioned?

Yes. Every IR document carries an `IR_VERSION` field, and the CLI refuses to
consume an IR produced by a newer toolchain than itself. See
[SPEC §9](./spec.md#9-ir-schema) for the schema, the version-bumping policy,
and the compatibility matrix. The IR is the project's stable contract — wire
format and code-gen output may evolve, but the IR is the pinning point.

---

## Can I generate JSON Schema from the IR?

Yes. Run `cargo taut mcp` to emit a JSON Schema document describing every
type referenced by the IR, suitable for use with editors, validators, or
tooling like [Model Context Protocol](https://modelcontextprotocol.io/)
servers. The mapping from Rust types to JSON Schema is documented in the
SPEC; round-tripping IR → JSON Schema → IR is supported and tested.

---

## Why no `State<S>` extractor in v0.1?

Threading a typed application state through the macro-generated handlers
introduces a generic parameter that ripples through the IR, the codegen, and
the client surface. The design is sound but the ergonomics are not yet
settled, so it's deferred to **Phase 6+**. For now, use the standard
workarounds: `OnceLock` / `LazyLock` globals for read-mostly state, or
`tokio::task_local!` for per-request scoped state. Both compose cleanly with
the current handler signatures.

---

## Do subscriptions support backpressure?

**Limited.** The current subscription transport is a server-sent-event stream
with a per-connection bounded channel; when the client falls behind, the
server drops the oldest messages and signals a `lag` event so the client can
resync. True end-to-end backpressure (where a slow consumer slows the
producer) requires WebSocket flow control or HTTP/2 stream-level windows,
neither of which the current transport exposes. See
[SPEC §4.2](./spec.md#42-subscriptions) for the exact semantics. WebSocket
support with proper backpressure is on the Phase 6 roadmap.

---

## Can I write the IR by hand?

Yes — the IR is just JSON, and there is nothing magical about how the macros
produce it. Hand-writing an IR is a reasonable path if you want to drive
codegen for an existing service that wasn't built with this toolchain, or if
you want to experiment with a custom emitter. The schema is documented in
[SPEC §9](./spec.md#9-ir-schema), and `cargo taut validate <file.json>` will
check a hand-written IR against it before emission.

# Comparison with alternatives

This page is a side-by-side look at where `taut-rpc` sits among the typed-RPC
tools you might reach for. The goal is not to crown a winner — each of these
projects is a reasonable choice under the right constraints — but to make the
trade-offs explicit so you can pick deliberately. Where we have a strong
opinion, we say so; where the answer is "it depends," we try to spell out what
it depends on.

## At a glance

| Feature | taut-rpc | rspc | taurpc | tRPC | gRPC + tonic |
|---|---|---|---|---|---|
| Languages | Rust ↔ TS | Rust ↔ TS | Rust ↔ TS (Tauri) | Node ↔ TS | Polyglot |
| Validation | Yes (Rust) | Partial | Partial | Yes (Zod) | Schema-first |
| Subscriptions | Yes (SSE/WS) | Yes (WS) | Yes (Tauri events) | Yes (RxJS) | Yes (streaming) |
| Codegen | Static `.ts` | Runtime + static | Static | Inferred | `.proto` |
| Wire format | JSON | JSON | Tauri IPC | JSON | Protobuf |
| Server framework | axum | axum / many | Tauri only | Express/Fastify/etc | tonic |
| Status | v0.1 (2026) | active | active | mature | mature |

## Row-by-row, in prose

### Languages

`taut-rpc`, `rspc`, and `taurpc` all bridge a Rust server to a TypeScript
client. `tRPC` is Node-only on the server: if you want to keep the same
language end-to-end, it remains the default for a reason. `gRPC` is the
polyglot answer — anything that can speak HTTP/2 and parse Protobuf can
participate, at the cost of giving up the "just one language pair" ergonomics.

### Validation

`taut-rpc` treats validation as a first-class concept: the same Rust types
that define your handler signatures carry validation metadata, and that
metadata flows through the IR into the generated TypeScript so the client
knows the rules before a request goes out. `tRPC` does the equivalent with
Zod on the Node side. `rspc` and `taurpc` support validation, but it tends to
be bolted on rather than woven through the type pipeline. `gRPC` defers to
schema-level constraints (e.g. `protoc-gen-validate`) which work but live
outside the language type system.

### Subscriptions

All five support some form of server-push. The differences are in transport:
`taut-rpc` uses SSE for one-way streams and WebSockets when bidirectionality
is required, both layered over plain HTTP. `rspc` standardizes on
WebSockets. `taurpc` uses the Tauri IPC bridge, which is great inside a Tauri
app and irrelevant outside one. `tRPC` exposes subscriptions as observables
(historically RxJS-flavored). `gRPC` has streaming RPCs built into the
protocol.

### Codegen

`taut-rpc` emits a static `.ts` file from the IR — no runtime reflection on
the client, and the file is deterministic so it diffs cleanly in PRs. `rspc`
historically supported both runtime type lookups and static export. `taurpc`
generates static bindings. `tRPC` skips codegen entirely and relies on
TypeScript type inference across the import boundary, which is elegant when
both halves live in the same monorepo and awkward when they don't. `gRPC`
generates from `.proto` files in every supported language.

### Wire format

`taut-rpc`, `rspc`, and `tRPC` all use JSON, which is unbeatable for
debuggability (you can curl a request and read the response). `taurpc` rides
the Tauri IPC, which is JSON-shaped but in-process. `gRPC` uses Protobuf —
faster and more compact, opaque to casual inspection.

### Server framework

`taut-rpc` v0.1 is axum-only — that's a deliberate scoping decision, not a
permanent constraint (see the [FAQ](./faq.md) and [roadmap](./roadmap.md)).
`rspc` ships adapters for several frameworks. `taurpc` only makes sense
inside a Tauri app. `tRPC` integrates with the entire Node web ecosystem.
`gRPC` runs on `tonic` in the Rust world.

### Status

`taut-rpc` is at v0.1 in 2026 — usable but young. `rspc`, `taurpc`, `tRPC`,
and `gRPC` are all actively maintained, with `tRPC` and `gRPC` being mature
in the "boring technology" sense.

## When to choose what

### Choose `taut-rpc` when…

You want refactor safety with Rust on the server and TypeScript on the
client, you don't want a JSON Schema generation step in your build pipeline,
and you treat validation as a first-class concern that should live with your
domain types rather than as a separate runtime layer. The sweet spot is a
new-ish axum service paired with a TypeScript SPA where you'd like changes
to a Rust handler signature to break the TS build immediately.

### Choose `rspc` when…

You're already deep in [Specta](https://github.com/oscartbeaumont/specta) and
you need its richer Rust type support — for example, `HashMap` keyed by
something other than `String`, or other type-system corners that `taut-rpc`
deliberately doesn't model in v0.1. `rspc` also has a head start on
multi-framework adapters if you can't or won't use axum.

### Choose `taurpc` when…

You're building a Tauri desktop app and the RPC layer doesn't need to leave
the process. `taurpc` is purpose-built for that case and its Tauri-event
subscription model is the right fit there.

### Choose `tRPC` when…

You want a Node server, full stop. `tRPC`'s ergonomics are excellent and the
ecosystem around it is large. There's no reason to pick a Rust-based
alternative if you don't actually want a Rust server.

### Choose `gRPC` when…

You need polyglot — services in three languages talking to each other — or
you're hitting wire-performance ceilings where JSON's overhead matters.
`gRPC` is also the right call when your organization has standardized on
Protobuf as the system-of-record for service contracts.

## What `taut-rpc` deliberately does not have

A few things are out of scope by design, not oversight. Calling them out so
you can rule us out faster:

- **No GraphQL.** Different problem domain; if you need declarative
  client-driven selection, use [`async-graphql`](https://github.com/async-graphql/async-graphql).
  See the [FAQ](./faq.md) for the long version.
- **No schema-first workflows.** The Rust types are the source of truth and
  the IR / TypeScript fall out of them. There is no `.proto`-equivalent to
  hand-author and there are no plans to add one.
- **No built-in caching.** Request-level caching, query invalidation, and
  optimistic updates are the client's job. Pair the generated client with
  [TanStack Query](https://tanstack.com/query), SWR, or whatever your app
  already uses; we don't want to be in that business.

If any of those is a hard requirement, one of the alternatives above is a
better fit than `taut-rpc`.

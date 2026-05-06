# taut-rpc

TypeScript runtime for [taut-rpc](https://github.com/nktkt/taut-rpc) — end-to-end
type-safe RPC between Rust servers and TypeScript clients. Rust types own the
contract; this package is the small client-side runtime that the generated
`api.gen.ts` plugs into. See the [main repository](https://github.com/nktkt/taut-rpc)
for the full design, the `#[rpc]` macro, and the `cargo taut` tooling.

> This package is the **runtime only**. The per-project `api.gen.ts` (containing
> procedure names, input/output types, and optional Valibot/Zod schemas) is
> produced by `cargo taut gen` from your Rust crate. There is no
> `/schema.json` round-trip on boot — the generated file is static `.ts`.

## Install

```sh
npm install taut-rpc
# Optional, only if your generated client uses runtime validation:
npm install valibot   # or: npm install zod
```

Requires Node.js 20+ (or any modern browser / Deno / Bun runtime with `fetch`
and `EventSource`).

## Quick example

```ts
import { createClient } from "taut-rpc/client";
import type { Procedures } from "./api.gen";

const client = createClient<Procedures>({
  url: "/rpc",
  // transport defaults to fetch + EventSource; override for tests, auth, etc.
  // transport: customTransport,
});

// Query / mutation
const pong = await client.ping();

// Subscription (Server-Sent Events)
for await (const evt of client.userEvents.subscribe({ userId: 1 })) {
  console.log(evt);
}
```

The shape of `client` is derived entirely from the `Procedures` type in
`api.gen.ts`. Renaming or removing a `#[rpc]` function in Rust regenerates the
file and surfaces as a TypeScript error at the call site.

## Subpath exports

| Import | What it gives you |
|---|---|
| `taut-rpc` / `taut-rpc/client` | `createClient`, core types |
| `taut-rpc/http` | HTTP transport (POST/GET to `/rpc/<procedure>`) |
| `taut-rpc/sse` | Server-Sent Events transport for subscriptions |

## Status

**0.0.0 — Day 0.** Public API is unstable until 0.1.0. The wire format is
documented in [SPEC.md §4](https://github.com/nktkt/taut-rpc/blob/main/SPEC.md)
and gated by an `ir_version` field; mismatches are refused by codegen.

## Changelog

## 0.0.0 — Phase 3

- Subscriptions: `client.<name>.subscribe(input)` returns `AsyncIterable<T>`
  for procedures generated as `kind: "subscription"`. Backed by the SSE
  transport when no custom transport is configured.
- SSE transport gained an AbortController, so breaking out of `for await`
  cancels the underlying fetch and stops the stream cleanly.

## 0.0.0 — Phase 2

- `assertTautError` and `errorMatch` helpers for narrower catch ergonomics.
- `isTautError` gained payload-narrowing overloads.

- **Phase 1** — `createApi`-friendly surface for codegen-emitted `api.gen.ts`:
  re-exports `TautError` from the package root, adds the `isTautError(err,
  code?)` type-guard for typed `catch` handlers, and accepts an optional
  `kinds` map on `ClientOptions` so per-procedure `query` / `mutation` /
  `subscription` tags reach the transport (forwarded as `x-taut-kind`).
  No breaking changes to existing exports.

## License

MIT OR Apache-2.0

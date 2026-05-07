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
  // Pass `schemas: procedureSchemas` (from api.gen.ts) to enable runtime validation.
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

## Disabling validation per call

The Phase 4 runtime exposes a per-CLIENT toggle (`createClient({ ..., validate: { send: false } })`)
but not a per-call override. The recommended pattern is to instantiate two clients:

```ts
const userClient = createClient<Procedures>({ url, schemas: procedureSchemas });           // strict
const internalClient = createClient<Procedures>({ url, schemas: procedureSchemas, validate: { send: false } });

await userClient.create_user(form);                  // validates
await internalClient.create_user(trusted_payload);   // skips schema.parse on input
```

Both clients share the underlying transport contract — no extra connection or
memory cost. Use the strict client for anything derived from user input
(forms, query strings, third-party webhooks); reach for the unvalidated client
only on internally-trusted code paths where the input shape is already known
to match the procedure's declared type. A per-call `validate` override is a
v0.2 consideration; for now, the two-client split keeps the trust boundary
visible at the call site.

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

## 0.0.0 — Phase 4

- `ClientOptions.schemas` accepts a `procedureSchemas` map emitted by codegen
  (Valibot or Zod). When set, the runtime parses inputs before sending and
  outputs after receiving.
- `ClientOptions.validate: { send, recv }` toggles per-client (defaults to
  `{ send: true, recv: true }`).
- A failing parse throws `TautError("validation_error", { errors: [...] }, 0)`,
  reusing the same envelope shape the server emits on its own `Validate`
  rejection.
- For subscriptions, output validation runs on each yielded frame.
- New `SchemaLike` interface — duck-type for anything with a `parse(value)` method,
  so user-supplied custom validators slot into `procedureSchemas` cleanly.

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

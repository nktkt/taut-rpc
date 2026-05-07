# taut-rpc examples

This directory is the canonical catalog of runnable taut-rpc examples. Each
subdirectory is a self-contained project that lives **outside** the cargo
workspace (`exclude = ["examples"]` in the root `Cargo.toml`) so it resolves
its own dependencies through path entries and never piggy-backs on workspace
machinery that doesn't yet exist.

The examples fall into two groups:

- **Reference examples** map one-to-one onto a roadmap phase and exist to
  prove a single piece of the spec end-to-end. They use minimal CLI clients
  (Node + `tsx`), keep their surface area narrow, and are the place to look
  when you want to see exactly how one feature wires up.
- **Full-stack apps** glue several phases together inside a real frontend
  toolchain (Vite/SvelteKit) to show that "the contract is the build artifact"
  survives all the way into a normal SPA bundler. They are the place to look
  when you want to see what user code actually feels like.

Every example binds a different port so they can all run side-by-side. CORS
is permissive throughout — these are local-only demonstrations, not
deployment templates. None of them have auth (except `phase2-auth`, which is
the auth example) and none of them are wired into a test harness yet.

## Reference examples

| Example | Demonstrates | Port | Run |
| --- | --- | --- | --- |
| [`smoke`](./smoke) | Phase 0: hand-written wire format reference (no macros, no codegen) — `POST /rpc/<proc>` envelopes plus an SSE subscription, all by hand, to prove SPEC §4 is implementable. | 7700 | `cd examples/smoke/server && cargo run` (then `cd ../client && npm install && npm run start` in another terminal) |
| [`phase1`](./phase1) | Phase 1: `#[rpc]` + `#[derive(Type)]` + `cargo taut gen` end-to-end — typed queries with macro-generated handlers and a typed `api.gen.ts` consumer. | 7701 | `cd examples/phase1/server && cargo run` (after `cargo taut gen --from-binary ...`; see the example's README for codegen) |
| [`phase2-auth`](./phase2-auth) | Phase 2: bearer-token auth via `axum::middleware::from_fn` plus `#[derive(TautError)]` for typed application errors — middleware short-circuit and procedure-level error share one wire shape. | 7702 | `cd examples/phase2-auth/server && cargo run` |
| [`phase2-tracing`](./phase2-tracing) | Phase 2: `tower-http::TraceLayer` layered over a taut-rpc `Router` — proves taut-rpc is axum-native, not axum-locked, by piping spans through `tracing-subscriber` with no taut-specific glue. | 7703 | `cd examples/phase2-tracing/server && RUST_LOG=info,tower_http=debug cargo run` (no TS client; exercise with `curl`) |
| [`phase3-counter`](./phase3-counter) | Phase 3: `#[rpc(stream)]` + SSE subscription — a tick counter consumed from TypeScript with `for await`. Validates the streaming exit criterion straight from the roadmap. | 7704 | `cd examples/phase3-counter/server && cargo run` |
| [`phase4-validate`](./phase4-validate) | Phase 4: `#[derive(Validate)]` + Valibot bridge — every v0.1 constraint (`length`, `email`, `min`, `max`, `pattern`, `url`) flows from Rust attributes through the IR into a generated Valibot schema, with client-side rejection before the network call. | 7705 | `cd examples/phase4-validate/server && cargo run` |

The reference clients all live at `examples/<name>/client` and are driven
with `npm install && npm run start`. The codegen step
(`cargo taut gen --from-binary <path>`) is the same across all of them and
is documented per-example.

## Full-stack apps

| Example | Demonstrates | Port | Run |
| --- | --- | --- | --- |
| [`todo-react`](./todo-react) | Phase 5: full-stack Todo app — axum server with broadcast-channel-backed subscription, Vite + React 18 frontend that imports `createApi` and `procedureSchemas` directly from `api.gen.ts`. Two browser tabs stay in sync via the `todos_changed` subscription. | 7710 (server), 7711 (Vite) | `cd examples/todo-react/server && cargo run` (then `cd ../client && npm install && npm run dev` in another terminal) |
| [`counter-sveltekit`](./counter-sveltekit) | Phase 5: full-stack counter app — axum server plus a SvelteKit frontend that drives unary mutations and an SSE subscription off `api.gen.ts`. The Svelte counterpart to `todo-react`, proving codegen output is bundler-agnostic. | 7712 (server) | `cd examples/counter-sveltekit/server && cargo run` (then `cd ../client && npm install && npm run dev` in another terminal) |

Both full-stack apps use Vite-style proxying (`/rpc` → the Rust server) so the
frontend code uses a relative `url: "/rpc"` and runs unchanged behind any
reverse proxy in production. State lives in process memory and vanishes on
restart — the point is the wire contract, not persistence.

## One-time setup

Before running any example that has a TypeScript client, build the npm
runtime once per checkout:

```sh
cd npm/taut-rpc
npm install
npm run build
```

The reference clients and the full-stack apps both depend on this through a
`file:` path, so it must exist on disk before `npm install` can resolve.

## Port map

```
7700  smoke (Phase 0)
7701  phase1
7702  phase2-auth
7703  phase2-tracing
7704  phase3-counter
7705  phase4-validate
7710  todo-react server   (7711 = Vite dev)
7712  counter-sveltekit server
```

If you want to run several examples at once to compare them, the ports are
deliberately disjoint and CORS is permissive everywhere.

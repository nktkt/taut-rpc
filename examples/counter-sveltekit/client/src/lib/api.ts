// Thin re-export wrapper around the generated client.
//
// The generated `api.gen.ts` (gitignored, produced by `cargo taut gen`) is the
// real artefact. This module exists so application code never imports the
// generated file directly: instead it imports `$lib/api`, which means a
// regenerate that adds or renames procedures only updates one bind site
// per consumer instead of every call site.
//
// The Vite proxy (see `vite.config.ts`) forwards `/rpc/*` to
// `http://127.0.0.1:7712`, so the runtime URL is just `"/rpc"` here. That
// keeps the build agnostic to where the Rust server actually lives — switch
// the proxy target and nothing else changes.

import { createApi, procedureSchemas } from "./api.gen";

/**
 * The typed RPC client. Procedure names map to functions on this object:
 *
 *   await api.current()
 *   await api.increment({ by: 5n })
 *   await api.reset()
 *   for await (const v of api.live.subscribe()) { ... }
 *
 * Static safety comes from `Procedures` in the generated file. The runtime
 * is duck-typed; the Proxy in `taut-rpc` turns property accesses into
 * fetches against `/rpc/<name>`.
 */
export const api = createApi({
  url: "/rpc",
  schemas: procedureSchemas,
  validate: { send: true, recv: true },
});

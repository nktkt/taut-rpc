/**
 * @packageDocumentation
 *
 * Public TypeScript runtime surface for `taut-rpc`.
 *
 * The generated per-project `api.gen.ts` produces a `Procedures` type-map
 * (a record of {@link ProcedureDef}) and exports the procedure-name strings.
 * This module turns that type-map into a typed client via {@link createClient},
 * a `Proxy`-based facade that turns property accesses into RPC calls.
 *
 * The runtime itself is duck-typed; static safety is supplied by the
 * generated type-map. Wire format and Client API are described in
 * {@link ../../../SPEC.md} sections 4 and 6.
 *
 * Quick reference (SPEC §4):
 *   POST /rpc/<procedure>            { input } → { ok } | { err }
 *   GET  /rpc/<procedure>?input=...  for explicit GET-marked procedures
 *   GET  /rpc/<procedure>?input=...  Accept: text/event-stream  (subscriptions)
 *
 * Quick reference (SPEC §6):
 *   const client = createClient<Procedures>({ url: "/rpc" });
 *   await client.ping();
 *   for await (const evt of client.userEvents.subscribe({ userId: 1 })) {}
 */

// ---------------------------------------------------------------------------
// Procedure descriptor types
// ---------------------------------------------------------------------------

/** The three procedure flavors taut-rpc supports. */
export type ProcedureKind = "query" | "mutation" | "subscription";

/**
 * Per-procedure descriptor. Generated `api.gen.ts` produces a `Procedures`
 * type-map of these — one entry per `#[rpc]` procedure on the server.
 *
 * The `__i` / `__o` / `__e` fields are phantom: they exist only in the type
 * domain to carry input/output/error types for inference. They are never
 * assigned at runtime.
 */
export interface ProcedureDef<I, O, E> {
  kind: ProcedureKind;
  /** Procedure name as registered server-side (e.g., `"users.get"`). */
  name: string;
  /** Phantom: input type. Never assigned. */
  __i?: I;
  /** Phantom: output type. Never assigned. */
  __o?: O;
  /** Phantom: error type. Never assigned. */
  __e?: E;
}

/**
 * A type-map of procedures. Example shape:
 *
 *   { "users.get": ProcedureDef<{ id: number }, User, NotFound>,
 *     "users.list": ProcedureDef<void, User[], never>,
 *     ... }
 */
export type Procedures = Record<string, ProcedureDef<any, any, any>>;

// ---------------------------------------------------------------------------
// Transport contract
// ---------------------------------------------------------------------------

/**
 * Transport abstraction. Implementations live in sibling modules:
 *   - {@link HttpTransport} for query/mutation over fetch.
 *   - {@link SseTransport} for subscriptions over Server-Sent Events.
 *
 * The `kind` argument is informational for the transport; over HTTP it is
 * forwarded as an `x-taut-kind` request header (SPEC §4.1) so middleware can
 * distinguish queries from mutations even though the wire shape is identical.
 */
export interface Transport {
  /** Call a query or mutation. Resolves with the decoded `ok` payload. */
  call<I, O, E>(name: string, kind: ProcedureKind, input: I): Promise<O>;
  /**
   * Subscribe to a streaming procedure. Implementations must throw if the
   * underlying procedure is not a subscription, but the runtime cannot
   * verify this — the type-map enforces it statically.
   */
  subscribe<I, O, E>(name: string, input: I): AsyncIterable<O>;
}

// ---------------------------------------------------------------------------
// Client options
// ---------------------------------------------------------------------------

export interface ClientOptions {
  /** Base URL the procedures are mounted under (e.g., `"/rpc"`). */
  url: string;
  /** Custom transport. If omitted, a default {@link HttpTransport} is created from `url`. */
  transport?: Transport;
  /** Request timeout in milliseconds. Default: `30_000`. */
  timeoutMs?: number;
  /** Custom fetch implementation (for tests, SSR, or polyfills). */
  fetch?: typeof fetch;
  /** Headers added to every request. Either a static map or a thunk evaluated per request. */
  headers?: Record<string, string> | (() => Record<string, string>);
}

// ---------------------------------------------------------------------------
// Public type: ClientOf<P>
// ---------------------------------------------------------------------------

/**
 * Map a `Procedures` type-map to the proxy interface the user sees.
 *
 * - Subscriptions become `{ subscribe(input) => AsyncIterable<O> }`.
 * - Queries/mutations become `(input) => Promise<O>` (or `() => Promise<O>`
 *   when the input type is `void` / `undefined`).
 *
 * Note: this collapses the dotted form to a flat keyed object. Users may
 * write either `client["users.get"]({...})` or `client.users.get({...})`
 * at runtime — the latter is supported by the proxy, but TypeScript only
 * sees the flat form here. Generated `api.gen.ts` typically re-exports a
 * nested type-map for nicer dotted IntelliSense.
 */
export type ClientOf<P extends Procedures> = {
  [K in keyof P]: P[K] extends ProcedureDef<infer I, infer O, infer _E>
    ? P[K]["kind"] extends "subscription"
      ? {
          subscribe: I extends void | undefined
            ? () => AsyncIterable<O>
            : (input: I) => AsyncIterable<O>;
        }
      : I extends void | undefined
        ? () => Promise<O>
        : (input: I) => Promise<O>
    : never;
};

// ---------------------------------------------------------------------------
// createClient
// ---------------------------------------------------------------------------

/** Sentinel marking the root proxy so we can detect it in nested gets. */
const ROOT = Symbol("taut.client.root");

/** Internal: build a chainable proxy that accumulates a dotted name path. */
function makeProxy(transport: Transport, path: readonly string[]): any {
  // The target is a function so the proxy is callable.
  const target = (() => {}) as any;

  return new Proxy(target, {
    get(_t, prop, _receiver) {
      // Skip well-known symbols / promise-thenable interrogation.
      if (typeof prop === "symbol") return undefined;
      // `subscribe` at any depth ≥ 1 marks the leaf as a subscription.
      // We return a function that fires the subscribe call with the path so far.
      if (prop === "subscribe" && path.length > 0) {
        return (input?: unknown) =>
          transport.subscribe(path.join("."), input);
      }
      // Otherwise extend the path.
      return makeProxy(transport, [...path, prop]);
    },
    apply(_t, _thisArg, args) {
      // Direct call: treat as query/mutation. We don't know which without the
      // type-map at runtime — default to "query". The wire format is identical
      // (SPEC §4.1); only the `x-taut-kind` header differs and is informational.
      const name = path.join(".");
      const input = args.length === 0 ? undefined : args[0];
      return transport.call(name, "query", input);
    },
  });
}

/**
 * Build a typed client from a `Procedures` type-map.
 *
 * The returned client is a `Proxy` that turns property accesses into procedure
 * calls. Both dotted and flat access are supported at runtime:
 *
 *   client.users.get({ id: 1 })       // → call("users.get", "query", ...)
 *   client["users.get"]({ id: 1 })    // same
 *   client.userEvents.subscribe(...)  // → subscribe("userEvents", ...)
 *
 * Static type-safety comes entirely from `P` (the user's generated
 * `Procedures` map); the runtime is duck-typed.
 */
export function createClient<P extends Procedures>(
  opts: ClientOptions,
): ClientOf<P> {
  const transport = opts.transport ?? defaultTransport(opts);
  return makeProxy(transport, []) as ClientOf<P>;
}

/**
 * Construct the default HTTP transport from {@link ClientOptions}.
 *
 * Lazy-instantiates {@link HttpTransport} via dynamic import so that the
 * subscription-only consumer can omit the HTTP code path, and so that this
 * module has no eager runtime dependency on `./http` (which is owned by a
 * separate file). The dynamic import resolves once and is then memoized
 * inside the returned `Transport` shim.
 */
function defaultTransport(opts: ClientOptions): Transport {
  let httpReady: Promise<Transport> | null = null;
  let sseReady: Promise<Transport> | null = null;

  const ensureHttp = (): Promise<Transport> => {
    if (httpReady) return httpReady;
    httpReady = import("./http.js").then(
      (m) =>
        new m.HttpTransport({
          url: opts.url,
          timeoutMs: opts.timeoutMs ?? 30_000,
          ...(opts.fetch !== undefined ? { fetch: opts.fetch } : {}),
          ...(opts.headers !== undefined ? { headers: opts.headers } : {}),
        }) as unknown as Transport,
    );
    return httpReady;
  };
  const ensureSse = (): Promise<Transport> => {
    if (sseReady) return sseReady;
    sseReady = import("./sse.js").then(
      (m) =>
        new m.SseTransport({
          url: opts.url,
          ...(opts.fetch !== undefined ? { fetch: opts.fetch } : {}),
          ...(opts.headers !== undefined ? { headers: opts.headers } : {}),
        }) as unknown as Transport,
    );
    return sseReady;
  };

  return {
    async call<I, O, E>(
      name: string,
      kind: ProcedureKind,
      input: I,
    ): Promise<O> {
      const t = await ensureHttp();
      return t.call<I, O, E>(name, kind, input);
    },
    subscribe<I, O, E>(name: string, input: I): AsyncIterable<O> {
      // We must return an AsyncIterable synchronously, so wrap the lazy load
      // inside an async generator that awaits the transport on first pull.
      return {
        [Symbol.asyncIterator]: async function* () {
          const t = await ensureSse();
          yield* t.subscribe<I, O, E>(name, input);
        },
      };
    },
  };
}

// ---------------------------------------------------------------------------
// Re-exports — sibling transport implementations
// ---------------------------------------------------------------------------

export { HttpTransport } from "./http.js";
export { SseTransport } from "./sse.js";

// ---------------------------------------------------------------------------
// In-source tests (vitest)
// ---------------------------------------------------------------------------

// TODO(taut-rpc): vitest is not yet configured in this package. Once the test
// harness lands, uncomment the block below and ensure tsconfig sets
// `"types": ["vitest/importMeta"]` (or augment ImportMeta) so `import.meta.vitest`
// type-checks.
//
// if (import.meta.vitest) {
//   const { describe, it, expect, vi } = import.meta.vitest;
//
//   describe("createClient proxy", () => {
//     const fakeTransport: Transport = {
//       call: vi.fn(async (_name, _kind, _input) => ({ ok: true }) as any),
//       subscribe: vi.fn((_name, _input) => ({
//         [Symbol.asyncIterator]: async function* () {
//           yield 1 as any;
//           yield 2 as any;
//         },
//       })),
//     };
//
//     it("dotted access composes to a procedure name", async () => {
//       const c = createClient<any>({ url: "/rpc", transport: fakeTransport });
//       await (c as any).users.get({ id: 1 });
//       expect(fakeTransport.call).toHaveBeenCalledWith("users.get", "query", { id: 1 });
//     });
//
//     it("flat access works too", async () => {
//       const c = createClient<any>({ url: "/rpc", transport: fakeTransport });
//       await (c as any)["users.get"]({ id: 1 });
//       expect(fakeTransport.call).toHaveBeenCalledWith("users.get", "query", { id: 1 });
//     });
//
//     it("subscribe returns an async iterable", async () => {
//       const c = createClient<any>({ url: "/rpc", transport: fakeTransport });
//       const out: number[] = [];
//       for await (const v of (c as any).userEvents.subscribe({ userId: 1 })) {
//         out.push(v);
//       }
//       expect(out).toEqual([1, 2]);
//       expect(fakeTransport.subscribe).toHaveBeenCalledWith("userEvents", { userId: 1 });
//     });
//
//     it("zero-arg call passes undefined input", async () => {
//       const c = createClient<any>({ url: "/rpc", transport: fakeTransport });
//       await (c as any).ping();
//       expect(fakeTransport.call).toHaveBeenCalledWith("ping", "query", undefined);
//     });
//   });
// }

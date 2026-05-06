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
export interface ProcedureDef<I, O, E, K extends ProcedureKind = ProcedureKind> {
  kind: K;
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
  /**
   * Optional map of procedure-name → kind. Generated `api.gen.ts` (Phase 1
   * `createApi` helper) supplies this so the runtime tags each call with the
   * correct {@link ProcedureKind} on the wire (forwarded by the HTTP transport
   * as `x-taut-kind`). Without it, the proxy defaults plain calls to `"query"`.
   * Subscriptions are detected from the `subscribe` access regardless.
   */
  kinds?: Record<string, ProcedureKind>;
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
export type ClientOf<P> = {
  [K in keyof P]: P[K] extends ProcedureDef<infer I, infer O, infer _E, infer Kind>
    ? Kind extends "subscription"
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
function makeProxy(
  transport: Transport,
  path: readonly string[],
  kinds: Record<string, ProcedureKind> | undefined,
): any {
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
      return makeProxy(transport, [...path, prop], kinds);
    },
    apply(_t, _thisArg, args) {
      // Direct call: query or mutation. We resolve the kind via the optional
      // `kinds` map (supplied by codegen). Without it we default to `"query"`.
      // The wire format is identical (SPEC §4.1); only the `x-taut-kind`
      // header differs and is informational.
      const name = path.join(".");
      const input = args.length === 0 ? undefined : args[0];
      const kind: ProcedureKind = kinds?.[name] ?? "query";
      return transport.call(name, kind, input);
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
export function createClient<P>(
  opts: ClientOptions,
): ClientOf<P> {
  const transport = opts.transport ?? defaultTransport(opts);
  return makeProxy(transport, [], opts.kinds) as ClientOf<P>;
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

export { HttpTransport, TautError } from "./http.js";
export { SseTransport } from "./sse.js";

// ---------------------------------------------------------------------------
// Error narrowing helpers
// ---------------------------------------------------------------------------

import { TautError as _TautError } from "./http.js";

/**
 * Type-guard that narrows `unknown` to {@link TautError} with no code
 * constraint. Useful in `catch` blocks when you just need to distinguish
 * RPC errors from arbitrary thrown values.
 */
export function isTautError(err: unknown): err is _TautError<string, unknown>;
/**
 * Type-guard that narrows `unknown` to {@link TautError} with a specific
 * `code`. The payload remains `unknown` — pass an explicit second type
 * parameter to also narrow `.payload`.
 *
 * ```ts
 * if (isTautError(err, "not_found")) {
 *   //          ^? TautError<"not_found", unknown>
 *   console.warn("missing:", err.payload);
 * }
 * ```
 */
export function isTautError<C extends string>(
  err: unknown,
  code: C,
): err is _TautError<C, unknown>;
/**
 * Type-guard that narrows `unknown` to {@link TautError} with both a
 * specific `code` and a caller-supplied payload type. The payload type
 * is purely a static assertion — there is no runtime check on its shape;
 * trust comes from the procedure's declared error type in `api.gen.ts`.
 *
 * ```ts
 * type NotFoundPayload = { id: number };
 * if (isTautError<"not_found", NotFoundPayload>(err, "not_found")) {
 *   //          ^? TautError<"not_found", NotFoundPayload>
 *   console.warn("missing id:", err.payload.id);
 * }
 * ```
 */
export function isTautError<C extends string, P>(
  err: unknown,
  code: C,
): err is _TautError<C, P>;
export function isTautError(
  err: unknown,
  code?: string,
): err is _TautError<string, unknown> {
  if (!(err instanceof _TautError)) return false;
  if (code !== undefined && err.code !== code) return false;
  return true;
}

/**
 * Assert that `err` is a {@link TautError}, otherwise re-throw it. Useful
 * inside `catch` blocks where any non-RPC error should propagate unchanged:
 *
 * ```ts
 * try {
 *   await client.add({ a, b });
 * } catch (e) {
 *   assertTautError(e);
 *   //              ^ after this line, e is TautError<string, unknown>
 *   logger.warn(e.code, e.payload);
 * }
 * ```
 */
export function assertTautError(
  err: unknown,
): asserts err is _TautError<string, unknown>;
/**
 * Assert that `err` is a {@link TautError} with a specific `code`. Any
 * other thrown value — including a `TautError` with a different code —
 * is re-thrown unchanged.
 *
 * ```ts
 * try {
 *   await client.users.get({ id });
 * } catch (e) {
 *   assertTautError(e, "not_found");
 *   //              ^ after this line, e is TautError<"not_found", unknown>
 * }
 * ```
 */
export function assertTautError<C extends string>(
  err: unknown,
  code: C,
): asserts err is _TautError<C, unknown>;
export function assertTautError(err: unknown, code?: string): void {
  if (!(err instanceof _TautError)) throw err;
  if (code !== undefined && err.code !== code) throw err;
}

/**
 * Pattern-match on a {@link TautError} by code. Returns the value of the
 * matched arm, or `defaultArm(err)` if no arm matched and a default was
 * supplied. Non-`TautError` values propagate unchanged; an unmatched
 * `TautError` with no `defaultArm` is also re-thrown.
 *
 * The generic `E` should be the procedure's declared error union (e.g.
 * `Proc_add_Error` from `api.gen.ts`). The `arms` object is exhaustive
 * over `E["code"]` — adding a new server-side code becomes a compile error
 * at every call site.
 *
 * ```ts
 * try {
 *   await client.add({ a, b });
 * } catch (e) {
 *   errorMatch<Proc_add_Error, void>(e, {
 *     overflow: () => console.log("overflow"),
 *     underflow: () => console.log("underflow"),
 *   });
 * }
 * ```
 */
export function errorMatch<E extends _TautError<string, unknown>, R>(
  err: unknown,
  arms: { [K in E["code"]]: (e: Extract<E, { code: K }>) => R },
  defaultArm?: (err: _TautError<string, unknown>) => R,
): R {
  if (!(err instanceof _TautError)) throw err;
  const lookup = arms as unknown as Record<
    string,
    (e: _TautError<string, unknown>) => R
  >;
  const handler = lookup[err.code];
  if (handler) return handler(err);
  if (defaultArm) return defaultArm(err);
  throw err;
}

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
//
//   describe("error narrowing helpers", () => {
//     it("isTautError() with no args narrows to TautError", () => {
//       const e: unknown = new _TautError("boom", { reason: "x" }, 500);
//       expect(isTautError(e)).toBe(true);
//       expect(isTautError(new Error("plain"))).toBe(false);
//       expect(isTautError("string error")).toBe(false);
//     });
//
//     it("isTautError(err, code) matches only the given code", () => {
//       const overflow: unknown = new _TautError("overflow", null, 400);
//       const underflow: unknown = new _TautError("underflow", null, 400);
//       expect(isTautError(overflow, "overflow")).toBe(true);
//       expect(isTautError(underflow, "overflow")).toBe(false);
//     });
//
//     it("isTautError<C, P> narrows payload statically", () => {
//       type NotFoundPayload = { id: number };
//       const e: unknown = new _TautError("not_found", { id: 7 }, 404);
//       if (isTautError<"not_found", NotFoundPayload>(e, "not_found")) {
//         // Type-level: e.payload is NotFoundPayload here.
//         expect(e.payload.id).toBe(7);
//       } else {
//         throw new Error("expected narrowing to succeed");
//       }
//     });
//
//     it("assertTautError throws on non-TautError values", () => {
//       const plain = new Error("plain");
//       expect(() => assertTautError(plain)).toThrow(plain);
//       expect(() => assertTautError("not an error")).toThrow();
//       const taut = new _TautError("ok_code", null, 400);
//       expect(() => assertTautError(taut)).not.toThrow();
//       // With a code, mismatched codes re-throw.
//       expect(() => assertTautError(taut, "other_code")).toThrow(taut);
//     });
//
//     it("errorMatch dispatches to the matching arm and re-throws others", () => {
//       type AddErr =
//         | _TautError<"overflow", null>
//         | _TautError<"underflow", null>;
//       const overflow: unknown = new _TautError("overflow", null, 400);
//       const result = errorMatch<AddErr, string>(overflow, {
//         overflow: () => "hi-overflow",
//         underflow: () => "hi-underflow",
//       });
//       expect(result).toBe("hi-overflow");
//
//       // Unmatched code with no defaultArm re-throws.
//       const other: unknown = new _TautError("other", null, 400);
//       expect(() =>
//         errorMatch<AddErr, string>(other as any, {
//           overflow: () => "hi-overflow",
//           underflow: () => "hi-underflow",
//         }),
//       ).toThrow();
//
//       // Non-TautError propagates unchanged.
//       const plain = new Error("plain");
//       expect(() =>
//         errorMatch<AddErr, string>(plain, {
//           overflow: () => "hi-overflow",
//           underflow: () => "hi-underflow",
//         }),
//       ).toThrow(plain);
//     });
//   });
// }

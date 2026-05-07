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

/**
 * Minimal duck-type for Valibot or Zod schemas. Both libraries expose a
 * synchronous `parse(value)` method that either returns the (possibly coerced)
 * value or throws a library-specific error. The runtime never imports either
 * library directly — codegen wires concrete schemas in via `procedureSchemas`.
 *
 * On parse failure, both libraries throw an error whose `.issues` (Valibot,
 * Zod v4) or `.errors` (Zod v3) array describes each validation problem.
 * {@link parseValidationIssues} flattens that into a transport-neutral
 * `{path, message}[]` payload for the SPEC error envelope.
 */
export interface SchemaLike {
  parse(value: unknown): unknown;
}

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
  /**
   * Per-procedure schema map (typically supplied by codegen as
   * `procedureSchemas`). When an entry is present and the corresponding
   * {@link ClientOptions.validate} toggle is not `false`, the runtime parses
   * inputs before sending and outputs after receiving (SPEC §7).
   *
   * Procedures whose entry is missing — or whose `input`/`output` slot is
   * `undefined` (e.g. codegen ran with `--validator none`) — skip validation
   * silently; only present schemas trigger a parse.
   */
  schemas?: Record<string, { input?: SchemaLike; output?: SchemaLike }>;
  /**
   * Validation toggle. Default: `{ send: true, recv: true }` — i.e.
   * validation is on whenever a schema is supplied. Setting either to `false`
   * disables that direction client-wide; the `schemas` map is then ignored
   * for that direction.
   */
  validate?: {
    send?: boolean;
    recv?: boolean;
  };
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
// Validation helpers
// ---------------------------------------------------------------------------

/**
 * One issue inside a validation-error payload. Mirrors the shape both Valibot
 * and Zod produce on parse failure, normalised to the minimum every consumer
 * needs: a stringified path through the input and a human-readable message.
 */
export interface ValidationIssue {
  path: string;
  message: string;
}

/**
 * Convert a Valibot/Zod parse exception into a `{ path, message }[]` payload
 * suitable for the {@link TautError} envelope. We duck-type on the error's
 * `.issues` (Valibot, Zod v4) or `.errors` (Zod v3) array; anything else is
 * preserved as a single issue with `path: ""` and the error's `message` (or
 * stringified form).
 *
 * The shape is deliberately library-neutral so callers can switch validators
 * without breaking error-handling code downstream.
 */
export function parseValidationIssues(err: unknown): ValidationIssue[] {
  // Both libraries throw real Error subclasses; pull `issues` then `errors`
  // before falling back to a single bag-issue derived from the message.
  if (err && typeof err === "object") {
    const e = err as { issues?: unknown; errors?: unknown; message?: unknown };
    const list =
      Array.isArray(e.issues)
        ? e.issues
        : Array.isArray(e.errors)
          ? e.errors
          : null;
    if (list !== null) {
      return list.map(issueToValidation);
    }
    if (typeof e.message === "string") {
      return [{ path: "", message: e.message }];
    }
  }
  return [{ path: "", message: typeof err === "string" ? err : String(err) }];
}

/**
 * Normalise a single Valibot/Zod issue object into our `{ path, message }`
 * shape.
 *
 * Path encoding:
 *   - Valibot: `issue.path` is `Array<{ key: string | number }>`
 *   - Zod v3/v4: `issue.path` is `Array<string | number>`
 * We coerce both into a dotted/bracketed string (`"a.b[0].c"`) without
 * importing either library.
 */
function issueToValidation(raw: unknown): ValidationIssue {
  const issue = raw as { path?: unknown; message?: unknown };
  const message =
    typeof issue.message === "string" ? issue.message : "validation failed";
  if (!Array.isArray(issue.path)) return { path: "", message };
  const parts: string[] = [];
  for (const seg of issue.path) {
    // Valibot wraps each segment in `{ key, value, ... }`.
    const key =
      seg !== null && typeof seg === "object" && "key" in (seg as object)
        ? (seg as { key: unknown }).key
        : seg;
    if (typeof key === "number") {
      parts.push(`[${key}]`);
    } else if (typeof key === "string") {
      parts.push(parts.length === 0 ? key : `.${key}`);
    } else {
      parts.push(`[${String(key)}]`);
    }
  }
  return { path: parts.join(""), message };
}

/**
 * Run a {@link SchemaLike}.parse(), translating any thrown Valibot/Zod error
 * into a {@link TautError} with code `"validation_error"`, payload `{ issues }`,
 * and HTTP status `0` (no network exchange occurred). The `direction`
 * discriminant lets callers tell input vs. output failures apart in logs.
 */
function runSchemaParse(
  schema: SchemaLike,
  value: unknown,
  direction: "input" | "output",
): unknown {
  try {
    return schema.parse(value);
  } catch (err) {
    const issues = parseValidationIssues(err);
    throw new _TautError(
      "validation_error",
      { direction, issues },
      0,
    );
  }
}

/**
 * Wrap an `AsyncIterable<O>` so each yielded value is fed through
 * `schema.parse()` before being surfaced. The wrapper preserves the inner
 * iterator's `return`/`throw` plumbing so consumer-driven cancellation
 * (`break` / early `return` / thrown exceptions inside `for await`) still
 * cancels the underlying transport.
 *
 * Output validation happens per-frame so the consumer of a long-lived
 * subscription notices a malformed frame at the moment it arrives, rather
 * than the whole stream being eagerly drained up front.
 */
function validateAsyncIterable<O>(
  inner: AsyncIterable<O>,
  schema: SchemaLike,
): AsyncIterable<O> {
  return {
    [Symbol.asyncIterator](): AsyncIterator<O> {
      const it = inner[Symbol.asyncIterator]();
      // Build the iterator with `exactOptionalPropertyTypes` in mind: we
      // only set `return` / `throw` when the inner iterator exposes them,
      // so the wrapper preserves cancellation semantics without inventing
      // them (and without assigning `undefined` to optional slots).
      const wrapped: AsyncIterator<O> = {
        async next(): Promise<IteratorResult<O>> {
          const r = await it.next();
          if (r.done) return r;
          const validated = runSchemaParse(schema, r.value, "output") as O;
          return { value: validated, done: false };
        },
      };
      if (it.return) {
        const innerReturn = it.return.bind(it);
        wrapped.return = (value?: O): Promise<IteratorResult<O>> =>
          innerReturn(value) as Promise<IteratorResult<O>>;
      }
      if (it.throw) {
        const innerThrow = it.throw.bind(it);
        wrapped.throw = (e?: unknown): Promise<IteratorResult<O>> =>
          innerThrow(e) as Promise<IteratorResult<O>>;
      }
      return wrapped;
    },
  };
}

// ---------------------------------------------------------------------------
// createClient
// ---------------------------------------------------------------------------

/** Sentinel marking the root proxy so we can detect it in nested gets. */
const ROOT = Symbol("taut.client.root");

/**
 * Resolved validation state for the proxy. Pre-computing the booleans means
 * the per-call hot path doesn't re-check `validate.send !== false` on every
 * request.
 */
interface ProxyContext {
  transport: Transport;
  kinds: Record<string, ProcedureKind> | undefined;
  schemas: Record<string, { input?: SchemaLike; output?: SchemaLike }> | undefined;
  validateSend: boolean;
  validateRecv: boolean;
}

/** Internal: build a chainable proxy that accumulates a dotted name path. */
function makeProxy(ctx: ProxyContext, path: readonly string[]): any {
  // The target is a function so the proxy is callable.
  const target = (() => {}) as any;

  return new Proxy(target, {
    get(_t, prop, _receiver) {
      // Skip well-known symbols / promise-thenable interrogation.
      if (typeof prop === "symbol") return undefined;
      // `subscribe` at any depth ≥ 1 marks the leaf as a subscription.
      // We return a function that fires the subscribe call with the path so far.
      if (prop === "subscribe" && path.length > 0) {
        return (input?: unknown): AsyncIterable<unknown> => {
          const name = path.join(".");
          const entry = ctx.schemas?.[name];
          // Pre-call input validation: throws synchronously (well, before the
          // first iterator pull) so the consumer sees a TautError on `next()`.
          // We run it eagerly here rather than inside the wrapped iterable so
          // misuse fails before any network/SSE work begins.
          let validatedInput: unknown = input;
          if (ctx.validateSend && entry?.input) {
            validatedInput = runSchemaParse(entry.input, input, "input");
          }
          const stream = ctx.transport.subscribe(name, validatedInput);
          if (ctx.validateRecv && entry?.output) {
            return validateAsyncIterable(stream, entry.output);
          }
          return stream;
        };
      }
      // Otherwise extend the path.
      return makeProxy(ctx, [...path, prop]);
    },
    apply(_t, _thisArg, args) {
      // Direct call: query or mutation. We resolve the kind via the optional
      // `kinds` map (supplied by codegen). Without it we default to `"query"`.
      // The wire format is identical (SPEC §4.1); only the `x-taut-kind`
      // header differs and is informational.
      const name = path.join(".");
      const input = args.length === 0 ? undefined : args[0];
      const kind: ProcedureKind = ctx.kinds?.[name] ?? "query";
      const entry = ctx.schemas?.[name];

      // Pre-send input validation. Per SPEC §7 this fails BEFORE the network
      // call, surfacing a TautError("validation_error") rather than a 4xx.
      // Zero-arg procedures pass `undefined` here; the wire shape uses JSON
      // null, so we coerce before parsing so a `v.null()` schema matches.
      const inputForParse = input === undefined ? null : input;
      let validatedInput: unknown = inputForParse;
      if (ctx.validateSend && entry?.input) {
        validatedInput = runSchemaParse(entry.input, inputForParse, "input");
      }

      const result = ctx.transport.call(name, kind, validatedInput);

      // Post-receive output validation. We only parse on the success path —
      // error envelopes (`TautError` thrown from the transport) propagate
      // unchanged.
      if (ctx.validateRecv && entry?.output) {
        const outputSchema = entry.output;
        return result.then((value) =>
          runSchemaParse(outputSchema, value, "output"),
        );
      }
      return result;
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
 *
 * When `schemas` is supplied (typically by codegen), inputs are parsed before
 * sending and outputs are parsed after receiving (SPEC §7). Either direction
 * can be disabled by setting `validate.send` / `validate.recv` to `false`.
 * Procedures whose entry is missing in `schemas` skip validation silently —
 * this is the `--validator none` codegen path.
 */
export function createClient<P>(
  opts: ClientOptions,
): ClientOf<P> {
  const transport = opts.transport ?? defaultTransport(opts);
  const ctx: ProxyContext = {
    transport,
    kinds: opts.kinds,
    schemas: opts.schemas,
    validateSend: opts.validate?.send !== false,
    validateRecv: opts.validate?.recv !== false,
  };
  return makeProxy(ctx, []) as ClientOf<P>;
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

// PSEUDO-CODE in-source tests. The body below is wrapped in an
// `if (import.meta.vitest)` guard, which Vite's vitest plugin rewrites to
// `undefined` for production builds and to the live module under test inside
// the harness. Until vitest is wired (and `tsconfig` adds
// `"types": ["vitest/importMeta"]` or augments ImportMeta), the guard is
// always falsy at runtime and the body never executes — these tests are
// documentation of the contract, exercised by `tsc` today and activated
// the moment the test harness lands.
//
// The `// @ts-ignore` on the guard line isolates the only spot that needs
// vitest's ImportMeta augmentation. Everything inside is regular TS that
// typechecks on its own merits.
// @ts-ignore vitest ImportMeta augmentation not yet installed
if (import.meta.vitest) {
  // @ts-ignore vitest ImportMeta augmentation not yet installed
  const { describe, it, expect, vi } = import.meta.vitest;

  describe("createClient proxy", () => {
    const fakeTransport: Transport = {
      call: vi.fn(async (_name: string, _kind: ProcedureKind, _input: unknown) => ({ ok: true }) as any),
      subscribe: vi.fn((_name: string, _input: unknown) => ({
        [Symbol.asyncIterator]: async function* () {
          yield 1 as any;
          yield 2 as any;
        },
      })),
    };

    it("dotted access composes to a procedure name", async () => {
      const c = createClient<any>({ url: "/rpc", transport: fakeTransport });
      await (c as any).users.get({ id: 1 });
      expect(fakeTransport.call).toHaveBeenCalledWith("users.get", "query", { id: 1 });
    });

    it("flat access works too", async () => {
      const c = createClient<any>({ url: "/rpc", transport: fakeTransport });
      await (c as any)["users.get"]({ id: 1 });
      expect(fakeTransport.call).toHaveBeenCalledWith("users.get", "query", { id: 1 });
    });

    it("subscribe returns an async iterable", async () => {
      const c = createClient<any>({ url: "/rpc", transport: fakeTransport });
      const out: number[] = [];
      for await (const v of (c as any).userEvents.subscribe({ userId: 1 })) {
        out.push(v as number);
      }
      expect(out).toEqual([1, 2]);
      expect(fakeTransport.subscribe).toHaveBeenCalledWith("userEvents", { userId: 1 });
    });

    it("zero-arg call passes undefined input", async () => {
      const c = createClient<any>({ url: "/rpc", transport: fakeTransport });
      await (c as any).ping();
      expect(fakeTransport.call).toHaveBeenCalledWith("ping", "query", undefined);
    });
  });

  // -------------------------------------------------------------------------
  // Validation hooks (Phase 4)
  //
  // These exercise the `schemas` + `validate` knobs Agent 9 added to
  // ClientOptions. Each test is PSEUDO-CODE: it documents the runtime
  // contract without depending on a real validator (Valibot/Zod). We
  // hand-roll `SchemaLike` objects whose `parse` either echoes the value
  // (success) or throws an object shaped like a Valibot/Zod error (failure
  // path picked up by `parseValidationIssues`).
  // -------------------------------------------------------------------------

  describe("createClient validation hooks", () => {
    /** SchemaLike that accepts every value unchanged. */
    const passThroughSchema: SchemaLike = {
      parse: (v: unknown) => v,
    };

    /**
     * SchemaLike that always throws a Valibot/Zod-shaped error. The
     * `.issues` array is what `parseValidationIssues` keys off, so the
     * resulting TautError lands with a non-empty `issues` payload.
     */
    const rejectingSchema: SchemaLike = {
      parse: (_v: unknown): unknown => {
        throw {
          issues: [{ path: [{ key: "id" }], message: "expected number" }],
        };
      },
    };

    /** Echoing transport: returns whatever it was given as the `ok` payload. */
    const makeEchoTransport = (): Transport => ({
      call: vi.fn(async (_name: string, _kind: ProcedureKind, input: unknown) => input),
      subscribe: vi.fn((_name: string, input: unknown) => ({
        [Symbol.asyncIterator]: async function* () {
          yield input as any;
        },
      })),
    });

    it("validate.send=true with a matching schema accepts input", async () => {
      const t = makeEchoTransport();
      const c = createClient<any>({
        url: "/rpc",
        transport: t,
        schemas: {
          "users.get": { input: passThroughSchema },
        },
        validate: { send: true },
      });
      // No throw expected; the schema's parse() is a pure pass-through.
      const out = await (c as any).users.get({ id: 1 });
      expect(t.call).toHaveBeenCalledWith("users.get", "query", { id: 1 });
      expect(out).toEqual({ id: 1 });
    });

    it("validate.send=true rejects input via schema.parse failure", async () => {
      const t = makeEchoTransport();
      const c = createClient<any>({
        url: "/rpc",
        transport: t,
        schemas: {
          "users.get": { input: rejectingSchema },
        },
        validate: { send: true },
      });
      await expect((c as any).users.get({ id: "not-a-number" })).rejects.toMatchObject({
        code: "validation_error",
        payload: { direction: "input" },
      });
      // The transport must NOT be called when input validation fails.
      expect(t.call).not.toHaveBeenCalled();
    });

    it("validate.send=false bypasses schema check", async () => {
      const t = makeEchoTransport();
      const c = createClient<any>({
        url: "/rpc",
        transport: t,
        schemas: {
          "users.get": { input: rejectingSchema },
        },
        validate: { send: false },
      });
      // Even though the schema would throw, validate.send=false skips the
      // parse step entirely — the call reaches the transport unchanged.
      const out = await (c as any).users.get({ id: "not-a-number" });
      expect(t.call).toHaveBeenCalledWith(
        "users.get",
        "query",
        { id: "not-a-number" },
      );
      expect(out).toEqual({ id: "not-a-number" });
    });

    it("validate.recv=true triggers output parse", async () => {
      const outputSchema: SchemaLike = {
        parse: vi.fn((v: unknown) => v),
      };
      const t = makeEchoTransport();
      const c = createClient<any>({
        url: "/rpc",
        transport: t,
        schemas: {
          "users.get": { output: outputSchema },
        },
        validate: { recv: true },
      });
      await (c as any).users.get({ id: 1 });
      // The output schema's parse must run exactly once with the transport's
      // returned value (here echoed back: `{ id: 1 }`).
      expect(outputSchema.parse).toHaveBeenCalledTimes(1);
      expect(outputSchema.parse).toHaveBeenCalledWith({ id: 1 });
    });

    it("procedureSchemas[unknownProc] === undefined → no validation", async () => {
      // The schemas map only knows about `users.get`; calling `users.list`
      // must skip validation silently (the `--validator none` codegen path
      // and the missing-entry fast path share this behavior).
      const inputSpy: SchemaLike = { parse: vi.fn((v: unknown) => v) };
      const outputSpy: SchemaLike = { parse: vi.fn((v: unknown) => v) };
      const t = makeEchoTransport();
      const c = createClient<any>({
        url: "/rpc",
        transport: t,
        schemas: {
          "users.get": { input: inputSpy, output: outputSpy },
        },
        // Defaults: validate.send=true, validate.recv=true.
      });
      await (c as any).users.list({ page: 1 });
      expect(inputSpy.parse).not.toHaveBeenCalled();
      expect(outputSpy.parse).not.toHaveBeenCalled();
      expect(t.call).toHaveBeenCalledWith(
        "users.list",
        "query",
        { page: 1 },
      );
    });
  });

  describe("error narrowing helpers", () => {
    it("isTautError() with no args narrows to TautError", () => {
      const e: unknown = new _TautError("boom", { reason: "x" }, 500);
      expect(isTautError(e)).toBe(true);
      expect(isTautError(new Error("plain"))).toBe(false);
      expect(isTautError("string error")).toBe(false);
    });

    it("isTautError(err, code) matches only the given code", () => {
      const overflow: unknown = new _TautError("overflow", null, 400);
      const underflow: unknown = new _TautError("underflow", null, 400);
      expect(isTautError(overflow, "overflow")).toBe(true);
      expect(isTautError(underflow, "overflow")).toBe(false);
    });

    it("isTautError<C, P> narrows payload statically", () => {
      type NotFoundPayload = { id: number };
      const e: unknown = new _TautError("not_found", { id: 7 }, 404);
      if (isTautError<"not_found", NotFoundPayload>(e, "not_found")) {
        // Type-level: e.payload is NotFoundPayload here.
        expect(e.payload.id).toBe(7);
      } else {
        throw new Error("expected narrowing to succeed");
      }
    });

    it("assertTautError throws on non-TautError values", () => {
      const plain = new Error("plain");
      expect(() => assertTautError(plain)).toThrow(plain);
      expect(() => assertTautError("not an error")).toThrow();
      const taut = new _TautError("ok_code", null, 400);
      expect(() => assertTautError(taut)).not.toThrow();
      // With a code, mismatched codes re-throw.
      expect(() => assertTautError(taut, "other_code")).toThrow(taut);
    });

    it("errorMatch dispatches to the matching arm and re-throws others", () => {
      type AddErr =
        | _TautError<"overflow", null>
        | _TautError<"underflow", null>;
      const overflow: unknown = new _TautError("overflow", null, 400);
      const result = errorMatch<AddErr, string>(overflow, {
        overflow: () => "hi-overflow",
        underflow: () => "hi-underflow",
      });
      expect(result).toBe("hi-overflow");

      // Unmatched code with no defaultArm re-throws.
      const other: unknown = new _TautError("other", null, 400);
      expect(() =>
        errorMatch<AddErr, string>(other as any, {
          overflow: () => "hi-overflow",
          underflow: () => "hi-underflow",
        }),
      ).toThrow();

      // Non-TautError propagates unchanged.
      const plain = new Error("plain");
      expect(() =>
        errorMatch<AddErr, string>(plain, {
          overflow: () => "hi-overflow",
          underflow: () => "hi-underflow",
        }),
      ).toThrow(plain);
    });
  });
}

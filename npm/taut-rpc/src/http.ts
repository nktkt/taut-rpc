/**
 * HTTP transport for the taut-rpc TypeScript runtime.
 *
 * Implements the query/mutation wire format defined in SPEC §4.1:
 *
 *   POST /rpc/<procedure>
 *   Content-Type: application/json
 *   Body: { "input": <Input> }
 *
 *   200 OK            → { "ok": <Output> }
 *   4xx/5xx           → { "err": { "code": "...", "payload": ... } }
 *
 * `GET /rpc/<procedure>?input=<urlencoded-json>` is allowed for procedures
 * explicitly marked `method = "GET"` (per-call option via `methods` map).
 *
 * Subscriptions (SPEC §4.2) are NOT handled here — they live in `sse.ts`.
 * Calling `subscribe()` on this transport throws `TautError("invalid_kind", ...)`.
 */

import type { ProcedureKind, Transport } from "./index.js";

export class TautError<C extends string = string, P = unknown> extends Error {
  constructor(public readonly code: C, public readonly payload: P, public readonly httpStatus: number) {
    super(`${code}: ${typeof payload === "string" ? payload : JSON.stringify(payload)}`);
    this.name = "TautError";
  }
}

export interface HttpTransportOptions {
  url: string;
  fetch?: typeof fetch;
  headers?: Record<string, string> | (() => Record<string, string>);
  timeoutMs?: number;
  /** Per-call override map: procedure name → "GET" | "POST". */
  methods?: Record<string, "GET" | "POST">;
}

export class HttpTransport implements Transport {
  constructor(private opts: HttpTransportOptions) {}

  async call<I, O, E>(name: string, kind: ProcedureKind, input: I): Promise<O> {
    if (kind === "subscription") {
      throw new TautError("invalid_kind", "use SseTransport for subscriptions", 0);
    }
    const method = this.opts.methods?.[name] ?? "POST";
    const baseUrl = this.opts.url.replace(/\/+$/, "");
    const url = method === "GET"
      ? `${baseUrl}/rpc/${encodeURIComponent(name)}?input=${encodeURIComponent(JSON.stringify(input))}`
      : `${baseUrl}/rpc/${encodeURIComponent(name)}`;

    const init: RequestInit = {
      method,
      headers: { "content-type": "application/json", ...this.resolveHeaders() },
      body: method === "GET" ? undefined : JSON.stringify({ input }),
    };

    const ctrl = new AbortController();
    const timer = this.opts.timeoutMs ? setTimeout(() => ctrl.abort(), this.opts.timeoutMs) : undefined;
    init.signal = ctrl.signal;

    let resp: Response;
    try {
      resp = await (this.opts.fetch ?? globalThis.fetch)(url, init);
    } catch (err) {
      throw new TautError("transport_error", err instanceof Error ? err.message : String(err), 0);
    } finally {
      if (timer) clearTimeout(timer);
    }

    let body: unknown;
    try {
      body = await resp.json();
    } catch {
      throw new TautError("decode_error", `non-JSON response (status ${resp.status})`, resp.status);
    }

    if (resp.ok) {
      if (typeof body === "object" && body !== null && "ok" in body) {
        return (body as { ok: O }).ok;
      }
      throw new TautError("decode_error", "missing 'ok' field", resp.status);
    }

    if (typeof body === "object" && body !== null && "err" in body) {
      const env = (body as { err: { code: string; payload: unknown } }).err;
      throw new TautError(env.code, env.payload, resp.status);
    }
    throw new TautError("http_error", body, resp.status);
  }

  /** HTTP transport doesn't support subscriptions; delegate to SseTransport. */
  subscribe<I, O, E>(_name: string, _input: I): AsyncIterable<O> {
    throw new TautError("invalid_kind", "HttpTransport cannot subscribe — use SseTransport", 0);
  }

  private resolveHeaders(): Record<string, string> {
    const h = this.opts.headers;
    return typeof h === "function" ? h() : (h ?? {});
  }
}

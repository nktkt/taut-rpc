/**
 * SSE (Server-Sent Events) subscription transport for the taut-rpc runtime.
 *
 * Implements the wire format defined in SPEC §4.2:
 *
 *   GET /rpc/<procedure>?input=<urlencoded-json>
 *   Accept: text/event-stream
 *
 *   event: data\ndata: <json>\n\n
 *   event: error\ndata: <json>\n\n
 *   event: end\ndata:\n\n
 *
 * This transport handles subscription procedures only; query/mutation calls
 * should go through the HTTP transport. `call()` therefore rejects with
 * `invalid_kind`.
 *
 * We deliberately use `fetch` + `ReadableStream` rather than the browser's
 * built-in `EventSource`: `EventSource` cannot send custom request headers,
 * which makes it unsuitable for authenticated requests (Bearer tokens, CSRF
 * headers, etc.). The fetch-based reader gives us full control over the
 * request, at the cost of having to parse the SSE framing ourselves.
 *
 * Validation hook (SPEC §7): per-frame output validation is intentionally
 * NOT performed here. `createClient`'s proxy wraps the `AsyncIterable`
 * returned from `subscribe()` with a validating iterator (one `schema.parse()`
 * per yielded value) when `opts.schemas[name].output` is set and
 * `opts.validate.recv !== false`. Keeping this concern in the proxy lets
 * custom transport implementations stay simple and stateless — they only
 * need to deliver decoded frames; the proxy decides whether to validate.
 */

import type { ProcedureKind, Transport } from "./index.js";
import { TautError } from "./http.js";

export interface SseTransportOptions {
  url: string;
  fetch?: typeof fetch;
  headers?: Record<string, string> | (() => Record<string, string>);
}

/**
 * Subscription transport over Server-Sent Events.
 * Uses fetch + ReadableStream rather than EventSource so we can send headers.
 */
export class SseTransport implements Transport {
  constructor(private opts: SseTransportOptions) {}

  async call<I, O, E>(_name: string, _kind: ProcedureKind, _input: I): Promise<O> {
    throw new TautError("invalid_kind", "SseTransport handles subscriptions only", 0);
  }

  subscribe<I, O, E>(name: string, input: I): AsyncIterable<O> {
    const opts = this.opts;
    const baseUrl = opts.url.replace(/\/+$/, "");
    // Undefined input → JSON null so the URL has a parseable `?input=null`.
    // BigInts in the input are downcast to JS numbers so JSON.stringify accepts
    // them; values above 2^53 lose precision in this default mode (matches the
    // HTTP transport's behaviour).
    const inputJson = JSON.stringify(input ?? null, (_k, v) =>
      typeof v === "bigint" ? Number(v) : v,
    );
    const url = `${baseUrl}/rpc/${encodeURIComponent(name)}?input=${encodeURIComponent(inputJson)}`;

    // AbortController is created per-iteration so each `for await` loop owns
    // its own cancellation handle. We expose abort() through the iterator's
    // `return()` method (invoked by `break`, early `return`, or `throw`
    // inside the consumer's loop), which cancels the in-flight fetch and the
    // underlying ReadableStream — preventing leaked sockets on early exit.
    return {
      [Symbol.asyncIterator]: (): AsyncIterator<O> => {
        const controller = new AbortController();
        const inner = consume<O>(url, opts, controller.signal);
        return {
          next: () => inner.next(),
          return: async (value?: O): Promise<IteratorResult<O>> => {
            controller.abort();
            // Drain the inner generator so its `finally` runs and the reader
            // releases its lock. The fetch abort surfaces as an AbortError on
            // the in-flight read; swallow it on the cancellation path so
            // `break` is clean.
            try {
              await inner.return();
            } catch {
              /* ignore abort propagation */
            }
            return { value: value as O, done: true };
          },
          throw: async (err?: unknown): Promise<IteratorResult<O>> => {
            controller.abort();
            // Forward to the inner generator so its finally runs, then
            // re-throw to honor the iterator-protocol contract.
            try {
              return await inner.throw(err);
            } catch (e) {
              throw e;
            }
          },
        };
      },
    };
  }
}

/**
 * Internal: drive the SSE wire format as an async generator.
 *
 * Lives outside `subscribe` so it can be returned as a real `AsyncGenerator`
 * (with proper `return`/`throw` plumbing). The outer iterator wraps this and
 * adds an AbortController so consumer-driven cancellation actually cancels
 * the in-flight fetch.
 */
async function* consume<O>(
  url: string,
  opts: SseTransportOptions,
  signal: AbortSignal,
): AsyncGenerator<O, void, unknown> {
  const headers = new Headers({ accept: "text/event-stream" });
  const extra = typeof opts.headers === "function" ? opts.headers() : (opts.headers ?? {});
  for (const [k, v] of Object.entries(extra)) headers.set(k, v);

  const resp = await (opts.fetch ?? globalThis.fetch)(url, { method: "GET", headers, signal });
  if (!resp.ok || !resp.body) {
    let payload: unknown = await resp.text().catch(() => "");
    throw new TautError("transport_error", payload, resp.status);
  }

  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });

      // Split SSE events on double-newline.
      let idx: number;
      while ((idx = buffer.indexOf("\n\n")) >= 0) {
        const raw = buffer.slice(0, idx);
        buffer = buffer.slice(idx + 2);
        const parsed = parseEvent(raw);
        if (!parsed) continue;
        if (parsed.event === "data") {
          yield JSON.parse(parsed.data) as O;
        } else if (parsed.event === "error") {
          const err = JSON.parse(parsed.data) as { code: string; payload: unknown };
          throw new TautError(err.code, err.payload, 0);
        } else if (parsed.event === "end") {
          return;
        }
        // Other event types are silently ignored (SPEC §4.2 lists exactly
        // three, but a forward-compatible parser shouldn't blow up).
      }
    }
  } finally {
    // cancel() releases the lock and signals the underlying stream to stop
    // pulling from the network. Safe to call even after abort: by then the
    // body is already errored and cancel() resolves quickly.
    try { await reader.cancel(); } catch { /* ignore */ }
    reader.releaseLock?.();
  }
}

interface ParsedEvent { event: string; data: string; }

function parseEvent(raw: string): ParsedEvent | null {
  let event = "message";
  const lines: string[] = [];
  for (const line of raw.split("\n")) {
    if (line.startsWith(":")) continue; // comment line
    const i = line.indexOf(":");
    if (i < 0) continue;
    const field = line.slice(0, i).trim();
    const value = line.slice(i + 1).replace(/^ /, "");
    if (field === "event") event = value;
    else if (field === "data") lines.push(value);
  }
  if (lines.length === 0 && event !== "end") return null;
  return { event, data: lines.join("\n") };
}

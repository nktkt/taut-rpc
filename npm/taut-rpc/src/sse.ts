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
    const url = `${baseUrl}/rpc/${encodeURIComponent(name)}?input=${encodeURIComponent(JSON.stringify(input))}`;

    return {
      [Symbol.asyncIterator]: async function* () {
        const headers = new Headers({ accept: "text/event-stream" });
        const extra = typeof opts.headers === "function" ? opts.headers() : (opts.headers ?? {});
        for (const [k, v] of Object.entries(extra)) headers.set(k, v);

        const resp = await (opts.fetch ?? globalThis.fetch)(url, { method: "GET", headers });
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
            }
          }
        } finally {
          reader.releaseLock?.();
        }
      },
    };
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

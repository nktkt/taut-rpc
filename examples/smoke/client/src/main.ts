// Phase 0 hand-written smoke client.
//
// Every type and every transport call is written longhand. No imports from
// taut-rpc — the runtime crate doesn't exist yet. The point is to validate
// that the wire format from SPEC.md §4 is round-trippable from a TS process
// that knows nothing about Rust.

const BASE_URL = "http://127.0.0.1:7700/rpc";

// --- Hand-written procedure types -----------------------------------------

interface AddInput {
  a: number;
  b: number;
}
type AddOutput = number;

interface GetUserInput {
  id: number;
}
interface User {
  id: number;
  name: string;
}

// --- Wire envelope (SPEC §4.1) --------------------------------------------

type ApiError<C extends string = string, P = unknown> = {
  code: C;
  payload: P;
};

type Envelope<T> = { ok: T } | { err: ApiError };

class RpcError extends Error {
  constructor(
    readonly code: string,
    readonly payload: unknown,
    readonly status: number,
  ) {
    super(`rpc error: ${code}`);
  }
}

async function call<I, O>(name: string, input: I): Promise<O> {
  const res = await fetch(`${BASE_URL}/${name}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ input }),
  });

  let body: Envelope<O>;
  try {
    body = (await res.json()) as Envelope<O>;
  } catch {
    throw new Error(`rpc ${name}: non-JSON response (status ${res.status})`);
  }

  if ("err" in body) {
    throw new RpcError(body.err.code, body.err.payload, res.status);
  }
  if (!res.ok) {
    throw new Error(`rpc ${name}: status ${res.status} but no err envelope`);
  }
  return body.ok;
}

// --- Hand-written SSE consumer (SPEC §4.2) --------------------------------
//
// Parses `event: <name>\ndata: <payload>\n\n` frames out of a fetch
// ReadableStream. Yields { event, data } objects until the stream closes.

async function* sse(
  url: string,
): AsyncGenerator<{ event: string; data: string }, void, void> {
  const res = await fetch(url, { headers: { accept: "text/event-stream" } });
  if (!res.ok || !res.body) {
    throw new Error(`sse ${url}: status ${res.status}`);
  }

  const reader = res.body.getReader();
  const decoder = new TextDecoder("utf-8");
  let buf = "";

  while (true) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });

    // SSE frames are terminated by a blank line (\n\n).
    let sep = buf.indexOf("\n\n");
    while (sep !== -1) {
      const raw = buf.slice(0, sep);
      buf = buf.slice(sep + 2);
      const frame = parseFrame(raw);
      if (frame) yield frame;
      sep = buf.indexOf("\n\n");
    }
  }
}

function parseFrame(
  raw: string,
): { event: string; data: string } | null {
  let event = "message";
  const dataLines: string[] = [];
  for (const line of raw.split("\n")) {
    if (line.startsWith(":") || line.length === 0) continue;
    const idx = line.indexOf(":");
    const field = idx === -1 ? line : line.slice(0, idx);
    const value =
      idx === -1
        ? ""
        : line.slice(idx + 1).startsWith(" ")
          ? line.slice(idx + 2)
          : line.slice(idx + 1);
    if (field === "event") event = value;
    else if (field === "data") dataLines.push(value);
  }
  if (dataLines.length === 0 && event === "message") return null;
  return { event, data: dataLines.join("\n") };
}

// --- Run sequence ---------------------------------------------------------

async function main(): Promise<void> {
  const pong = await call<undefined, string>("ping", undefined);
  console.log(`ping        -> ${pong}`);

  const sum = await call<AddInput, AddOutput>("add", { a: 2, b: 3 });
  console.log(`add(2, 3)   -> ${sum}`);

  const ada = await call<GetUserInput, User>("get_user", { id: 1 });
  console.log(`get_user(1) -> { id: ${ada.id}, name: '${ada.name}' }`);

  try {
    await call<GetUserInput, User>("get_user", { id: 999 });
    console.log("get_user(999) -> unexpected ok");
    process.exitCode = 1;
  } catch (e) {
    if (e instanceof RpcError) {
      console.log(`get_user(999) -> err ${e.code}`);
    } else {
      throw e;
    }
  }

  for await (const evt of sse(`${BASE_URL}/tick`)) {
    if (evt.event === "data") {
      console.log(`tick: ${evt.data}`);
    } else if (evt.event === "end") {
      console.log("tick: end");
      break;
    } else if (evt.event === "error") {
      console.log(`tick: error ${evt.data}`);
      process.exitCode = 1;
      break;
    }
  }

  console.log("done");
}

main().catch((e: unknown) => {
  console.error(e);
  process.exit(1);
});

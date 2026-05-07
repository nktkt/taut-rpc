// Phase 3 example client.
//
// Imports the GENERATED `./api.gen.ts` and consumes the server's streaming
// procedures via `for await`. The runtime exposes subscriptions as a
// `.subscribe(input)` method that returns an `AsyncIterable<T>`, mirroring
// SPEC §6's `for await (const evt of client.userEvents.subscribe(...))`
// shape.
//
// Run order:
//
//   1. cd examples/phase3-counter/server && cargo build
//   2. (from repo root) cargo run -p taut-rpc-cli -- taut gen \
//        --from-binary examples/phase3-counter/server/target/debug/phase3-counter-server \
//        --out         examples/phase3-counter/client/src/api.gen.ts
//   3. cd examples/phase3-counter/server && cargo run        (in one terminal)
//   4. cd examples/phase3-counter/client && npm run start    (in another)
//
// Until step 2 has run, `./api.gen.js` does not exist and TypeScript will
// flag the import — that is the entire point of this example: the generated
// file is the contract.

// `.js` extension because tsconfig is set to NodeNext / ESM. Codegen output.
import { createApi } from "./api.gen.js";
import { TautError } from "taut-rpc";

// HTTP transport already prefixes `/rpc/<name>` (and uses GET + SSE for
// subscriptions per SPEC §4.2), so the URL is the origin only.
const client = createApi({ url: "http://127.0.0.1:7704" });

async function main(): Promise<void> {
  // Plain unary call — confirms the unary path still works alongside streams.
  console.log("ping:", await client.ping());

  // The headline: a 5-tick counter, one per second. `count` and `interval_ms`
  // are u64 on the server (SPEC §3.1) so they cross the wire as bigints.
  console.log("ticks (interval 1000ms, count 5):");
  for await (const tick of client.ticks.subscribe({
    count: 5n,
    interval_ms: 1000n,
  })) {
    console.log("  tick:", tick);
  }

  // Zero-input subscription: codegen drops the input parameter from
  // `.subscribe()`. Three ISO-8601 timestamps, one per second.
  console.log("server_time:");
  for await (const t of client.server_time.subscribe()) {
    console.log("  t:", t);
  }

  // Phase 4: `TicksInput` carries `#[derive(taut_rpc::Validate)]` with
  // `count` capped at 100 and `interval_ms` constrained to `[10, 60_000]`.
  // The server rejects out-of-range inputs before the stream is opened, so
  // the client never receives a partial sequence — instead the
  // `subscribe()` call throws a `TautError` with `code === "validation_error"`.
  // We exercise that path here with `count = 200n` (above the max of 100).
  try {
    for await (const _ of client.ticks.subscribe({
      count: 200n,
      interval_ms: 1000n,
    })) {
      // Should be unreachable: the server validates before yielding any
      // values, so the SSE stream never opens.
    }
    console.error("expected validation_error, got success");
    process.exit(1);
  } catch (e: unknown) {
    if (e instanceof TautError) {
      console.log("rejected (count=200):", e.code);
    } else {
      throw e;
    }
  }

  console.log("done");
}

main().catch((e: unknown) => {
  console.error(e);
  process.exit(1);
});

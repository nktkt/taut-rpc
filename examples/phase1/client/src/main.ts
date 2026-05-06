// Phase 1 example client.
//
// Imports the GENERATED `./api.gen.ts` and uses it to call the Phase 1 server's
// procedures. The generated module exports `createApi(opts) => ClientOf<...>`,
// a thin wrapper around `createClient` from the `taut-rpc` runtime that bakes
// in the project's `Procedures` type-map.
//
// Run order:
//
//   1. cd examples/phase1/server && cargo build
//   2. (from repo root) cargo run -p taut-rpc-cli -- taut gen \
//        --from-binary examples/phase1/server/target/debug/phase1-server \
//        --out         examples/phase1/client/src/api.gen.ts
//   3. cd examples/phase1/server && cargo run        (in one terminal)
//   4. cd examples/phase1/client && npm run start    (in another)
//
// Until step 2 has run, `./api.gen.js` does not exist and TypeScript will
// flag the import — that is the entire point of this example: the generated
// file is the contract.

// `.js` extension because tsconfig is set to NodeNext / ESM. The runtime file
// is emitted by `cargo taut gen` as `src/api.gen.ts` and TS resolves the
// `./api.gen.js` specifier back to it.
import { createApi } from "./api.gen.js"; // codegen output — see steps above.

// Note: HTTP transport already prefixes `/rpc/<name>`, so the URL is the
// origin only — not the procedure root.
const client = createApi({ url: "http://127.0.0.1:7701" });

async function main(): Promise<void> {
  // ping(): zero-input, success-only.
  console.log(await client.ping());

  // add({ a, b }): success path.
  console.log(await client.add({ a: 2, b: 3 }));

  // get_user({ id }): success path. v0.1 codegen does NOT translate snake_case
  // to camelCase, so the property name on the client matches the Rust fn name.
  // Use bracket syntax to keep TypeScript happy with the underscore.
  console.log(await client["get_user"]({ id: 1n }));

  // add overflow: surfaces as a thrown error whose `.code` is the tagged
  // discriminant the Rust enum serialised with (`overflow`, snake_case).
  try {
    await client.add({ a: 2147483647, b: 1 });
    console.log("err: expected overflow but call succeeded");
    process.exitCode = 1;
  } catch (e: any) {
    console.log("err:", e.code);
  }
}

main().catch((e: unknown) => {
  console.error(e);
  process.exit(1);
});

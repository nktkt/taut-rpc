// Phase 4 example client.
//
// Imports the GENERATED `./api.gen.ts` and exercises the full validation
// pipeline from SPEC §7: client-side input parse before the network call,
// server-side parse before the procedure body, and the TS error envelope
// (`code = "validation_error"`) on either side.
//
// Run order:
//
//   1. cd examples/phase4-validate/server && cargo build
//   2. (from repo root) cargo run -p taut-rpc-cli -- taut gen \
//        --from-binary examples/phase4-validate/server/target/debug/phase4-validate-server \
//        --out         examples/phase4-validate/client/src/api.gen.ts
//   3. cd examples/phase4-validate/server && cargo run        (in one terminal)
//   4. cd examples/phase4-validate/client && npm run start    (in another)
//
// Until step 2 has run, `./api.gen.js` does not exist and TypeScript will
// flag the import — that is the entire point of this example: the generated
// file is the contract.

// `.js` extension because tsconfig is set to NodeNext / ESM. Codegen output.
import { createApi, procedureSchemas } from "./api.gen.js";
import { isTautError } from "taut-rpc";

// HTTP transport already prefixes `/rpc/<name>`, so the URL is the origin
// only. `procedureSchemas` is the per-procedure Valibot schema map emitted
// by codegen; passing it in turns on validation in both directions
// (`validate.send` / `validate.recv` default to `true` when schemas are
// supplied).
const client = createApi({
  url: "http://127.0.0.1:7705",
  schemas: procedureSchemas,
  validate: { send: true, recv: true },
});

async function main() {
  console.log("ping:", await client.ping());

  // Success
  const user = await client.create_user({
    username: "alice",
    email: "alice@example.com",
    age: 30,
    handle: "alice_42",
    homepage: "https://alice.example.com",
  });
  console.log("created:", user);

  // Server-side rejection (username taken)
  try {
    await client.create_user({
      username: "taken",
      email: "x@y.com",
      age: 22,
      handle: "x",
      homepage: "https://x.com",
    });
  } catch (e) {
    if (isTautError(e, "username_taken")) console.log("server rejected: username_taken");
    else throw e;
  }

  // Client-side validation: invalid email — fails BEFORE network call
  try {
    await client.create_user({
      username: "alice",
      email: "not an email",
      age: 30,
      handle: "alice",
      homepage: "https://alice.com",
    });
    console.log("ERROR: should have been rejected client-side");
  } catch (e) {
    if (isTautError(e, "validation_error")) console.log("client rejected:", e.payload);
    else throw e;
  }

  // Server-side validation: bypass client (set validate.send=false on a separate client)
  const noClientCheck = createApi({
    url: "http://127.0.0.1:7705",
    schemas: procedureSchemas,
    validate: { send: false },
  });
  try {
    await noClientCheck.create_user({
      username: "ab",  // too short
      email: "ok@ok.co",
      age: 30,
      handle: "ok",
      homepage: "https://ok.co",
    });
  } catch (e) {
    if (isTautError(e, "validation_error")) console.log("server rejected:", e.payload);
    else throw e;
  }
}

main().catch(e => { console.error(e); process.exit(1); });

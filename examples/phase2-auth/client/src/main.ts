// Phase 2 example client.
//
// Demonstrates the TS side of the SPEC §3.3 typed-error contract paired with
// the SPEC §5 `tower::Layer` middleware. The Rust server short-circuits
// unauthenticated requests at the auth layer (HTTP 401, code
// `"unauthenticated"`) and returns `AuthError::Forbidden { required_role }`
// for non-admin callers (HTTP 403, code `"forbidden"`). Both reach the client
// as the same `TautError` runtime type, narrowed on `e.code`.
//
// Run order:
//
//   1. cd examples/phase2-auth/server && cargo build
//   2. (from repo root) cargo run -p taut-rpc-cli -- taut gen \
//        --from-binary examples/phase2-auth/server/target/debug/phase2-auth-server \
//        --out         examples/phase2-auth/client/src/api.gen.ts
//   3. cd examples/phase2-auth/server && cargo run        (in one terminal)
//   4. cd examples/phase2-auth/client && npm run start    (in another)

// `.js` extension because tsconfig is set to NodeNext / ESM. Codegen output.
import { createApi } from "./api.gen.js";
import { isTautError } from "taut-rpc";

const URL = "http://127.0.0.1:7702";

// Three clients, each with a different `Authorization` header config. The
// `headers` field on `ClientOptions` is forwarded by the HTTP transport on
// every request, which is exactly the contract the server's auth layer
// inspects.
const anonymous = createApi({ url: URL });
const alpha = createApi({
  url: URL,
  headers: { authorization: "Bearer alpha" },
});
const admin = createApi({
  url: URL,
  headers: { authorization: "Bearer admin" },
});

async function main(): Promise<void> {
  // 1. Public ping — no auth required, succeeds for the anonymous client.
  console.log("ping:", await anonymous.ping());

  // 2. whoami without a token — the auth layer short-circuits with 401 +
  // `unauthenticated` before the procedure runs.
  try {
    await anonymous.whoami();
    console.log("err: expected unauthenticated but call succeeded");
    process.exitCode = 1;
  } catch (e: unknown) {
    if (isTautError(e, "unauthenticated")) {
      console.log("whoami (anonymous) rejected:", e.code);
    } else {
      throw e;
    }
  }

  // 3. whoami with a valid token — auth layer waves it through.
  console.log("whoami (alpha):", await alpha.whoami());

  // 4. get_secret as a non-admin — auth layer rejects with 403 + `forbidden`,
  // payload tells the client which role would have unblocked them.
  try {
    await alpha.get_secret();
    console.log("err: expected forbidden but call succeeded");
    process.exitCode = 1;
  } catch (e: unknown) {
    if (isTautError(e, "forbidden")) {
      // Narrowed: e.code is "forbidden", e.payload carries the typed shape
      // declared by `AuthError::Forbidden { required_role }` on the server.
      console.log("get_secret (alpha) rejected:", e.code, e.payload);
    } else {
      throw e;
    }
  }

  // 5. get_secret as admin — succeeds, returns the canned secret.
  console.log("get_secret (admin):", await admin.get_secret());
}

main().catch((e: unknown) => {
  console.error(e);
  process.exit(1);
});

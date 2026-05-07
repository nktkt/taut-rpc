// Vite config for the counter demo.
//
// Two responsibilities:
//
//   1. Wire SvelteKit in via `@sveltejs/kit/vite`. This is the standard
//      SvelteKit Vite plugin and there's nothing custom about it here.
//   2. Proxy `/rpc/*` to the Rust server on port 7712 during `vite dev`.
//
// The proxy keeps the client's runtime URL relative (`/rpc`) so the same
// build works against `vite preview`, the static export served by any HTTP
// server, or a future deployment where Rust and SvelteKit live behind a
// single reverse proxy. `ws: true` is necessary for SSE — Vite's dev proxy
// otherwise buffers the response and breaks the chunked-encoding stream
// `live()` relies on.

import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    proxy: {
      "/rpc": {
        target: "http://127.0.0.1:7712",
        changeOrigin: true,
        ws: true,
      },
    },
  },
});

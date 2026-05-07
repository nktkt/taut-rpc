// Vite config for the Phase 5 Todo client.
//
// The dev server binds 7711 (server runs on 7710 — kept on different ports
// so both can run side-by-side and the client is free to use a relative
// `/rpc` URL via the proxy below). Anything under `/rpc/*` is forwarded to
// the Rust server, including the SSE subscription endpoint — `ws: false`
// is intentional, taut-rpc subscriptions are SSE not WebSocket and only
// need ordinary HTTP proxying.

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  server: {
    port: 7711,
    strictPort: true,
    proxy: {
      "/rpc": {
        target: "http://127.0.0.1:7710",
        changeOrigin: true,
      },
    },
  },
});

// SvelteKit configuration for the counter demo.
//
// We use `adapter-static` because this example has no SSR concerns: every
// page is a single SPA that talks to the Rust server over HTTP/SSE on a
// different port. Static export keeps the build artefact a flat folder of
// HTML/JS that any web server (or `vite preview`) can serve.
//
// `fallback: "index.html"` is the SPA-mode escape hatch — without it, the
// adapter would try to prerender every route and fail on dynamic ones.
// With it, every unmatched URL falls back to `index.html` and SvelteKit's
// client-side router takes over.

import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),
  kit: {
    adapter: adapter({
      pages: "build",
      assets: "build",
      fallback: "index.html",
      precompress: false,
      strict: true,
    }),
  },
};

export default config;

// SPA-mode toggle for the static adapter.
//
// `ssr: false` disables server-side rendering — the demo only ever runs as a
// browser-side SPA against the Rust server, and SSR would try to import the
// generated `api.gen.ts` (which uses `bigint` literals) at build time.
//
// `prerender: false` keeps the static adapter from materialising every
// route; combined with `fallback: "index.html"` in `svelte.config.js`,
// the build is a single SPA shell.
export const ssr = false;
export const prerender = false;

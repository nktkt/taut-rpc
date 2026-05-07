# Deployment

This chapter covers shipping a `taut-rpc` server to production: building a release
binary, packaging it into a container, deploying to common platforms, and the
operational concerns (TLS, CORS, health checks, observability) that come with
running a long-lived RPC service.

## 1. Build a release binary

The simplest production build is `cargo build --release`. This produces an
optimised, unstripped binary at `target/release/<bin-name>`.

```bash
cargo build --release
ls -lh target/release/your-server
# -rwxr-xr-x  ... 28M target/release/your-server
```

A 20-40 MB debug-info-laden binary is fine for most uses, but if size matters
(small VMs, container layers) strip it:

```bash
strip target/release/your-server
# Or, cross-platform:
cargo install cargo-strip
cargo strip
```

You can also enable strip in `Cargo.toml` so it happens on every release build:

```toml
[profile.release]
strip = "symbols"      # or "debuginfo" to keep symbols
lto = "thin"           # smaller + faster
codegen-units = 1
panic = "abort"        # only if you don't catch panics in handlers
```

### Static linking with musl on Linux

For a fully static binary that runs on any Linux distro (including
`scratch`-based containers) build against the musl target:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

If you depend on crates that link C (e.g. `ring` for rustls), install the musl
toolchain (`brew install FiloSottile/musl-cross/musl-cross` on macOS, or use the
`messense/rust-musl-cross` Docker image in CI).

## 2. Dockerfile (multi-stage)

A plain multi-stage Dockerfile that caches dependencies separately from app
sources:

```dockerfile
# syntax=docker/dockerfile:1.6
FROM rust:1.81 AS builder
WORKDIR /app

# Cache deps: copy manifests, build a stub, then replace with real sources.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin your-server && \
    cp target/release/your-server /usr/local/bin/your-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/bin/your-server /usr/local/bin/your-server
EXPOSE 8080
ENV RUST_LOG=info SERVER_PORT=8080
CMD ["your-server"]
```

For faster CI builds, use [`cargo chef`](https://github.com/LukeMathWalker/cargo-chef)
to compute a recipe of dependency manifests so the dependency layer rebuilds
only when `Cargo.lock` changes:

```dockerfile
FROM rust:1.81 AS chef
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin your-server

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/your-server /usr/local/bin/your-server
CMD ["your-server"]
```

## 3. fly.io

A minimal `fly.toml` for a taut-rpc server:

```toml
app = "your-app"
primary_region = "nrt"

[build]
  dockerfile = "Dockerfile"

[env]
  RUST_LOG = "info,tower_http=debug"
  SERVER_PORT = "8080"

[http_service]
  internal_port = 8080
  force_https = true
  auto_stop_machines = true
  auto_start_machines = true
  min_machines_running = 1

  [[http_service.checks]]
    interval = "10s"
    timeout = "2s"
    grace_period = "5s"
    method = "GET"
    path = "/rpc/_health"

[[vm]]
  cpu_kind = "shared"
  cpus = 1
  memory_mb = 512
```

Deploy with `fly deploy`. The first deploy provisions a machine and a public
IPv4/IPv6; subsequent deploys do rolling restarts.

## 4. Other platforms

- **Railway** — point it at the repo, set `SERVER_PORT=$PORT`, add `RUST_LOG`,
  and Railway's Nixpacks builder will detect Cargo and build automatically.
  For faster builds and reproducibility, commit a Dockerfile.
- **Render** — supports both Docker and native Rust. Set the start command to
  your binary and configure the health check path to `/rpc/_health`.
- **Fly Machines (lower-level)** — `flyctl machine run` for ad-hoc instances.
  Useful for cron-style background workers that share the same image as the
  server.
- **AWS / GCP / Fargate** — deploy the Docker image. Put an ALB / Cloud Load
  Balancer in front for TLS termination and health checks.

## 5. TLS

You have two reasonable options:

### Behind a reverse proxy (recommended)

Let nginx, Caddy, or your platform's load balancer terminate TLS and forward
plaintext HTTP to the taut-rpc server on a private port. Caddy is the lowest
overhead:

```caddyfile
your.app {
    reverse_proxy localhost:8080
}
```

Caddy auto-provisions Let's Encrypt certs. nginx works equivalently with
`proxy_pass http://127.0.0.1:8080;` plus a TLS block.

### Direct with `axum-server`

If you want the binary to terminate TLS itself (single-binary deploys, edge
boxes), swap the listener for [`axum-server`](https://crates.io/crates/axum-server):

```rust
use axum_server::tls_rustls::RustlsConfig;

let tls = RustlsConfig::from_pem_file("cert.pem", "key.pem").await?;
axum_server::bind_rustls("0.0.0.0:8443".parse()?, tls)
    .serve(app.into_make_service())
    .await?;
```

You'll need to handle cert renewal yourself (e.g. a sidecar `certbot` cron, or
the `rustls-acme` crate for in-process ACME).

## 6. Serving the SPA from the same binary

If you ship the generated `api.gen.ts` to a Vite/Next/SolidStart SPA, you can
serve the built static assets from the same axum binary using
`tower_http::services::ServeDir`. The `.fallback_service` pattern routes
unmatched paths to the SPA, so client-side routing works:

```rust
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};

let rpc_router = taut_rpc_axum::router(state.clone());

let static_dir = ServeDir::new("./dist")
    .not_found_service(ServeFile::new("./dist/index.html"));

let app = Router::new()
    .nest("/rpc", rpc_router)
    .fallback_service(static_dir);
```

In your Dockerfile, copy `dist/` from a Node build stage:

```dockerfile
FROM node:20 AS frontend
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ .
RUN npm run build         # outputs dist/

# ... rust builder stage as above ...

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/your-server /usr/local/bin/your-server
COPY --from=frontend /web/dist /var/lib/your-server/dist
WORKDIR /var/lib/your-server
CMD ["your-server"]
```

## 7. Environment variables

Read configuration from the environment so the same binary works in every
deploy target:

| Variable      | Purpose                                       | Example                  |
|---------------|-----------------------------------------------|--------------------------|
| `SERVER_PORT` | Port to bind                                  | `8080`                   |
| `SERVER_HOST` | Bind address                                  | `0.0.0.0`                |
| `RUST_LOG`    | `tracing-subscriber` filter                   | `info,tower_http=debug`  |
| `DATABASE_URL`| Postgres connection string                    | `postgres://...`         |
| `CORS_ORIGINS`| Comma-separated allow-list                    | `https://your.app`       |

Parse with [`figment`](https://crates.io/crates/figment) or plain `std::env`:

```rust
let port: u16 = std::env::var("SERVER_PORT")
    .ok()
    .and_then(|s| s.parse().ok())
    .unwrap_or(8080);
```

## 8. Health checks

`taut-rpc-axum` mounts `GET /rpc/_health` automatically. It returns `200 OK`
with a small JSON body and does no I/O, so it's safe to hit from a load
balancer every few seconds.

Wire it up to your platform:

- **fly.toml** — `path = "/rpc/_health"` (see above)
- **Kubernetes** — `livenessProbe.httpGet.path: /rpc/_health`
- **ALB target group** — health check path `/rpc/_health`, matcher `200`

If you need a deeper readiness probe (DB reachable, migrations applied), add
your own handler at `/rpc/_ready` and point startup probes there while keeping
liveness on `/rpc/_health`.

## 9. CORS for separate origins

If the SPA is served from a different origin than the RPC server (e.g.
`app.your.app` and `api.your.app`), add a CORS layer.

```rust
use tower_http::cors::{Any, CorsLayer};
use http::Method;

#[cfg(debug_assertions)]
let cors = CorsLayer::permissive();

#[cfg(not(debug_assertions))]
let cors = CorsLayer::new()
    .allow_origin(["https://your.app".parse()?])
    .allow_methods([Method::GET, Method::POST])
    .allow_headers(Any)
    .allow_credentials(true);

let app = Router::new()
    .nest("/rpc", rpc_router)
    .layer(cors);
```

`CorsLayer::permissive()` allows any origin/method/header — fine for local dev,
never ship it to production.

## 10. Observability

Use `tracing-subscriber` for structured logs and `tower_http::trace::TraceLayer`
for one log line per request:

```rust
use tower_http::trace::TraceLayer;
use tracing_subscriber::{EnvFilter, fmt};

fmt()
    .with_env_filter(EnvFilter::from_default_env())
    .json()
    .init();

let app = Router::new()
    .nest("/rpc", rpc_router)
    .layer(TraceLayer::new_for_http());
```

For OpenTelemetry export (Jaeger, Honeycomb, Datadog), see the
[`phase2-tracing`](../examples/phase2-tracing.md) example which shows the full
`tracing-opentelemetry` plumbing.

## 11. Health checks and subscriptions

Long-lived SSE subscriptions need keep-alive frames or upstream proxies will
close the connection at their idle timeout (60s on most ALBs, 75s on nginx by
default).

`taut-rpc-axum`'s subscription transport sends a `:keep-alive` comment every
15 seconds by default. If you front the server with a custom proxy, raise the
proxy's read timeout above that:

```nginx
location /rpc/ {
    proxy_pass http://127.0.0.1:8080;
    proxy_http_version 1.1;
    proxy_set_header Connection "";
    proxy_buffering off;
    proxy_read_timeout 1h;          # for SSE
}
```

For load balancer health checks, hit `/rpc/_health` (a normal request), not a
subscription endpoint.

## 12. The `dump_if_requested` step

`taut-rpc` generates the TypeScript client from a JSON description that the
server itself can emit when started with `--dump-spec` (or
`TAUT_RPC_DUMP_SPEC=1`). This must run **before** the server binds the port —
in CI/build, not at runtime.

A typical pre-build step:

```bash
# Build the binary in dump mode.
cargo run --release --bin your-server -- --dump-spec > spec.json

# Generate the client.
npx taut-rpc-codegen spec.json --out web/src/api.gen.ts

# Build the SPA.
(cd web && npm ci && npm run build)

# Then build the production server image.
docker build -t your-server .
```

In code, the call site looks like:

```rust
fn main() -> anyhow::Result<()> {
    let server = build_server()?;
    server.dump_if_requested()?;   // exits if --dump-spec was passed
    runtime().block_on(server.serve())?;
    Ok(())
}
```

`dump_if_requested` returns `Ok(())` and does nothing in normal runs; with
`--dump-spec` it writes to stdout and calls `std::process::exit(0)`. Skipping
it means your client and server can drift, so wire it into every CI pipeline.

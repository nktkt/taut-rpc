//! Library surface of `taut-rpc-cli`. The crate ships both a `cargo-taut`
//! binary (see `src/main.rs`) and this library, which exposes the codegen
//! module so other crates (notably `taut-rpc`'s integration tests) can call
//! `render_ts` directly without spawning the binary.

pub mod codegen;

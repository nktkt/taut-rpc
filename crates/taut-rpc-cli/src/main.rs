//! `cargo taut` — the codegen and IR-inspection driver for `taut-rpc`.
//!
//! This binary is named `cargo-taut` so that Cargo will discover it as the
//! `cargo taut` subcommand (see the Cargo book on custom subcommands). When
//! invoked through Cargo, the first positional argument is the literal string
//! `"taut"`; we transparently strip it so users can run either of:
//!
//! ```text
//! cargo taut gen --validator valibot
//! cargo-taut gen --validator valibot
//! ```
//!
//! Subcommands map 1:1 to the phases described in `ROADMAP.md`:
//!
//! - [`Cmd::Gen`]      — Phase 1: read the IR, emit `api.gen.ts`.
//! - [`Cmd::Check`]    — Phase 5: detect IR drift in CI.
//! - [`Cmd::Inspect`]  — Phase 5: pretty-print the IR for humans.
//! - [`Cmd::Mcp`]      — Beyond 0.1: emit an MCP `tools/list` manifest.
//!
//! See `SPEC.md` §2 for the macro/IR/codegen architecture.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod codegen;
mod commands;
mod mcp;

/// `cargo taut` — codegen and IR tooling for `taut-rpc`.
///
/// The IR (`target/taut/ir.json`) is produced by the `#[rpc]` / `#[derive(Type)]`
/// proc-macros at compile time. This CLI consumes that IR.
#[derive(Debug, Parser)]
#[command(
    name = "cargo-taut",
    bin_name = "cargo taut",
    version,
    about = "Codegen and IR tooling for taut-rpc.",
    long_about = "Reads the taut-rpc IR (target/taut/ir.json) emitted by the \
                  proc-macros and either generates a typed TypeScript client, \
                  checks the IR against an expected version, or inspects it."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
enum Cmd {
    /// Generate a TypeScript client from the IR (Phase 1).
    Gen(commands::gen::GenArgs),

    /// Check the IR for drift / version mismatches (Phase 5, CI use case).
    Check(commands::check::CheckArgs),

    /// Inspect the IR as a human-readable table or JSON (Phase 5).
    Inspect(commands::inspect::InspectArgs),

    /// Emit an MCP (Model Context Protocol) `tools/list` manifest.
    Mcp(commands::mcp::McpArgs),
}

fn main() -> Result<()> {
    // When invoked as `cargo taut <sub>`, Cargo passes argv as
    // ["cargo-taut", "taut", "<sub>", ...]. When invoked directly as
    // `cargo-taut <sub>`, argv is ["cargo-taut", "<sub>", ...]. Normalize.
    let mut argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if argv.get(1).is_some_and(|a| a == "taut") {
        argv.remove(1);
    }

    let cli = Cli::parse_from(argv);
    match cli.cmd {
        Cmd::Gen(args) => commands::gen::run(args),
        Cmd::Check(args) => commands::check::run(args),
        Cmd::Inspect(args) => commands::inspect::run(args),
        Cmd::Mcp(args) => commands::mcp::run(args),
    }
}

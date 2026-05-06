//! `cargo taut inspect` — render the IR for humans.
//!
//! See `ROADMAP.md` Phase 5.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, ValueEnum};

/// Output format for `cargo taut inspect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InspectFormat {
    /// Human-readable table (default).
    Table,
    /// Pretty-printed JSON, suitable for piping into `jq`.
    Json,
}

/// Arguments for `cargo taut inspect`.
#[derive(Debug, Args)]
#[command(
    about = "Render the IR as a human-readable table or JSON.",
    long_about = "Loads target/taut/ir.json (or the path passed to --ir) and \
                  prints procedures, types, and errors in a format suitable for \
                  eyeballing during development."
)]
pub struct InspectArgs {
    /// Path to the IR JSON file to render.
    #[arg(long, value_name = "PATH", default_value = "target/taut/ir.json")]
    pub ir: PathBuf,

    /// Output format.
    #[arg(long, value_name = "FMT", value_enum, default_value_t = InspectFormat::Table)]
    pub format: InspectFormat,
}

/// Entry point for `cargo taut inspect`.
pub fn run(args: InspectArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let ir_abs = if args.ir.is_absolute() {
        args.ir.clone()
    } else {
        cwd.join(&args.ir)
    };

    println!("unimplemented: see ROADMAP Phase 5");
    println!("  would read IR from:  {}", ir_abs.display());
    println!("  format:              {:?}", args.format);
    Ok(())
}

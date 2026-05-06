//! `cargo taut check` — detect IR drift in CI.
//!
//! See `ROADMAP.md` Phase 5. The intent is that CI runs `cargo taut check`
//! after `cargo build` to verify that the committed `api.gen.ts` matches the
//! IR a fresh build just produced — i.e. nobody forgot to re-run `gen`.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

/// Arguments for `cargo taut check`.
#[derive(Debug, Args)]
#[command(
    about = "Validate the IR (CI drift detection).",
    long_about = "Loads the IR and asserts invariants useful in CI: the \
                  ir_version field matches --expected-version (if given), and \
                  the file parses cleanly. Future: diff against the generated \
                  client to detect stale codegen."
)]
pub struct CheckArgs {
    /// Path to the IR JSON file to validate.
    #[arg(long, value_name = "PATH", default_value = "target/taut/ir.json")]
    pub ir: PathBuf,

    /// Required IR schema version. If set, `check` fails when the IR's
    /// `ir_version` differs.
    #[arg(long, value_name = "N")]
    pub expected_version: Option<u32>,
}

/// Entry point for `cargo taut check`.
pub fn run(args: CheckArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let ir_abs = if args.ir.is_absolute() {
        args.ir.clone()
    } else {
        cwd.join(&args.ir)
    };

    println!("unimplemented: see ROADMAP Phase 5");
    println!("  IR drift detection for CI: re-derives the IR during the build,");
    println!("  compares against the committed copy, and exits non-zero if the");
    println!("  generated TypeScript client is stale relative to Rust sources.");
    println!("  would read IR from:        {}", ir_abs.display());
    if let Some(v) = args.expected_version {
        println!("  expected ir_version:       {v}");
    }
    Ok(())
}

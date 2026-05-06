//! `cargo taut gen` — read the IR and emit a typed TypeScript client.
//!
//! See `ROADMAP.md` Phase 1 and `SPEC.md` §6.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, ValueEnum};

/// Validator runtime to target in the generated client.
///
/// Valibot is the default per `SPEC.md` §7; Zod is opt-in via `--validator zod`;
/// `none` skips emitting validation code entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Validator {
    /// Emit Valibot schemas (default).
    Valibot,
    /// Emit Zod schemas.
    Zod,
    /// Do not emit any validation code.
    None,
}

/// Arguments for `cargo taut gen`.
#[derive(Debug, Args)]
#[command(
    about = "Generate a TypeScript client from the taut-rpc IR.",
    long_about = "Reads target/taut/ir.json (or the path passed to --ir) and \
                  writes a typed TypeScript client to --out. The generated file \
                  is pure types plus procedure-name string constants; the \
                  runtime is shipped separately as the `taut-rpc` npm package."
)]
pub struct GenArgs {
    /// Path to the IR JSON file produced by the proc-macros.
    #[arg(long, value_name = "PATH", default_value = "target/taut/ir.json")]
    pub ir: PathBuf,

    /// Path to write the generated TypeScript client to.
    #[arg(long, value_name = "PATH", default_value = "src/api.gen.ts")]
    pub out: PathBuf,

    /// Validation runtime to target in the generated client.
    #[arg(long, value_name = "KIND", value_enum, default_value_t = Validator::Valibot)]
    pub validator: Validator,
}

/// Entry point for `cargo taut gen`.
pub fn run(args: GenArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let ir_abs = if args.ir.is_absolute() {
        args.ir.clone()
    } else {
        cwd.join(&args.ir)
    };
    let out_abs = if args.out.is_absolute() {
        args.out.clone()
    } else {
        cwd.join(&args.out)
    };

    println!("unimplemented: see ROADMAP Phase 1");
    println!("  would read IR from:  {}", ir_abs.display());
    println!("  would write TS to:   {}", out_abs.display());
    println!("  validator:           {:?}", args.validator);
    Ok(())
}

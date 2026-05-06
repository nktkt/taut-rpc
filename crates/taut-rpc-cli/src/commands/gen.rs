//! `cargo taut gen` — read the IR and emit a typed TypeScript client.
//!
//! See `ROADMAP.md` Phase 1 and `SPEC.md` §6.
//!
//! There are two ways to feed the IR into this command:
//!
//! 1. `--ir <PATH>` (default `target/taut/ir.json`) — read an IR file written
//!    by something else (e.g. an explicit `cargo run -- --dump-ir` step).
//! 2. `--from-binary <PATH>` — spawn the user's compiled binary with the
//!    `TAUT_DUMP_IR` env var set, let it write its IR via
//!    `taut_rpc::dump_if_requested`, then read that file back and proceed.
//!
//! The two flags are mutually exclusive: pick one input source per invocation.
//!
//! Once the IR is on disk, [`crate::codegen::render_ts`] turns it into a
//! single `api.gen.ts` source string, which we write to `--out`.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, ValueEnum};

use crate::codegen::{self, CodegenOptions};

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

impl From<Validator> for codegen::Validator {
    fn from(v: Validator) -> Self {
        match v {
            Validator::Valibot => codegen::Validator::Valibot,
            Validator::Zod => codegen::Validator::Zod,
            Validator::None => codegen::Validator::None,
        }
    }
}

/// How to emit 64- and 128-bit integers in the generated client. Mirrors
/// [`taut_rpc::type_map::BigIntStrategy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum BigIntStrategy {
    /// Emit `bigint` (default per `SPEC.md` §3.1).
    Native,
    /// Emit `string` for u64/i64/u128/i128.
    AsString,
}

impl From<BigIntStrategy> for taut_rpc::type_map::BigIntStrategy {
    fn from(v: BigIntStrategy) -> Self {
        match v {
            BigIntStrategy::Native => taut_rpc::type_map::BigIntStrategy::Native,
            BigIntStrategy::AsString => taut_rpc::type_map::BigIntStrategy::AsString,
        }
    }
}

/// Arguments for `cargo taut gen`.
#[derive(Debug, Args)]
#[command(
    about = "Generate a TypeScript client from the taut-rpc IR.",
    long_about = "Reads target/taut/ir.json (or the path passed to --ir) and \
                  writes a typed TypeScript client to --out. The generated file \
                  is pure types plus procedure-name string constants; the \
                  runtime is shipped separately as the `taut-rpc` npm package. \
                  Pass --from-binary <PATH> to dump the IR straight from a \
                  compiled binary instead of reading an existing IR file."
)]
pub struct GenArgs {
    /// Path to the IR JSON file produced by the proc-macros.
    #[arg(
        long,
        value_name = "PATH",
        default_value = "target/taut/ir.json",
        conflicts_with = "from_binary"
    )]
    pub ir: PathBuf,

    /// Path to a compiled user binary. The binary is spawned with
    /// `TAUT_DUMP_IR=<--ir path>` so it writes its IR via
    /// `taut_rpc::dump_if_requested` and exits before binding any port.
    #[arg(long, value_name = "PATH", conflicts_with = "ir")]
    pub from_binary: Option<PathBuf>,

    /// Path to write the generated TypeScript client to.
    #[arg(long, value_name = "PATH", default_value = "src/api.gen.ts")]
    pub out: PathBuf,

    /// Validation runtime to target in the generated client.
    #[arg(long, value_name = "KIND", value_enum, default_value_t = Validator::Valibot)]
    pub validator: Validator,

    /// How to render 64- and 128-bit integers.
    #[arg(long, value_name = "STRATEGY", value_enum, default_value_t = BigIntStrategy::Native)]
    pub bigint_strategy: BigIntStrategy,
}

/// Entry point for `cargo taut gen`.
pub fn run(args: GenArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;

    // `--ir` carries a default value, so it's always set even when the user
    // chose `--from-binary`. We treat that default as "the path the binary
    // should dump to" in the from-binary flow, and as "the path to read" in
    // the default flow.
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

    if let Some(bin) = args.from_binary.as_ref() {
        let bin_abs = if bin.is_absolute() {
            bin.clone()
        } else {
            cwd.join(bin)
        };
        dump_ir_from_binary(&bin_abs, &ir_abs)?;
        println!("dumped IR from {} to {}", bin_abs.display(), ir_abs.display());
    }

    if !ir_abs.exists() {
        return Err(anyhow!(
            "IR file not found at {}.\n\
             Build and run your crate first so the proc-macros can dump the IR \
             (e.g. `cargo run` with `taut_rpc::dump_if_requested(&router)` in \
             your `main`), or pass `--from-binary <PATH>` to spawn it for you.",
            ir_abs.display()
        ));
    }

    let raw = std::fs::read_to_string(&ir_abs)
        .with_context(|| format!("reading IR from {}", ir_abs.display()))?;
    let ir: taut_rpc::ir::Ir = serde_json::from_str(&raw)
        .with_context(|| format!("parsing IR JSON at {}", ir_abs.display()))?;

    if ir.ir_version != taut_rpc::IR_VERSION {
        bail!(
            "IR schema version mismatch: file at {} reports ir_version={}, \
             but this CLI expects ir_version={}. Rebuild your crate with a \
             matching `taut-rpc` version, or upgrade `taut-rpc-cli` if your \
             crate is newer.",
            ir_abs.display(),
            ir.ir_version,
            taut_rpc::IR_VERSION
        );
    }

    let opts = CodegenOptions {
        validator: args.validator.into(),
        bigint_strategy: args.bigint_strategy.into(),
        honor_undefined: true,
    };
    let rendered = codegen::render_ts(&ir, &opts);

    if let Some(parent) = out_abs.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
    }
    std::fs::write(&out_abs, rendered.as_bytes())
        .with_context(|| format!("writing TS to {}", out_abs.display()))?;

    println!("wrote {} bytes to {}", rendered.len(), out_abs.display());
    Ok(())
}

/// Spawn `bin` with `TAUT_DUMP_IR=<ir_target>`, wait for it to exit, and
/// surface a useful error if it fails.
///
/// The binary is expected to call `taut_rpc::dump_if_requested(&router)`
/// early in `main()` so it writes the IR to `ir_target` and exits with status
/// 0 before binding any port. We capture stderr so a non-zero exit produces a
/// readable error rather than just "process exited 2".
fn dump_ir_from_binary(bin: &std::path::Path, ir_target: &std::path::Path) -> Result<()> {
    if !bin.exists() {
        bail!(
            "--from-binary path does not exist: {}",
            bin.display()
        );
    }

    // Make sure the parent of the IR target exists. The binary's
    // `dump_if_requested` does this too, but failing here gives a clearer
    // error if we e.g. lack permission to create `target/taut/`.
    if let Some(parent) = ir_target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create IR output directory: {}", parent.display())
            })?;
        }
    }

    let output = Command::new(bin)
        .env("TAUT_DUMP_IR", ir_target)
        .output()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "<terminated by signal>".to_string());
        return Err(anyhow!(
            "binary {} exited with status {} while dumping IR\n--- stderr ---\n{}",
            bin.display(),
            code,
            stderr.trim_end()
        ));
    }

    if !ir_target.exists() {
        bail!(
            "binary {} exited successfully but did not write IR to {}; \
             does its main() call `taut_rpc::dump_if_requested(&router)` \
             before any port binding?",
            bin.display(),
            ir_target.display()
        );
    }

    Ok(())
}

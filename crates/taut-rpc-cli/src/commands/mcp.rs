//! `cargo taut mcp` — read the IR and emit an MCP `tools/list` manifest.
//!
//! Each non-subscription `#[rpc]` procedure becomes an MCP tool whose
//! `inputSchema` is a JSON Schema (Draft 2020-12) describing the wire
//! envelope `{"input": <value>}`. See `crate::mcp` for the schema shape.
//!
//! Two ways to feed the IR in (mirrors `cargo taut gen`):
//! 1. `--ir <PATH>` (default `target/taut/ir.json`) — read an IR file.
//! 2. `--from-binary <PATH>` — spawn the user's compiled binary with
//!    `TAUT_DUMP_IR` set, let `taut_rpc::dump_if_requested` dump the IR,
//!    then read it back.

use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, ValueEnum};

use crate::mcp::{render_manifest, McpOptions};

/// How to render 64- and 128-bit integers in the JSON Schema.
///
/// Mirrors [`taut_rpc::type_map::BigIntStrategy`] and the equivalent flag on
/// `cargo taut gen`. Most LLM tool callers cannot emit Rust-style bigints, so
/// `as-string` is a defensive option for u64/i64 inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum McpBigIntStrategy {
    /// Emit `{ "type": "integer" }` for u64/i64/u128/i128 (default).
    Native,
    /// Emit `{ "type": "string", "pattern": "^-?\\d+$" }`.
    AsString,
}

impl From<McpBigIntStrategy> for taut_rpc::type_map::BigIntStrategy {
    fn from(v: McpBigIntStrategy) -> Self {
        match v {
            McpBigIntStrategy::Native => taut_rpc::type_map::BigIntStrategy::Native,
            McpBigIntStrategy::AsString => taut_rpc::type_map::BigIntStrategy::AsString,
        }
    }
}

/// Arguments for `cargo taut mcp`.
#[derive(Debug, Args)]
#[command(
    about = "Emit an MCP tools/list manifest from the taut-rpc IR.",
    long_about = "Reads target/taut/ir.json (or the path passed to --ir) and \
                  writes a JSON manifest matching the MCP `tools/list` \
                  response shape (spec 2025-06-18). Each query/mutation \
                  procedure becomes a tool whose `inputSchema` is a JSON \
                  Schema (Draft 2020-12). Subscriptions are skipped by \
                  default; pass --include-subscriptions to surface them too."
)]
pub struct McpArgs {
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

    /// Path to write the generated manifest to. Use `-` for stdout.
    #[arg(long, value_name = "PATH", default_value = "target/taut/mcp.json")]
    pub out: PathBuf,

    /// How to render 64- and 128-bit integers in the JSON Schema.
    #[arg(long, value_name = "STRATEGY", value_enum, default_value_t = McpBigIntStrategy::Native)]
    pub bigint_strategy: McpBigIntStrategy,

    /// Also emit a tool entry per subscription procedure. MCP tools are
    /// strictly request/response; the streaming nature is invisible at the
    /// manifest level.
    #[arg(long)]
    pub include_subscriptions: bool,
}

/// Entry point for `cargo taut mcp`.
#[allow(clippy::needless_pass_by_value)] // owned `args` matches the clap-generated dispatch convention
pub fn run(args: McpArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;

    let ir_abs = if args.ir.is_absolute() {
        args.ir.clone()
    } else {
        cwd.join(&args.ir)
    };

    if let Some(bin) = args.from_binary.as_ref() {
        let bin_abs = if bin.is_absolute() {
            bin.clone()
        } else {
            cwd.join(bin)
        };
        dump_ir_from_binary(&bin_abs, &ir_abs)?;
        eprintln!(
            "dumped IR from {} to {}",
            bin_abs.display(),
            ir_abs.display()
        );
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

    let opts = McpOptions {
        bigint_strategy: args.bigint_strategy.into(),
        include_subscriptions: args.include_subscriptions,
    };
    let manifest = render_manifest(&ir, &opts);
    let rendered =
        serde_json::to_string_pretty(&manifest).context("serializing MCP manifest to JSON")?;

    if args.out.as_os_str() == "-" {
        println!("{rendered}");
        return Ok(());
    }

    let out_abs = if args.out.is_absolute() {
        args.out.clone()
    } else {
        cwd.join(&args.out)
    };
    if let Some(parent) = out_abs.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
    }
    std::fs::write(&out_abs, rendered.as_bytes())
        .with_context(|| format!("writing manifest to {}", out_abs.display()))?;

    eprintln!("wrote {} bytes to {}", rendered.len(), out_abs.display());
    Ok(())
}

/// Spawn `bin` with `TAUT_DUMP_IR=<ir_target>`, wait for it to exit, surface
/// a useful error if it fails. Mirrors the helper in `commands/gen.rs`.
fn dump_ir_from_binary(bin: &std::path::Path, ir_target: &std::path::Path) -> Result<()> {
    if !bin.exists() {
        bail!("--from-binary path does not exist: {}", bin.display());
    }

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
            .map_or_else(|| "<terminated by signal>".to_string(), |c| c.to_string());
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

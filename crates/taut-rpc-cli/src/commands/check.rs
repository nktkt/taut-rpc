//! `cargo taut check` — detect IR drift in CI.
//!
//! See `ROADMAP.md` Phase 5. The intent is that CI runs `cargo taut check`
//! after `cargo build` to verify that the IR a fresh build just produced
//! still matches the committed `taut/ir.snapshot.json`. If it doesn't, some
//! Rust change altered the public surface of the API without re-running
//! `cargo taut gen` and re-committing the snapshot.
//!
//! There are two ways to feed the *current* IR into this command, mirroring
//! `cargo taut gen`:
//!
//! 1. `--ir <PATH>` (default `target/taut/ir.json`) — read an IR file written
//!    by something else (e.g. an explicit `cargo run -- --dump-ir` step).
//! 2. `--from-binary <PATH>` — spawn the user's compiled binary with the
//!    `TAUT_DUMP_IR` env var set, let it write its IR via
//!    `taut_rpc::dump_if_requested`, then read that file back.
//!
//! The two flags are mutually exclusive.
//!
//! The *baseline* — the committed snapshot — defaults to
//! `taut/ir.snapshot.json` at the repo root. With `--write`, the current IR
//! is written to that path (CI-bootstrap mode). Without `--write`, the
//! current IR is compared against the baseline; equality is exit 0, drift is
//! exit 1 with a unified-diff-style summary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use clap::Args;

use taut_rpc::ir::{Ir, Procedure, TypeDef};

/// Arguments for `cargo taut check`.
#[derive(Debug, Args)]
#[command(
    about = "Detect IR drift between the live build and the committed baseline.",
    long_about = "Re-derives the current IR (either by reading --ir or by \
                  spawning --from-binary with TAUT_DUMP_IR set) and compares \
                  it against the committed baseline at --baseline. Exits 0 if \
                  they match and 1 if they differ, printing a summary of \
                  added/removed/changed procedures and types. Pass --write to \
                  overwrite the baseline with the current IR (CI-bootstrap \
                  mode)."
)]
pub struct CheckArgs {
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

    /// Path to the committed baseline snapshot to compare against (or write
    /// to with `--write`).
    #[arg(long, value_name = "PATH", default_value = "taut/ir.snapshot.json")]
    pub baseline: PathBuf,

    /// Overwrite `--baseline` with the current IR instead of comparing.
    /// Useful for bootstrapping the snapshot in a fresh repo.
    #[arg(long)]
    pub write: bool,
}

/// Entry point for `cargo taut check`.
#[allow(clippy::needless_pass_by_value)] // owned `args` matches the clap-generated dispatch convention
pub fn run(args: CheckArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;

    let ir_abs = if args.ir.is_absolute() {
        args.ir.clone()
    } else {
        cwd.join(&args.ir)
    };
    let baseline_abs = if args.baseline.is_absolute() {
        args.baseline.clone()
    } else {
        cwd.join(&args.baseline)
    };

    if let Some(bin) = args.from_binary.as_ref() {
        let bin_abs = if bin.is_absolute() {
            bin.clone()
        } else {
            cwd.join(bin)
        };
        dump_ir_from_binary(&bin_abs, &ir_abs)?;
    }

    let current = load_ir(&ir_abs)?;
    verify_ir_version(&current, &ir_abs)?;

    if args.write {
        write_baseline(&current, &baseline_abs)
    } else {
        compare_against_baseline(&current, &baseline_abs)
    }
}

/// Load and parse an IR JSON file at `path`.
fn load_ir(path: &Path) -> Result<Ir> {
    if !path.exists() {
        return Err(anyhow!(
            "IR file not found at {}.\n\
             Build and run your crate first so the proc-macros can dump the IR \
             (e.g. `cargo run` with `taut_rpc::dump_if_requested(&router)` in \
             your `main`), or pass `--from-binary <PATH>` to spawn it for you.",
            path.display()
        ));
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading IR from {}", path.display()))?;
    let ir: Ir = serde_json::from_str(&raw)
        .with_context(|| format!("parsing IR JSON at {}", path.display()))?;
    Ok(ir)
}

/// Reject an IR whose `ir_version` doesn't match what this CLI was built
/// against. Mirrors the shape used in `gen.rs` so the error is consistent.
fn verify_ir_version(ir: &Ir, src: &Path) -> Result<()> {
    if ir.ir_version != taut_rpc::IR_VERSION {
        bail!(
            "IR schema version mismatch: file at {} reports ir_version={}, \
             but this CLI expects ir_version={}. Rebuild your crate with a \
             matching `taut-rpc` version, or upgrade `taut-rpc-cli` if your \
             crate is newer.",
            src.display(),
            ir.ir_version,
            taut_rpc::IR_VERSION
        );
    }
    Ok(())
}

/// Pretty-serialize `ir` and write it to `baseline`, creating parent dirs as
/// needed.
fn write_baseline(ir: &Ir, baseline: &Path) -> Result<()> {
    let body = serde_json::to_string_pretty(ir).context("serializing IR to pretty JSON")?;
    if let Some(parent) = baseline.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating baseline directory {}", parent.display()))?;
        }
    }
    std::fs::write(baseline, body.as_bytes())
        .with_context(|| format!("writing baseline to {}", baseline.display()))?;
    println!("wrote {} bytes to {}", body.len(), baseline.display());
    Ok(())
}

/// Compare the current IR against the baseline at `baseline`. Returns Ok on
/// match and Err on drift; the Err carries a unified-diff-style summary.
fn compare_against_baseline(current: &Ir, baseline: &Path) -> Result<()> {
    if !baseline.exists() {
        return Err(anyhow!(
            "baseline snapshot not found at {}.\n\
             Run `cargo taut check --write` to bootstrap it, or pass \
             `--baseline <PATH>` to point at an existing snapshot.",
            baseline.display()
        ));
    }
    let raw = std::fs::read_to_string(baseline)
        .with_context(|| format!("reading baseline from {}", baseline.display()))?;
    let baseline_ir: Ir = serde_json::from_str(&raw)
        .with_context(|| format!("parsing baseline JSON at {}", baseline.display()))?;

    let report = diff_ir(&baseline_ir, current);
    if report.is_clean() {
        println!(
            "IR matches baseline ({} procedures, {} types)",
            current.procedures.len(),
            current.types.len()
        );
        Ok(())
    } else {
        Err(anyhow!(
            "IR drift detected vs baseline at {}\n{}",
            baseline.display(),
            report.render()
        ))
    }
}

/// A summary of the differences between two IR documents, broken down by
/// procedures and by types and grouped into added / removed / changed.
#[derive(Debug, Default, PartialEq, Eq)]
struct DiffReport {
    proc_added: Vec<String>,
    proc_removed: Vec<String>,
    proc_changed: Vec<String>,
    type_added: Vec<String>,
    type_removed: Vec<String>,
    type_changed: Vec<String>,
}

impl DiffReport {
    fn is_clean(&self) -> bool {
        self.proc_added.is_empty()
            && self.proc_removed.is_empty()
            && self.proc_changed.is_empty()
            && self.type_added.is_empty()
            && self.type_removed.is_empty()
            && self.type_changed.is_empty()
    }

    /// Render the report as a unified-diff-style summary. Lines are prefixed
    /// `+` (added), `-` (removed), or `~` (changed).
    fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for name in &self.proc_added {
            let _ = writeln!(out, "+ procedure {name}");
        }
        for name in &self.proc_removed {
            let _ = writeln!(out, "- procedure {name}");
        }
        for name in &self.proc_changed {
            let _ = writeln!(out, "~ procedure {name} (signature changed)");
        }
        for name in &self.type_added {
            let _ = writeln!(out, "+ type {name}");
        }
        for name in &self.type_removed {
            let _ = writeln!(out, "- type {name}");
        }
        for name in &self.type_changed {
            let _ = writeln!(out, "~ type {name} (definition changed)");
        }
        // Trim the trailing newline so callers can wrap us cleanly.
        if out.ends_with('\n') {
            out.pop();
        }
        out
    }
}

/// Compute a structural diff between `baseline` and `current`. Both sides are
/// indexed by name; identical names with non-equal `Procedure`/`TypeDef`
/// values land in the "changed" buckets.
fn diff_ir(baseline: &Ir, current: &Ir) -> DiffReport {
    let base_procs: BTreeMap<&str, &Procedure> = baseline
        .procedures
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();
    let cur_procs: BTreeMap<&str, &Procedure> = current
        .procedures
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();

    let base_types: BTreeMap<&str, &TypeDef> = baseline
        .types
        .iter()
        .map(|t| (t.name.as_str(), t))
        .collect();
    let cur_types: BTreeMap<&str, &TypeDef> =
        current.types.iter().map(|t| (t.name.as_str(), t)).collect();

    let mut report = DiffReport::default();

    for (name, p) in &cur_procs {
        match base_procs.get(name) {
            None => report.proc_added.push((*name).to_string()),
            Some(b) if *b != *p => report.proc_changed.push((*name).to_string()),
            _ => {}
        }
    }
    for name in base_procs.keys() {
        if !cur_procs.contains_key(name) {
            report.proc_removed.push((*name).to_string());
        }
    }

    for (name, t) in &cur_types {
        match base_types.get(name) {
            None => report.type_added.push((*name).to_string()),
            Some(b) if *b != *t => report.type_changed.push((*name).to_string()),
            _ => {}
        }
    }
    for name in base_types.keys() {
        if !cur_types.contains_key(name) {
            report.type_removed.push((*name).to_string());
        }
    }

    // BTreeMap iteration is already sorted by key, but the removed-loop above
    // produces sorted output too. Sort defensively in case the iteration
    // order ever changes.
    report.proc_added.sort();
    report.proc_removed.sort();
    report.proc_changed.sort();
    report.type_added.sort();
    report.type_removed.sort();
    report.type_changed.sort();

    report
}

/// Spawn `bin` with `TAUT_DUMP_IR=<ir_target>`, wait for it to exit, and
/// surface a useful error if it fails.
///
/// Mirrors `gen::dump_ir_from_binary` — see that function for the protocol
/// the spawned binary is expected to follow (call
/// `taut_rpc::dump_if_requested(&router)` early in `main` and exit before
/// binding any port).
fn dump_ir_from_binary(bin: &Path, ir_target: &Path) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use taut_rpc::ir::{HttpMethod, Primitive, ProcKind, TypeRef, TypeShape};

    /// Build a small but realistic IR with one procedure and one type so each
    /// test starts from the same canonical baseline.
    fn sample_ir() -> Ir {
        Ir {
            ir_version: taut_rpc::IR_VERSION,
            procedures: vec![Procedure {
                name: "get_user".to_string(),
                kind: ProcKind::Query,
                input: TypeRef::Primitive(Primitive::String),
                output: TypeRef::Named("User".to_string()),
                errors: vec![],
                http_method: HttpMethod::Post,
                doc: None,
            }],
            types: vec![TypeDef {
                name: "User".to_string(),
                doc: None,
                shape: TypeShape::Struct(vec![]),
            }],
        }
    }

    #[test]
    fn equal_ir_returns_clean_match() {
        // Identical IR on both sides should diff to nothing — the "matches"
        // path the CLI prints in CI when nobody forgot to re-run `gen`.
        let base = sample_ir();
        let cur = sample_ir();
        let report = diff_ir(&base, &cur);
        assert!(report.is_clean(), "expected no drift, got {report:?}");
    }

    #[test]
    fn equal_ir_compare_against_baseline_writes_match_message() {
        // End-to-end check that `compare_against_baseline` is happy when
        // baseline and current match: it must return Ok(()) without error.
        let dir = std::env::temp_dir().join(format!(
            "taut-check-equal-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let baseline_path = dir.join("ir.snapshot.json");
        let ir = sample_ir();
        std::fs::write(
            &baseline_path,
            serde_json::to_string_pretty(&ir).unwrap().as_bytes(),
        )
        .unwrap();

        let result = compare_against_baseline(&ir, &baseline_path);
        std::fs::remove_dir_all(&dir).ok();
        result.expect("equal IRs should compare clean");
    }

    #[test]
    fn different_procedure_list_reports_drift() {
        // Drop `get_user`, add `create_user`. The diff must surface the
        // removal and the addition as separate entries so a reviewer can see
        // exactly which procedures moved.
        let base = sample_ir();
        let mut cur = sample_ir();
        cur.procedures = vec![Procedure {
            name: "create_user".to_string(),
            kind: ProcKind::Mutation,
            input: TypeRef::Named("User".to_string()),
            output: TypeRef::Primitive(Primitive::Unit),
            errors: vec![],
            http_method: HttpMethod::Post,
            doc: None,
        }];

        let report = diff_ir(&base, &cur);
        assert!(!report.is_clean());
        assert_eq!(report.proc_added, vec!["create_user".to_string()]);
        assert_eq!(report.proc_removed, vec!["get_user".to_string()]);
        assert!(report.proc_changed.is_empty());

        let rendered = report.render();
        assert!(
            rendered.contains("+ procedure create_user"),
            "missing addition line in:\n{rendered}"
        );
        assert!(
            rendered.contains("- procedure get_user"),
            "missing removal line in:\n{rendered}"
        );
    }

    #[test]
    fn changed_procedure_signature_lands_in_changed_bucket() {
        // Same name, different signature: that's the "stale codegen"
        // scenario we most care about catching in CI.
        let base = sample_ir();
        let mut cur = sample_ir();
        cur.procedures[0].kind = ProcKind::Mutation;

        let report = diff_ir(&base, &cur);
        assert_eq!(report.proc_changed, vec!["get_user".to_string()]);
        assert!(report.proc_added.is_empty());
        assert!(report.proc_removed.is_empty());
        assert!(report
            .render()
            .contains("~ procedure get_user (signature changed)"));
    }

    #[test]
    fn type_diff_uses_type_buckets() {
        // Add one type, remove the existing `User` — types should be
        // reported via the type-prefixed lines, not as procedures.
        let base = sample_ir();
        let mut cur = sample_ir();
        cur.types = vec![TypeDef {
            name: "Session".to_string(),
            doc: None,
            shape: TypeShape::Struct(vec![]),
        }];

        let report = diff_ir(&base, &cur);
        assert_eq!(report.type_added, vec!["Session".to_string()]);
        assert_eq!(report.type_removed, vec!["User".to_string()]);
        let rendered = report.render();
        assert!(rendered.contains("+ type Session"));
        assert!(rendered.contains("- type User"));
    }

    #[test]
    fn mismatched_ir_version_fails_before_diff() {
        // The IR schema is versioned (SPEC §9). If the file on disk reports
        // a different version than this CLI was built against, refuse to
        // diff at all — the diff would be meaningless across schemas.
        let mut ir = sample_ir();
        ir.ir_version = taut_rpc::IR_VERSION + 1;

        let err = verify_ir_version(&ir, Path::new("dummy.json"))
            .expect_err("version mismatch must error");
        let msg = format!("{err}");
        assert!(
            msg.contains("IR schema version mismatch"),
            "unexpected error message: {msg}"
        );
        assert!(msg.contains(&format!("ir_version={}", taut_rpc::IR_VERSION + 1)));
        assert!(msg.contains(&format!("expects ir_version={}", taut_rpc::IR_VERSION)));
    }

    #[test]
    fn write_baseline_serializes_pretty_json() {
        // `--write` is the bootstrap mode. The on-disk snapshot must be
        // pretty-printed (so it diffs cleanly in PR review) and must
        // round-trip through serde back to the same IR.
        let dir = std::env::temp_dir().join(format!(
            "taut-check-write-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let baseline_path = dir.join("nested").join("ir.snapshot.json");
        let ir = sample_ir();

        let res = write_baseline(&ir, &baseline_path);
        let read_back = std::fs::read_to_string(&baseline_path).ok();
        std::fs::remove_dir_all(&dir).ok();

        res.expect("write_baseline should succeed");
        let body = read_back.expect("baseline should exist");
        // Pretty-print uses two-space indentation and newlines.
        assert!(body.contains('\n'), "expected pretty-printed JSON");
        let round: Ir = serde_json::from_str(&body).expect("baseline is valid JSON");
        assert_eq!(round, ir);
    }
}

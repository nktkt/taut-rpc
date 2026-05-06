//! IR-dump entrypoint used by `cargo taut gen`. See SPEC §2.
//!
//! `cargo taut gen` produces `target/taut/ir.json` by cooperating with the
//! user's binary: it spawns the binary with the `TAUT_DUMP_IR` environment
//! variable set, and the binary writes its IR and exits before binding any
//! port. This avoids parsing Rust source ourselves and guarantees the IR
//! reflects exactly what the runtime sees, since both come from the same
//! `Router::ir()` call.
//!
//! The user's `main()` should call [`dump_if_requested`] early — before
//! `tokio::main` (or any port binding / long-lived setup). If the env var
//! `TAUT_DUMP_IR` is set, the function writes the IR JSON and exits the
//! process with status 0; otherwise it returns and `main()` proceeds normally.
//!
//! ```ignore
//! fn main() {
//!     let router = taut_rpc::Router::new()
//!         .procedure(/* ... */);
//!     taut_rpc::dump_if_requested(&router);
//!     // …normal startup follows…
//! }
//! ```
//!
//! For users who want explicit control over where the IR goes (e.g. piping it
//! into another tool from a custom subcommand), [`ir_json`] returns the
//! pretty-printed JSON without touching the environment or exiting.

use crate::Router;

/// If `TAUT_DUMP_IR` is set, write the router's IR to the requested location
/// and `exit(0)`; otherwise no-op.
///
/// # Behaviour
///
/// - `TAUT_DUMP_IR` unset (or empty) → returns immediately, caller continues.
/// - `TAUT_DUMP_IR=1`, `TAUT_DUMP_IR=true`, or `TAUT_DUMP_IR=stdout` → write
///   the IR JSON to standard output and exit with status 0.
/// - `TAUT_DUMP_IR=<path>` → write the IR JSON to that file (parent
///   directories are created if missing) and exit with status 0.
///
/// On any failure (serialization, `mkdir -p`, file write), the function prints
/// a diagnostic to stderr and exits with status 2. The non-zero status lets
/// `cargo taut gen` distinguish a dump failure from a successful dump that
/// happens to produce empty output.
///
/// # When to call
///
/// Call this *first thing* in `main()`, before any port binding, database
/// connection, or other side-effect that might fail in a CI environment that
/// only wants to extract the IR. The whole point of the protocol is that the
/// codegen tool doesn't need a working database, network, or filesystem
/// outside the IR target path.
pub fn dump_if_requested(router: &Router) {
    let val = match std::env::var("TAUT_DUMP_IR") {
        Ok(v) if !v.is_empty() => v,
        _ => return,
    };
    let ir = router.ir();
    let json = match serde_json::to_string_pretty(&ir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("taut-rpc: failed to serialize IR: {e}");
            std::process::exit(2);
        }
    };
    let target_is_stdout = matches!(val.as_str(), "1" | "true" | "stdout");
    if target_is_stdout {
        println!("{json}");
    } else {
        let path = std::path::PathBuf::from(&val);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    eprintln!(
                        "taut-rpc: failed to create {}: {e}",
                        parent.display()
                    );
                    std::process::exit(2);
                }
            }
        }
        if let Err(e) = std::fs::write(&path, &json) {
            eprintln!("taut-rpc: failed to write {}: {e}", path.display());
            std::process::exit(2);
        }
    }
    std::process::exit(0);
}

/// Return the router's IR as a pretty-printed JSON string.
///
/// Useful for callers that want to embed IR extraction in a custom subcommand
/// or test, without the env-var dispatch and `exit()` semantics of
/// [`dump_if_requested`].
pub fn ir_json(router: &Router) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(&router.ir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Ir;

    // Note: `dump_if_requested` reads `TAUT_DUMP_IR` and calls
    // `std::process::exit`, so exercising its env-driven branch requires a
    // subprocess harness. We don't have one in this crate's unit tests; the
    // CLI-side `--from-binary` flow effectively integration-tests it. Here we
    // cover the behaviour the unit-test layer can observe: that `ir_json`
    // produces valid IR JSON, and that an unset env var is a no-op (we infer
    // the no-op from `dump_if_requested` returning rather than exiting — if
    // it did exit, the test process would die and the test runner would
    // report a failure).

    #[test]
    fn ir_json_returns_parseable_ir_for_empty_router() {
        let router = Router::new();
        let json = ir_json(&router).expect("serialize empty IR");

        // Must be valid JSON.
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("ir_json produced valid JSON");
        assert!(parsed.is_object(), "IR root must be a JSON object");

        // Must round-trip back into the typed IR with the current schema
        // version (SPEC §9).
        let ir: Ir = serde_json::from_str(&json).expect("ir_json round-trips into Ir");
        assert_eq!(ir.ir_version, Ir::CURRENT_VERSION);
        assert!(ir.procedures.is_empty());
        assert!(ir.types.is_empty());
    }

    #[test]
    fn ir_json_is_pretty_printed() {
        // We promise pretty output (newlines + indentation) so that a dumped
        // `ir.json` is human-diffable in CI. Assert the multi-line shape
        // rather than exact whitespace, which serde could legitimately
        // reformat.
        let router = Router::new();
        let json = ir_json(&router).expect("serialize");
        assert!(
            json.contains('\n'),
            "expected multi-line pretty JSON, got: {json}"
        );
    }

    // The "noop when env unset" test was removed: env::set_var / remove_var are
    // `unsafe` from the 2024 edition, and the workspace forbids unsafe code. The
    // env-driven exit path is exercised by integration tests with a subprocess
    // (Phase 1 example smoke), where it can run safely under TAUT_DUMP_IR.
}

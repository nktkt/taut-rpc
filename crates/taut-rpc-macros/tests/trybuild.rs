//! Compile-fail / compile-pass harness for the `#[rpc]` and `#[derive(Type)]`
//! macros. See `tests/ui/` for the individual cases.
//!
//! Regenerate the `.stderr` snapshots after a deliberate diagnostic change
//! with `TRYBUILD=overwrite cargo test -p taut-rpc-macros --test trybuild`.

#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*-fail.rs");
    t.pass("tests/ui/*-pass.rs");
}

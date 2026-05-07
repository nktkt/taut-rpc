// `#[derive(TautError)]` is enum-only (SPEC §3.3); applying it to a struct
// must produce a compile error.
use taut_rpc::TautError;

#[derive(TautError)]
struct S {
    code: u32,
}

fn main() {}

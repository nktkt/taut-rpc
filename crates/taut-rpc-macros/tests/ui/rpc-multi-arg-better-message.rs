// Verifies the multi-arg rejection message guides users to wrap their
// arguments in a struct (SPEC §5). The committed `.stderr` for this case
// must contain the phrase "wrap your arguments in a struct".
use taut_rpc::rpc;

#[rpc]
async fn three(a: i32, b: i32, c: i32) -> i32 {
    a + b + c
}

fn main() {}

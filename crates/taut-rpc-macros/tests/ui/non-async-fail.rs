use taut_rpc::rpc;

#[rpc]
fn ping() -> &'static str {
    "pong"
}

fn main() {}

use taut_rpc::rpc;

#[rpc]
async fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {}

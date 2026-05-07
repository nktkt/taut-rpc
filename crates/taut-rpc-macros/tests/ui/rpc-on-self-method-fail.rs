use taut_rpc::rpc;

struct Foo;

impl Foo {
    #[rpc]
    async fn bar(&self) -> u32 {
        0
    }
}

fn main() {}

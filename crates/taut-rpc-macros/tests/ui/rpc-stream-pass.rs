// Basic `#[rpc(stream)]` smoke test — an `async fn` returning
// `impl Stream<Item = T>` should expand cleanly and compile.
use taut_rpc::rpc;

#[rpc(stream)]
async fn ticks() -> impl futures::Stream<Item = u64> + Send + 'static {
    futures::stream::empty()
}

fn main() {}

// `#[rpc(stream)]` requires an `async fn`; a plain `fn` returning
// `impl Stream<Item = T>` must be rejected with the "requires an async fn"
// diagnostic.
use taut_rpc::rpc;

#[rpc(stream)]
fn ticks() -> impl futures::Stream<Item = u64> + Send + 'static {
    futures::stream::empty()
}

fn main() {}

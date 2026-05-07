use criterion::{black_box, criterion_group, criterion_main, Criterion};
// The router/descriptor imports are unused today; they are kept as anchors for
// the dispatch-through-router benchmarks that will land in a follow-up phase.
use std::sync::Arc;
#[allow(unused_imports)]
use taut_rpc::{ProcedureBody, ProcedureDescriptor, ProcedureResult, Router};

fn bench_handler_dispatch(c: &mut Criterion) {
    c.bench_function("noop_handler", |b| {
        let handler: taut_rpc::UnaryHandler =
            Arc::new(|_v| Box::pin(async move { ProcedureResult::Ok(serde_json::Value::Null) }));
        let rt = tokio::runtime::Runtime::new().unwrap();
        b.iter(|| {
            rt.block_on(async {
                let _ = handler(black_box(serde_json::Value::Null)).await;
            });
        });
    });
}

criterion_group!(benches, bench_handler_dispatch);
criterion_main!(benches);

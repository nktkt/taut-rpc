use taut_rpc::Validate;
#[derive(Validate)]
struct X { foo: u32, bar: String }
fn main() {
    let _ = X { foo: 0, bar: String::new() };
}

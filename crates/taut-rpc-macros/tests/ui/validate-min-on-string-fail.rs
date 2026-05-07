use taut_rpc::Validate;
#[derive(Validate)]
struct X { #[taut(min = 3)] name: String }
fn main() {}

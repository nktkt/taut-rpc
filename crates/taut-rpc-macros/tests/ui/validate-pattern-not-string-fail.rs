use taut_rpc::Validate;
#[derive(Validate)]
struct X { #[taut(pattern = "^[a-z]+$")] age: u32 }
fn main() {}
